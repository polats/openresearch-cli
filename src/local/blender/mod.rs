//! Blender as a generative-AI provider, reached through the Blender Lab MCP
//! server (`reference/blender_mcp`).
//!
//! Three moving parts, and the reason this module reports each one separately is
//! that their fixes are different:
//!
//! 1. **`blender-mcp`** — a Python CLI the user installs. Present on PATH is not
//!    the same as runnable: a pipx venv whose interpreter was removed by a Python
//!    upgrade leaves the launcher in place and failing, which is the state this
//!    was written against.
//! 2. **A running Blender with the add-on's socket server** on
//!    `localhost:9876`. Owned by the user, not by us — Blender is a GUI app they
//!    open and close.
//! 3. **The MCP server process** crux manages, in [`server`].
//!
//! The layering matters because part 3 does *not* depend on part 2 at runtime.
//! The MCP server opens a **fresh TCP connection per tool call**
//! (`tools_helpers/connection.py`), so it holds no session and needs no restart
//! when Blender is closed and reopened — Blender-backed tools simply fail with
//! the add-on's own "is Blender running" message in between. That is what lets
//! crux spawn one long-lived server and hand its URL to harness children that
//! outlive any individual Blender session.

pub(crate) mod server;

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The add-on's socket defaults, and the env vars that override them. Matching
/// the MCP server's own `get_connection_params` is what keeps our status probe
/// and its tool calls pointed at the same Blender.
const DEFAULT_ADDON_HOST: &str = "localhost";
const DEFAULT_ADDON_PORT: u16 = 9876;

/// Add-on probe budget. Short: it answers instantly when Blender is up, and the
/// interesting case — Blender closed — is a connection *refusal*, not a hang.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// `blender-mcp --help` budget. Generous because a cold Python start behind a
/// pipx shim is slow, but bounded so a wedged interpreter can't stall the card.
const HELP_TIMEOUT: Duration = Duration::from_secs(20);

/// The binary the user installs (`pipx install blender-mcp`, or the `mcp/`
/// package in the reference checkout).
const SERVER_BIN: &str = "blender-mcp";

/// What we know about the Blender side, one field per independently fixable fact.
///
/// Serializes itself rather than being hand-mapped in `up.rs`, following
/// `jobs::kubernetes::Preflight` — the same "one bool per fact plus an error"
/// shape, for the same reason.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlenderStatus {
    /// `blender-mcp` exists on PATH.
    pub server_found: bool,
    /// …and actually executes. False with `server_found` true is the broken-venv
    /// case, and the two need different advice, which is why they're separate.
    pub server_runnable: bool,
    /// A running Blender answered on the add-on socket.
    pub blender_reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_path: Option<String>,
    /// `host:port` actually probed, so the card can show a non-default override
    /// instead of leaving the user guessing which Blender we mean.
    pub addon_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blender_version: Option<String>,
    /// The open .blend, or `None` for an unsaved file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_count: Option<u64>,
    /// Why the server isn't usable — the CLI's own message, not our paraphrase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_error: Option<String>,
    /// Why Blender isn't reachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blender_error: Option<String>,
}

impl BlenderStatus {
    /// Both halves working. What a harness needs before a Blender-backed tool
    /// call can succeed.
    pub(crate) fn ready(&self) -> bool {
        self.server_runnable && self.blender_reachable
    }
}

