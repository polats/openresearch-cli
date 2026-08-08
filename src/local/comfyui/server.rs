//! The crux-managed `comfyui-mcp` child, plus ComfyUI's own lifecycle.
//!
//! Two children with deliberately different policies:
//!
//! - **`comfyui-mcp`** starts eagerly with `orx up`, like `blender-mcp`, because
//!   harness children are configured once at spawn and then live for many turns —
//!   the URL has to be valid before any harness starts. It tolerates ComfyUI being
//!   down, failing per call.
//! - **ComfyUI itself** is *adopted* when something already answers on its port,
//!   and otherwise started only when asked. It is a GPU server that loads models
//!   and holds VRAM; launching one unbidden on every dashboard start would be a
//!   rude default, and the user often has their own running already.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::Tool;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use tokio::process::{Child, Command};

use crate::error::{anyhow, Result};
use crate::store;

/// The name ComfyUI's tools appear under in an agent's namespace
/// (`mcp__comfyui__*`).
pub(crate) const SERVER_NAME: &str = "comfyui";

/// How long to wait for the MCP server to bind.
const MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait for ComfyUI to answer after we start it. Generous: it imports
/// torch and scans models, which is tens of seconds on a warm cache and worse on a
/// cold one.
const COMFY_STARTUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Watchdog cadence, matching the Blender server's.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);

/// Per-call ceiling for crux's own MCP calls.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn mcp_log_path() -> PathBuf {
    store::data_dir().join("comfyui-mcp.log")
}

pub(crate) fn comfy_log_path() -> PathBuf {
    store::data_dir().join("comfyui.log")
}

/// A running `comfyui-mcp`. Cheap to clone; the child sits behind a mutex so the
/// watchdog can replace it without invalidating anyone's handle.
#[derive(Clone)]
pub(crate) struct ComfyMcpServer {
    inner: Arc<Inner>,
}

struct Inner {
    bin: PathBuf,
    port: u16,
    child: tokio::sync::Mutex<Child>,
}

impl ComfyMcpServer {
    /// Spawn the MCP server and wait for it to bind.
    pub(crate) async fn start(bin: PathBuf) -> Result<Self> {
        let port = free_port()?;
        let child = spawn_mcp(&bin, port).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                bin,
                port,
                child: tokio::sync::Mutex::new(child),
            }),
        })
    }

    /// The endpoint harnesses and our own client connect to.
    pub(crate) fn url(&self) -> String {
        url_for(self.inner.port)
    }

    pub(crate) fn port(&self) -> u16 {
        self.inner.port
    }

    /// Restart the child if it exited. Returns whether a restart happened.
    async fn ensure_alive(&self) -> Result<bool> {
        let mut guard = self.inner.child.lock().await;
        match guard.try_wait() {
            Ok(None) => Ok(false),
            Ok(Some(status)) => {
                eprintln!(
                    "orx up: comfyui-mcp exited ({status}); restarting on port {}",
                    self.inner.port
                );
                *guard = spawn_mcp(&self.inner.bin, self.inner.port).await?;
                Ok(true)
            }
            Err(e) => Err(anyhow!("cannot check on the comfyui-mcp child: {e}")),
        }
    }

    /// The tools the server advertises.
    ///
    /// **Polled, never hard-coded.** `comfyui-mcp` is a third-party package under
    /// active development; a list baked into crux would be stale within a release,
    /// and the card would then lie about what the agent can do.
    pub(crate) async fn list_tools(&self) -> Result<Vec<Tool>> {
        let session = self.connect().await?;
        let tools = session
            .list_all_tools()
            .await
            .map_err(|e| anyhow!("cannot list ComfyUI MCP tools: {e}"));
        let _ = session.cancel().await;
        tools
    }

    async fn connect(&self) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
        // `#[non_exhaustive]`, so built then mutated rather than a struct literal.
        let config = StreamableHttpClientTransportConfig::with_uri(self.url());
        let transport = StreamableHttpClientTransport::from_config(config);
        tokio::time::timeout(CALL_TIMEOUT, ().serve(transport))
            .await
            .map_err(|_| {
                anyhow!(
                    "comfyui-mcp did not answer on {} within {}s",
                    self.url(),
                    CALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| anyhow!("cannot open a ComfyUI MCP session: {e}"))
    }
}

/// The MCP endpoint for a given port.
///
/// Note the `/mcp` path: `comfyui-mcp` serves there, unlike `blender-mcp` which
/// serves at `/`. Getting it wrong 404s every tool call. Loopback is hardcoded —
/// this endpoint can drive arbitrary workflows and download models.
fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| anyhow!("could not pick a port for comfyui-mcp: {e}"))?;
    Ok(listener.local_addr()?.port())
}

