//! The crux-managed `blender-mcp` child, and crux's own client for it.
//!
//! One server process shared by every harness and by crux itself, run in
//! streamable-HTTP mode rather than the stdio mode an MCP client would normally
//! spawn per-client. Two reasons:
//!
//! - **Harness children are configured at spawn and then live for many turns.**
//!   Claude's resident child gets its `--mcp-config` once. So the URL has to be
//!   valid *before* any harness starts and stay valid for that child's whole
//!   life — which means one long-lived server on a port fixed at startup, not a
//!   process per client.
//! - **Crux needs its own access** for the Generative AI card's tool count today,
//!   and for workflow nodes later. Over HTTP that is the same `rmcp` client the
//!   Scenario bridge already uses, minus the auth.
//!
//! The child deliberately outlives any individual Blender session: the MCP server
//! reconnects to Blender per tool call, so closing and reopening Blender needs
//! nothing from us. See the [module docs](super) for that protocol detail.

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

/// How long to wait for the server to bind its port.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the watchdog checks the child is still alive. Matches the cadence of
/// `up`'s stale-play-session reaper; the harness configs point at this URL for a
/// child's whole life, so a dead server has to come back on its own.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);

/// Per-call ceiling for crux's own MCP calls. Well under Scenario's 240s: nothing
/// here is a generation job, and a hung Blender should surface quickly.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the child's stdout/stderr land, so a startup failure is diagnosable.
pub(crate) fn log_path() -> PathBuf {
    store::data_dir().join("blender-mcp.log")
}

/// The managed server's URL, for the harness spawn paths.
///
/// A process global rather than a value threaded through three call chains: the
/// three harnesses are plumbed completely differently (Claude's spawn takes a
/// `ChatHost`, Codex's takes neither, OpenCode's config writer runs on a blocking
/// task), yet all three want the same single startup fact. Set once, before
/// anything can spawn a harness — see the ordering note in `commands::up::run`.
static HARNESS_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The URL to give harnesses, or `None` when no server started.
///
/// `None` means the harness gets **no Blender entry at all**, which is deliberate:
/// a configured MCP server that nothing answers shows up as a broken tool in the
/// agent's list, which is worse than its absence.
pub(crate) fn harness_url() -> Option<&'static str> {
    HARNESS_URL.get().map(String::as_str)
}

/// The `-c` value that registers the managed server with Codex, or `None` when
/// there is no server.
///
/// A **dotted path** on purpose: it sets one key *inside* `mcp_servers` and leaves
/// the rest of the table alone, so the user's own Codex MCP servers survive.
/// Verified — with a `config.toml` declaring a server, `-c
/// mcp_servers.blender={…}` lists both. (Contrast the title path's
/// `-c mcp_servers={}`, which replaces the whole table, deliberately, to boot none
/// of them for a one-line request.)
///
/// The value is TOML, and the URL is ours (`http://127.0.0.1:<port>/`), so it needs
/// no escaping beyond the quotes.
pub(crate) fn codex_config_override() -> Option<String> {
    harness_url().map(|url| format!("mcp_servers.blender={{url=\"{url}\"}}"))
}

/// A running `blender-mcp`. Cheap to clone; the child sits behind a mutex so the
/// watchdog can replace it without invalidating anyone's handle.
#[derive(Clone)]
pub(crate) struct BlenderServer {
    inner: Arc<Inner>,
}

struct Inner {
    bin: PathBuf,
    port: u16,
    child: tokio::sync::Mutex<Child>,
}

impl BlenderServer {
    /// Spawn the server and wait for it to bind.
    ///
    /// The port is chosen once, here, and never changes — see the module docs for
    /// why it has to be stable.
    pub(crate) async fn start(bin: PathBuf) -> Result<Self> {
        let port = free_port()?;
        let child = spawn(&bin, port).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                bin,
                port,
                child: tokio::sync::Mutex::new(child),
            }),
        })
    }

    /// The URL harnesses and our own client connect to.
    pub(crate) fn url(&self) -> String {
        url_for(self.inner.port)
    }

    pub(crate) fn port(&self) -> u16 {
        self.inner.port
    }

    /// Restart the child if it has exited. Returns whether a restart happened.
    ///
    /// Called by the watchdog rather than by request handlers: a handler that
    /// respawned would make an ordinary status poll pay a 30s startup wait.
    async fn ensure_alive(&self) -> Result<bool> {
        let mut guard = self.inner.child.lock().await;
        match guard.try_wait() {
            // Still running.
            Ok(None) => Ok(false),
            Ok(Some(status)) => {
                eprintln!(
                    "orx up: blender-mcp exited ({status}); restarting on port {}",
                    self.inner.port
                );
                *guard = spawn(&self.inner.bin, self.inner.port).await?;
                Ok(true)
            }
            Err(e) => Err(anyhow!("cannot check on the blender-mcp child: {e}")),
        }
    }

    /// The tools the server advertises.
    ///
    /// Works with Blender closed: roughly three of the twenty are pure
    /// documentation search and need no Blender at all, and `tools/list` itself
    /// never touches the socket. So this is a fact about the *server*, which is
    /// what the card wants to report.
    pub(crate) async fn list_tools(&self) -> Result<Vec<Tool>> {
        let session = self.connect().await?;
        let tools = session
            .list_all_tools()
            .await
            .map_err(|e| anyhow!("cannot list Blender MCP tools: {e}"));
        let _ = session.cancel().await;
        tools
    }

    /// One MCP session against the managed server.
    ///
    /// A fresh session per call, as with Scenario: the calls are short and a
    /// cached session that died mid-flight fails the *next* call with something
    /// that reads as a protocol error rather than a restart.
    async fn connect(&self) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
        // `#[non_exhaustive]`, so built then mutated rather than a struct literal.
        let config = StreamableHttpClientTransportConfig::with_uri(self.url());
        let transport = StreamableHttpClientTransport::from_config(config);
        tokio::time::timeout(CALL_TIMEOUT, ().serve(transport))
            .await
            .map_err(|_| {
                anyhow!(
                    "blender-mcp did not answer on {} within {}s",
                    self.url(),
                    CALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| anyhow!("cannot open a Blender MCP session: {e}"))
    }
}