/// `host:port` for the add-on socket, honouring the same env overrides the MCP
/// server reads.
fn addon_address() -> (String, u16) {
    let host = std::env::var("BLENDER_MCP_HOST").unwrap_or_else(|_| DEFAULT_ADDON_HOST.to_string());
    let port = std::env::var("BLENDER_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_ADDON_PORT);
    (host, port)
}

/// Locate `blender-mcp`. Shares the harness module's PATH walk rather than
/// adding a fourth hand-rolled one.
pub(crate) fn find_server() -> Option<PathBuf> {
    crate::local::harness::find_on_path(SERVER_BIN)
}

/// Best-effort probe of all three layers. Never errors: every failure is a field.
pub(crate) async fn detect() -> BlenderStatus {
    let (host, port) = addon_address();
    let mut status = BlenderStatus {
        addon_address: format!("{host}:{port}"),
        ..Default::default()
    };

    match find_server() {
        Some(path) => {
            status.server_found = true;
            status.server_path = Some(path.display().to_string());
            match probe_server(&path).await {
                Ok(()) => status.server_runnable = true,
                Err(e) => status.server_error = Some(e),
            }
        }
        None => {
            status.server_error = Some(format!(
                "{SERVER_BIN} not found on PATH — install it with `pipx install blender-mcp`."
            ));
        }
    }

    // Independent of the server on purpose: the socket belongs to Blender, so it
    // can be up while the server is broken, and vice versa. Reporting them
    // together would hide whichever failure came second.
    match probe_blender(&host, port).await {
        Ok(info) => {
            status.blender_reachable = true;
            status.blender_version = info.version;
            status.blend_file = info.file;
            status.object_count = info.objects;
        }
        Err(e) => status.blender_error = Some(e),
    }

    status
}

/// Does the launcher actually run? `--help` is the cheapest subcommand that
/// exercises the interpreter without starting a server or touching Blender.
///
/// The three-way match is `jobs::modal::detect`'s: a missing file, a failed
/// spawn, and a non-zero exit are different diagnoses, and the last one carries
/// the message that matters — the shim's `bad interpreter` line lands on stderr.
async fn probe_server(path: &PathBuf) -> std::result::Result<(), String> {
    let fut = tokio::process::Command::new(path)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match tokio::time::timeout(HELP_TIMEOUT, fut).await {
        Err(_) => Err(format!(
            "{SERVER_BIN} did not respond to --help within {}s",
            HELP_TIMEOUT.as_secs()
        )),
        // `NotFound` is ambiguous here and the ambiguity is the whole problem: the
        // kernel reports ENOENT both when the *launcher* is gone and when its
        // **interpreter** is — a `#!` pointing at a Python that a version upgrade
        // removed. Reporting the first would send the user looking for a file
        // that's plainly there, so disambiguate by looking.
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            if !path.exists() {
                return Err(format!("{} disappeared from PATH", path.display()));
            }
            Err(match missing_interpreter(path) {
                Some(interp) => format!(
                    "{} needs {interp}, which is not installed — `pipx reinstall blender-mcp` \
                     rebuilds it against your current Python.",
                    path.display()
                ),
                None => format!(
                    "cannot run {}: {e} — if it was installed with pipx, \
                     `pipx reinstall blender-mcp` rebuilds it.",
                    path.display()
                ),
            })
        }
        Ok(Err(e)) => Err(format!("cannot run {}: {e}", path.display())),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr
                .lines()
                .last()
                .unwrap_or("`blender-mcp --help` failed")
                .trim();
            // A known upstream packaging break, and the one a fresh install hits
            // today: blender-mcp asks for `mcp[cli]>=1.2.0` with no upper bound,
            // but mcp 2.0 removed `mcp.server.fastmcp`. The raw ImportError says
            // nothing about which pin to change, so name it.
            if stderr.contains("mcp.server.fastmcp") {
                return Err(format!(
                    "{last} — blender-mcp needs the mcp 1.x SDK (2.0 removed \
                     `mcp.server.fastmcp`) but its dependency has no upper bound. Reinstall it \
                     with `mcp[cli]<2` pinned."
                ));
            }
            Err(last.to_string())
        }
    }
}

/// The interpreter a `#!` line names, when that interpreter doesn't exist.
///
/// This is what turns an opaque ENOENT into the actual diagnosis. `Some` means the
/// script's shebang points somewhere empty — the signature of a pipx venv orphaned
/// by a Python upgrade.
fn missing_interpreter(path: &std::path::Path) -> Option<String> {
    // Only the first line is needed, but these launchers are a few hundred bytes;
    // reading the whole file is simpler than a buffered reader and no slower.
    let body = std::fs::read_to_string(path).ok()?;
    let shebang = body.lines().next()?.strip_prefix("#!")?.trim();
    // `#!/usr/bin/env python3` defers the lookup to PATH, so the literal string
    // isn't a path to test — skip rather than guess.
    let interp = shebang.split_whitespace().next()?;
    (!std::path::Path::new(interp).exists()).then(|| interp.to_string())
}

/// What the add-on told us about the live session.
#[derive(Debug)]
struct BlenderInfo {
    version: Option<String>,
    file: Option<String>,
    objects: Option<u64>,
}

/// Ask the running Blender who it is, over the add-on's socket.
///
/// The add-on protocol is a NUL-terminated JSON request whose `code` is executed
/// inside Blender, assigning a dict to `result`. **The snippet must stay
/// read-only** — this runs on every status refresh, against whatever the user has
/// open and possibly unsaved.
///
/// Raw TCP rather than going through the MCP server: this answers "is Blender
/// there" for the card, and it has to work when the server is the broken part.
async fn probe_blender(host: &str, port: u16) -> std::result::Result<BlenderInfo, String> {
    const SNIPPET: &str = "import bpy\n\
         result = {\n\
         \x20   \"version\": bpy.app.version_string,\n\
         \x20   \"file\": bpy.data.filepath,\n\
         \x20   \"objects\": len(bpy.data.objects),\n\
         }";
    let request = json!({ "type": "execute", "code": SNIPPET, "strict_json": true });
    let mut payload = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    payload.push(0);

    let body = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut sock = TcpStream::connect((host, port)).await.map_err(|e| {
            format!(
                "cannot reach Blender at {host}:{port}: {e} — open Blender and make sure the \
                 Blender MCP add-on is enabled with its server started."
            )
        })?;
        sock.write_all(&payload).await.map_err(|e| e.to_string())?;
        sock.flush().await.map_err(|e| e.to_string())?;

        // Read to the NUL delimiter. A closed connection before it means the
        // add-on hung up mid-answer, which is a failure, not an empty success.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = sock.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.contains(&0) {
                break;
            }
        }
        Ok::<Vec<u8>, String>(buf)
    })
    .await
    .map_err(|_| format!("Blender did not answer at {host}:{port} within {PROBE_TIMEOUT:?}"))??;

    parse_probe(&body)
}