/// Spawn `comfyui-mcp --http` and wait for it to bind.
///
/// Deliberately **without** `--token`, even though this server offers one and
/// `blender-mcp` doesn't. The token has to reach three harnesses to be usable, and
/// they take it three ways — Claude a literal header, OpenCode a header map, Codex
/// only the *name* of an env var holding it. Adding it without that plumbing is
/// what a first attempt here did, and every harness silently got 401 while crux's
/// own client (which does send it) reported a healthy 40 tools.
///
/// It would also guard little: the endpoint is bound to loopback, and the token
/// would sit in a plaintext config inside the session worktree. The bind is the
/// boundary. If this ever needs a token, plumb the header through
/// `local::mcp_servers` for all three harnesses in the same change.
async fn spawn_mcp(bin: &PathBuf, port: u16) -> Result<Child> {
    let log = open_log(&mcp_log_path())?;
    let mut cmd = Command::new(bin);
    cmd.arg("--http")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        // Pin the install explicitly. Without this the server picks one by its own
        // search order, which on a machine with two checkouts is a coin flip — and
        // it would then be describing a different ComfyUI than the card does.
        .env("COMFYUI_PATH", super::install_path())
        // Numeric, not the dashboard's `localhost`: this is the MCP server's own
        // programmatic connection, and `localhost` would try `::1` first against a
        // server bound to 127.0.0.1.
        .env("COMFYUI_URL", super::api_base())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| anyhow!("{e}"))?,
        ))
        .stderr(std::process::Stdio::from(log))
        .kill_on_drop(true);
    for (key, value) in crate::config::list_synced_env() {
        if std::env::var_os(&key).is_none() {
            cmd.env(key, value);
        }
    }
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("could not spawn {}: {e}", bin.display()))?;
    if let Err(e) = wait_listening(&mut child, port, MCP_STARTUP_TIMEOUT, "comfyui-mcp").await {
        let _ = child.kill().await;
        return Err(e);
    }
    Ok(child)
}

fn open_log(path: &std::path::Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow!("could not open {}: {e}", path.display()))
}

/// Wait until something listens on `port`, watching for an early exit.
async fn wait_listening(child: &mut Child, port: u16, budget: Duration, what: &str) -> Result<()> {
    let deadline = Instant::now() + budget;
    loop {
        // Checked first: a broken install exits immediately, and without this the
        // failure would present as a timeout minutes later.
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "{what} exited during startup ({status}); see {}",
                if what == "comfyui-mcp" {
                    mcp_log_path()
                } else {
                    comfy_log_path()
                }
                .display()
            ));
        }
        if tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .is_ok_and(|r| r.is_ok())
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "{what} did not listen on 127.0.0.1:{port} within {}s",
                budget.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

// --- ComfyUI's own process ------------------------------------------------------

/// The ComfyUI child, when crux started one. `None` means either nothing is
/// running or the user's own instance was adopted — and the card distinguishes
/// those, because "we started it" decides whether stopping it is ours to offer.
fn comfy_child() -> &'static tokio::sync::Mutex<Option<Child>> {
    static CHILD: std::sync::OnceLock<tokio::sync::Mutex<Option<Child>>> =
        std::sync::OnceLock::new();
    CHILD.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Whether the running ComfyUI is one crux started.
pub(crate) fn comfy_is_managed() -> bool {
    comfy_child()
        .try_lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Is something listening on ComfyUI's port?
pub(crate) async fn comfy_is_listening() -> bool {
    let (host, port) = super::address();
    tokio::time::timeout(
        Duration::from_millis(700),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// Start ComfyUI and wait for it to answer, unless something already is.
///
/// Adopt-then-start: if the port answers we take it as the user's own instance and
/// return without spawning, which is both what they expect and the only safe move
/// — two ComfyUI processes on one port would leave the second dead and the first
/// mysteriously in charge.
pub(crate) async fn start_comfy() -> Result<bool> {
    if comfy_is_listening().await {
        return Ok(false);
    }
    let mut guard = comfy_child().lock().await;
    // Re-check under the lock: two Start presses can race here.
    if let Some(child) = guard.as_mut() {
        if child.try_wait()?.is_none() {
            return Ok(false);
        }
    }

    let dir = super::install_path();
    let python = comfy_python(&dir)?;
    let (host, port) = super::address();
    let log = open_log(&comfy_log_path())?;

    let mut cmd = Command::new(&python);
    cmd.arg("main.py")
        .arg("--listen")
        .arg(&host)
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| anyhow!("{e}"))?,
        ))
        .stderr(std::process::Stdio::from(log))
        // Dies with `orx up`: a GPU process holding VRAM must not outlive the
        // dashboard that started it.
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn().map_err(|e| {
        anyhow!(
            "could not start ComfyUI with {} in {}: {e}",
            python.display(),
            dir.display()
        )
    })?;
    if let Err(e) = wait_listening(&mut child, port, COMFY_STARTUP_TIMEOUT, "ComfyUI").await {
        let _ = child.kill().await;
        return Err(e);
    }
    eprintln!("orx up: started ComfyUI on {}", super::dashboard_url());
    *guard = Some(child);
    Ok(true)
}