/// The endpoint for a given port.
///
/// The trailing slash is the server's `streamable_http_path`, set to `/` in
/// `blmcp/__init__.py` — *not* the SDK default of `/mcp`. Loopback is hardcoded on
/// purpose: this server executes arbitrary Python inside the user's Blender, so it
/// must not be reachable off-box even when the dashboard is widened with `--host`.
fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// Ask the OS for a free loopback port (bind :0, read it back, release). Same
/// trick as `local::opencode::free_port`.
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| anyhow!("could not pick a port for blender-mcp: {e}"))?;
    Ok(listener.local_addr()?.port())
}

/// Spawn `blender-mcp --transport http` and wait for it to bind.
async fn spawn(bin: &PathBuf, port: u16) -> Result<Child> {
    // The data dir may not exist yet on a fresh machine.
    if let Some(parent) = log_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .map_err(|e| anyhow!("could not open {}: {e}", log_path().display()))?;

    let mut cmd = Command::new(bin);
    cmd.arg("--transport")
        .arg("http")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| anyhow!("{e}"))?,
        ))
        .stderr(std::process::Stdio::from(log))
        // Dies with `orx up` when the runtime drops the handle (Ctrl-C, exit).
        .kill_on_drop(true);
    // Vars from the dashboard's Environment tab reach it too, so
    // BLENDER_MCP_HOST/PORT can be set there rather than only in the shell that
    // launched orx. The real process env still wins.
    for (key, value) in crate::config::list_synced_env() {
        if std::env::var_os(&key).is_none() {
            cmd.env(key, value);
        }
    }
    // Own process group: a terminal SIGINT reaches orx up alone, which then tears
    // this child down deliberately via kill_on_drop.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("could not spawn {}: {e}", bin.display()))?;
    if let Err(e) = wait_listening(&mut child, port).await {
        let _ = child.kill().await;
        return Err(e);
    }
    Ok(child)
}

/// Wait until something is listening on the port, watching for an early exit.
///
/// A TCP connect rather than an HTTP request: the endpoint is a streamable-HTTP
/// MCP transport, which answers a bare probing GET with a 4xx — so "bound" is the
/// honest readiness signal, and a handshake would only add ways to be wrong.
async fn wait_listening(child: &mut Child, port: u16) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        // Checked first, and this is the branch that fires for a broken install:
        // a pipx shim with a missing interpreter exits immediately, and without
        // this the failure would present as a 30s timeout.
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "blender-mcp exited during startup ({status}); see {}",
                log_path().display()
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
                "blender-mcp did not listen on 127.0.0.1:{port} within {}s; see {}",
                STARTUP_TIMEOUT.as_secs(),
                log_path().display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Start the server if it's installed and runnable, else explain why not.
///
/// Returns `None` rather than an error when the server simply isn't usable:
/// Blender is optional, so a machine without it must not make `orx up` noisy.
/// The card reports the same facts in a place the user is actually looking.
pub(crate) async fn start_if_available() -> Option<BlenderServer> {
    let status = super::detect().await;
    if !status.server_runnable {
        // Silent on the common "not installed" case; only a *broken* install gets
        // a line, since that one looks like it should work.
        if status.server_found {
            if let Some(e) = &status.server_error {
                eprintln!("orx up: blender-mcp found but not runnable: {e}");
            }
        }
        return None;
    }
    let bin = super::find_server()?;
    match BlenderServer::start(bin).await {
        Ok(server) => {
            eprintln!("orx up: blender-mcp on 127.0.0.1:{}", server.port());
            // Publish before returning: the caller wires the watchdog and the
            // router next, and any harness spawn after that must see the URL.
            let _ = HARNESS_URL.set(server.url());
            Some(server)
        }
        Err(e) => {
            eprintln!("orx up: could not start blender-mcp: {e}");
            None
        }
    }
}

/// Keep the child alive for the server's lifetime.
pub(crate) fn spawn_watchdog(server: BlenderServer) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_INTERVAL).await;
            if let Err(e) = server.ensure_alive().await {
                eprintln!("orx up: could not restart blender-mcp: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL must end in `/`, not `/mcp`: the server sets
    /// `streamable_http_path = "/"`, overriding the SDK default. Getting this
    /// wrong would 404 every harness tool call.
    #[test]
    fn url_uses_the_servers_root_streamable_path() {
        assert_eq!(url_for(41234), "http://127.0.0.1:41234/");
        assert!(url_for(41234).ends_with('/'));
        assert!(!url_for(41234).ends_with("/mcp"));
    }

    /// Loopback only, whatever `--host` the dashboard was given: this server
    /// executes arbitrary Python inside the user's Blender.
    #[test]
    fn url_is_loopback_only() {
        assert!(url_for(1234).starts_with("http://127.0.0.1:"));
    }

    /// The port must be usable — a free port is what the harness configs are
    /// built from, so a zero or unbindable port would poison every config.
    #[test]
    fn free_port_returns_a_usable_port() {
        let port = free_port().expect("a free loopback port");
        assert!(port > 0);
        // Released again, so the child can bind it.
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }
}