/// Pull the session facts out of an add-on response.
fn parse_probe(body: &[u8]) -> std::result::Result<BlenderInfo, String> {
    let frame = body.split(|b| *b == 0).next().unwrap_or(body);
    let doc: Value = serde_json::from_slice(frame)
        .map_err(|e| format!("Blender sent a response we could not read: {e}"))?;

    // The add-on reports its own failures in-band with `status`, so a 200-shaped
    // reply can still be an error; checking only the transport would call that a
    // success.
    if doc.get("status").and_then(Value::as_str) != Some("ok") {
        let message = doc
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the Blender add-on reported an error");
        return Err(message.to_string());
    }
    let result = doc.get("result").unwrap_or(&Value::Null);
    Ok(BlenderInfo {
        version: result
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        // An unsaved file is the empty string; report it as absent rather than
        // rendering a blank row.
        file: result
            .get("file")
            .and_then(Value::as_str)
            .filter(|f| !f.is_empty())
            .map(str::to_string),
        objects: result.get("objects").and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_reads_a_live_session() {
        let body = br#"{"status":"ok","result":{"version":"5.2.0 LTS","file":"","objects":3}}"#;
        let info = parse_probe(body).unwrap();
        assert_eq!(info.version.as_deref(), Some("5.2.0 LTS"));
        assert_eq!(info.objects, Some(3));
        // Unsaved: empty string must not become a blank "File" row.
        assert_eq!(info.file, None);
    }

    /// The NUL delimiter is part of the framing, so it has to be stripped before
    /// the JSON parser sees it.
    #[test]
    fn parse_probe_ignores_the_nul_terminator_and_trailing_bytes() {
        let body = b"{\"status\":\"ok\",\"result\":{\"version\":\"5.2.0\"}}\0junk";
        assert_eq!(parse_probe(body).unwrap().version.as_deref(), Some("5.2.0"));
    }

    /// The add-on reports failures in-band with `status`, so a well-formed reply
    /// can still be an error — treating any parseable frame as success would
    /// report a broken Blender as reachable.
    #[test]
    fn parse_probe_surfaces_an_in_band_error() {
        let body = br#"{"status":"error","message":"the `result` variable must be a dict"}"#;
        let err = parse_probe(body).unwrap_err();
        assert!(err.contains("must be a dict"), "got {err}");
    }

    #[test]
    fn parse_probe_rejects_garbage() {
        assert!(parse_probe(b"not json\0").is_err());
    }

    /// The diagnosis that matters: a launcher whose `#!` interpreter is gone.
    /// Without this the kernel's ENOENT reads as "the launcher is missing", and
    /// the user goes looking for a file that is plainly there.
    #[test]
    fn missing_interpreter_names_a_dead_shebang() {
        let dir = std::env::temp_dir().join(format!("orx-blender-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("blender-mcp");
        std::fs::write(&script, "#!/nonexistent/python3.13\nimport sys\n").unwrap();
        assert_eq!(
            missing_interpreter(&script).as_deref(),
            Some("/nonexistent/python3.13")
        );

        // A shebang that resolves is not the problem, so it must stay quiet.
        let ok = dir.join("fine");
        std::fs::write(&ok, "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(missing_interpreter(&ok), None);

        // `env` defers to PATH, so the literal string isn't a path to test.
        let via_env = dir.join("via-env");
        std::fs::write(&via_env, "#!/usr/bin/env python3\n").unwrap();
        assert_eq!(missing_interpreter(&via_env), None);

        // No shebang at all: nothing to diagnose.
        let binary = dir.join("no-shebang");
        std::fs::write(&binary, "\x7fELF not really\n").unwrap();
        assert_eq!(missing_interpreter(&binary), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ready` must require both halves: a runnable server with no Blender behind
    /// it cannot answer a single Blender-backed tool call.
    #[test]
    fn ready_needs_both_the_server_and_blender() {
        let mut s = BlenderStatus {
            server_found: true,
            server_runnable: true,
            ..Default::default()
        };
        assert!(!s.ready());
        s.blender_reachable = true;
        assert!(s.ready());
        s.server_runnable = false;
        assert!(!s.ready());
    }

    /// The probed address has to follow the env overrides the MCP server itself
    /// reads, or the card would vouch for a different Blender than the tools use.
    #[test]
    fn addon_address_defaults_are_the_addons_defaults() {
        // Not asserting against the env (tests share a process); just the shape.
        let (host, port) = addon_address();
        assert!(!host.is_empty());
        assert!(port > 0);
        assert_eq!(DEFAULT_ADDON_PORT, 9876);
        assert_eq!(DEFAULT_ADDON_HOST, "localhost");
    }
}