/// The interpreter to run ComfyUI with: its own venv if present, else `python3`.
///
/// The venv matters — ComfyUI's dependencies (torch, and every custom node's
/// compiled extensions) live there, and the system interpreter would fail on the
/// first import.
fn comfy_python(dir: &std::path::Path) -> Result<PathBuf> {
    if !dir.join("main.py").is_file() {
        return Err(anyhow!(
            "no ComfyUI install at {} (no main.py) — set COMFYUI_PATH to your install.",
            dir.display()
        ));
    }
    for rel in ["venv/bin/python", ".venv/bin/python"] {
        let candidate = dir.join(rel);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    crate::local::harness::find_on_path("python3").ok_or_else(|| {
        anyhow!(
            "no interpreter for ComfyUI: {} has no venv/bin/python and python3 is not on PATH",
            dir.display()
        )
    })
}

/// Start the MCP server if it's installed and runnable, else explain why not.
pub(crate) async fn start_if_available() -> Option<ComfyMcpServer> {
    let status = super::detect().await;
    if !status.mcp_runnable {
        // Silent on "not installed"; a *broken* install gets a line, since that
        // one looks like it should work.
        if status.mcp_found {
            if let Some(e) = &status.mcp_error {
                eprintln!("orx up: comfyui-mcp found but not runnable: {e}");
            }
        }
        return None;
    }
    let bin = super::find_mcp()?;
    match ComfyMcpServer::start(bin).await {
        Ok(server) => {
            eprintln!("orx up: comfyui-mcp on 127.0.0.1:{}", server.port());
            crate::local::mcp_servers::register(SERVER_NAME, server.url());
            Some(server)
        }
        Err(e) => {
            eprintln!("orx up: could not start comfyui-mcp: {e}");
            None
        }
    }
}

/// Keep the MCP child alive for the server's lifetime.
pub(crate) fn spawn_watchdog(server: ComfyMcpServer) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_INTERVAL).await;
            if let Err(e) = server.ensure_alive().await {
                eprintln!("orx up: could not restart comfyui-mcp: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `comfyui-mcp` serves `/mcp`, not `/` like `blender-mcp`. The two providers
    /// having different endpoint paths is exactly the kind of detail that silently
    /// 404s every tool call, so it is pinned.
    #[test]
    fn url_uses_the_mcp_path() {
        assert_eq!(url_for(9100), "http://127.0.0.1:9100/mcp");
        assert!(url_for(9100).ends_with("/mcp"));
        assert!(url_for(9100).starts_with("http://127.0.0.1:"));
    }

    /// Regression guard for the bug that end-to-end testing caught: the server is
    /// started with no `--token`, because a token that only crux sends leaves every
    /// harness on 401 while the card still reports a healthy tool count. Re-adding
    /// it means plumbing the header through `local::mcp_servers` at the same time.
    #[test]
    fn the_mcp_server_is_started_without_a_token() {
        let src = include_str!("server.rs");
        let spawn = src
            .split("async fn spawn_mcp")
            .nth(1)
            .expect("spawn_mcp exists");
        let body = spawn.split("async fn ").next().unwrap_or(spawn);
        assert!(
            !body.contains("\"--token\""),
            "spawn_mcp passes --token; harness configs must carry it too"
        );
    }

    /// A directory with no `main.py` is not an install, and saying so beats a
    /// spawn failure from the venv probe.
    #[test]
    fn comfy_python_rejects_a_non_install() {
        let dir = std::env::temp_dir().join(format!("orx-comfy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = comfy_python(&dir).unwrap_err().to_string();
        assert!(err.contains("no ComfyUI install"), "got {err}");
        assert!(err.contains("COMFYUI_PATH"), "the fix is not named: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The venv interpreter wins over PATH: torch and every compiled custom-node
    /// extension live there, so the system python would die on first import.
    #[test]
    fn comfy_python_prefers_the_venv() {
        let dir = std::env::temp_dir().join(format!("orx-comfy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("venv/bin")).unwrap();
        std::fs::write(dir.join("main.py"), "").unwrap();
        std::fs::write(dir.join("venv/bin/python"), "").unwrap();
        assert_eq!(comfy_python(&dir).unwrap(), dir.join("venv/bin/python"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Placeholder for URL/token tests that must not spawn anything real.
    fn placeholder_child() -> Child {
        Command::new("/bin/true")
            .stdin(std::process::Stdio::null())
            .spawn()
            .expect("spawn /bin/true")
    }
}
