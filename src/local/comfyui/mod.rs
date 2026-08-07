//! ComfyUI as a generative-AI provider, reached through the `comfyui-mcp` server.
//!
//! Four moving parts, reported separately because their fixes differ:
//!
//! 1. **ComfyUI itself** — a Python server on `127.0.0.1:8188`. Crux can adopt one
//!    the user already started, or start it; see [`server`].
//! 2. **`comfyui-mcp`** — a Node CLI the user installs globally. Present on PATH
//!    is not the same as runnable.
//! 3. **The MCP server process** crux manages, in [`server`].
//! 4. **The loaded node set.** This is the layer with no analogue in the Blender
//!    integration and the one worth having: a custom-node pack can serve workflow
//!    *templates* while its node classes failed to import, so the templates look
//!    available and every submission fails with an unknown node type. Found on
//!    this machine — `ComfyUI-Trellis2` serves 15 templates with all 14 of its
//!    classes missing, because a compiled dependency was built against a
//!    different torch. Nothing in ComfyUI's UI says so.
//!
//! Unlike Blender there is **no official local MCP server**: Comfy's first-party
//! one drives your own install via `comfy-cli` but is in private test, and their
//! hosted Cloud MCP is explicitly not for self-hosted instances. So this uses the
//! community `comfyui-mcp`, whose tool surface we **poll** rather than hard-code —
//! it is a third-party dependency that will add and rename tools, and a list
//! baked in here would be wrong within a release.

pub(crate) mod server;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// Where ComfyUI listens by default — the address we bind and connect to.
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8188;

/// The host used only in the **browser-facing** dashboard link.
///
/// Deliberately different from [`DEFAULT_HOST`], because these two jobs have
/// different constraints and one name cannot serve both:
///
/// * A browser hitting `http://127.0.0.1:8188` can be refused where `localhost`
///   works — extensions and private-network policies treat a bare loopback IP as a
///   different, less trusted origin. Reported on this machine.
/// * Our own probes must NOT use `localhost`: it resolves to `::1` first on most
///   systems, and ComfyUI binds `127.0.0.1` only, so the connection would be
///   refused. `address()` therefore stays numeric.
///
/// An explicit `COMFYUI_HOST` overrides both — if the user names a host, that is
/// the host in every context.
const DASHBOARD_HOST: &str = "localhost";

/// Default install location. Overridable with `COMFYUI_PATH` — deliberately the
/// same variable `comfyui-mcp` itself reads, so one setting points both at the
/// same install. It matters here: this machine has two ComfyUI checkouts, and the
/// MCP server picks one by its own search order if nothing says otherwise.
const PATH_ENV: &str = "COMFYUI_PATH";

/// The `comfyui-mcp` CLI. A Node package, so a global npm install.
const MCP_BIN: &str = "comfyui-mcp";

/// HTTP probe budget. ComfyUI answers `/system_stats` immediately when up; the
/// interesting case (not running) is a connection refusal, not a hang.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// `comfyui-mcp --help` budget. Node cold starts are slow.
const HELP_TIMEOUT: Duration = Duration::from_secs(30);

/// What we know about the ComfyUI side, one field per independently fixable fact.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComfyStatus {
    /// `comfyui-mcp` exists on PATH.
    pub mcp_found: bool,
    /// …and actually executes.
    pub mcp_runnable: bool,
    /// ComfyUI answered `/system_stats`.
    pub comfy_reachable: bool,
    /// The install crux points at (`COMFYUI_PATH` or the default).
    pub install_path: String,
    /// The **browsable** URL for ComfyUI's own web UI, which the card links to.
    /// Uses a hostname rather than the loopback IP our probes use — see
    /// [`DASHBOARD_HOST`].
    pub dashboard_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comfy_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torch_version: Option<String>,
    /// First CUDA/accelerator device name, for the card's one-line hardware row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_free: Option<u64>,
    /// Registered node classes — the size of the catalog, not its contents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_classes: Option<usize>,
    /// Jobs running plus pending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<usize>,
    /// Custom-node packs whose templates reference classes that never registered.
    /// Empty is healthy; non-empty means those workflows cannot run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub broken_packs: Vec<BrokenPack>,
    /// Whether crux started ComfyUI (as opposed to adopting the user's).
    pub comfy_managed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comfy_error: Option<String>,
}

/// A custom-node pack that serves templates it cannot execute.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrokenPack {
    pub pack: String,
    /// How many of its templates are affected.
    pub templates: usize,
    /// The node classes that failed to register. Truncated — the point is which
    /// pack to reinstall, not an exhaustive list.
    pub missing_nodes: Vec<String>,
}

impl ComfyStatus {
    /// Everything a tool call needs: a runnable MCP server and a live ComfyUI.
    pub(crate) fn ready(&self) -> bool {
        self.mcp_runnable && self.comfy_reachable
    }
}

/// The install directory crux points at.
pub(crate) fn install_path() -> PathBuf {
    if let Some(p) = std::env::var_os(PATH_ENV) {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .map(|h| h.join("ComfyUI"))
        .unwrap_or_else(|| PathBuf::from("ComfyUI"))
}

/// `host:port` ComfyUI is expected on.
pub(crate) fn address() -> (String, u16) {
    let host = std::env::var("COMFYUI_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port = std::env::var("COMFYUI_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    (host, port)
}

/// The base URL for our own HTTP calls to ComfyUI. Always numeric — see
/// [`DASHBOARD_HOST`] for why this must not be `localhost`.
pub(crate) fn api_base() -> String {
    let (host, port) = address();
    format!("http://{host}:{port}")
}

/// The URL to open in a browser — ComfyUI's own web UI, which the card links to.
///
/// Uses `localhost` rather than the loopback IP: some browsers refuse
/// `http://127.0.0.1:<port>` as a distinct, less-trusted origin.
pub(crate) fn dashboard_url() -> String {
    let (_, port) = address();
    let host = std::env::var("COMFYUI_HOST").unwrap_or_else(|_| DASHBOARD_HOST.to_string());
    format!("http://{host}:{port}")
}

/// Locate `comfyui-mcp`, sharing the harness module's PATH walk.
pub(crate) fn find_mcp() -> Option<PathBuf> {
    crate::local::harness::find_on_path(MCP_BIN)
}

/// Best-effort probe of every layer. Never errors: each failure is a field.
pub(crate) async fn detect() -> ComfyStatus {
    let mut status = ComfyStatus {
        install_path: install_path().display().to_string(),
        dashboard_url: dashboard_url(),
        comfy_managed: server::comfy_is_managed(),
        ..Default::default()
    };

    match find_mcp() {
        Some(path) => {
            status.mcp_found = true;
            status.mcp_path = Some(path.display().to_string());
            match probe_mcp(&path).await {
                Ok(()) => status.mcp_runnable = true,
                Err(e) => status.mcp_error = Some(e),
            }
        }
        None => {
            status.mcp_error = Some(format!(
                "{MCP_BIN} not found on PATH — install it with `npm install -g {MCP_BIN}`."
            ));
        }
    }

    // Independent of the MCP server: ComfyUI is the user's process (or ours), and
    // either side can be broken while the other works.
    match probe_comfy().await {
        Ok(info) => {
            status.comfy_reachable = true;
            status.comfy_version = info.version;
            status.python_version = info.python;
            status.torch_version = info.torch;
            status.device = info.device;
            status.vram_total = info.vram_total;
            status.vram_free = info.vram_free;
            status.node_classes = info.node_classes;
            status.queue_depth = info.queue_depth;
            status.broken_packs = info.broken_packs;
        }
        Err(e) => status.comfy_error = Some(e),
    }

    status
}

/// Does the launcher run? `--help` exercises Node and the package without
/// starting a server or touching ComfyUI.
async fn probe_mcp(path: &PathBuf) -> std::result::Result<(), String> {
    let fut = tokio::process::Command::new(path)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match tokio::time::timeout(HELP_TIMEOUT, fut).await {
        Err(_) => Err(format!(
            "{MCP_BIN} did not respond to --help within {}s",
            HELP_TIMEOUT.as_secs()
        )),
        // Same ambiguity as the Blender probe: ENOENT covers both a missing
        // launcher and a missing interpreter, so check which before blaming PATH.
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            if !path.exists() {
                return Err(format!("{} disappeared from PATH", path.display()));
            }
            Err(format!(
                "cannot run {}: {e} — its node interpreter may be missing; \
                 `npm install -g {MCP_BIN}` reinstalls it.",
                path.display()
            ))
        }
        Ok(Err(e)) => Err(format!("cannot run {}: {e}", path.display())),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("`comfyui-mcp --help` failed")
            .trim()
            .to_string()),
    }
}

/// What ComfyUI told us about itself.
#[derive(Default)]
struct ComfyInfo {
    version: Option<String>,
    python: Option<String>,
    torch: Option<String>,
    device: Option<String>,
    vram_total: Option<u64>,
    vram_free: Option<u64>,
    node_classes: Option<usize>,
    queue_depth: Option<usize>,
    broken_packs: Vec<BrokenPack>,
}

/// Ask ComfyUI's own HTTP API, not the MCP server.
///
/// Deliberately direct: this has to answer "is ComfyUI there" even when the MCP
/// server is the broken part, and it is one cheap request either way.
async fn probe_comfy() -> std::result::Result<ComfyInfo, String> {
    // Numeric, not the dashboard's `localhost`: that would resolve to `::1` first
    // and be refused by a server bound to 127.0.0.1.
    let base = api_base();
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;

    let stats: Value = client
        .get(format!("{base}/system_stats"))
        .send()
        .await
        .map_err(|e| {
            format!(
                "cannot reach ComfyUI at {base}: {e} — start it from this card, or run it \
                 yourself with `python main.py`."
            )
        })?
        .json()
        .await
        .map_err(|e| format!("ComfyUI sent a /system_stats body we could not read: {e}"))?;

    let sys = stats.get("system").cloned().unwrap_or(Value::Null);
    let mut info = ComfyInfo {
        version: str_field(&sys, "comfyui_version"),
        python: str_field(&sys, "python_version"),
        torch: str_field(&sys, "pytorch_version"),
        ..Default::default()
    };
    if let Some(dev) = stats
        .get("devices")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
    {
        info.device = str_field(dev, "name");
        info.vram_total = dev.get("vram_total").and_then(Value::as_u64);
        info.vram_free = dev.get("vram_free").and_then(Value::as_u64);
    }

    // The rest is best-effort enrichment: a reachable ComfyUI is already the
    // headline, and a hiccup on a detail row must not turn it into "unreachable".
    if let Ok(queue) = get_json(&client, &format!("{base}/queue")).await {
        let count = |k: &str| {
            queue
                .get(k)
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        };
        info.queue_depth = Some(count("queue_running") + count("queue_pending"));
    }

    // `/object_info` is ~1.4 MB on a normal install, so it is fetched for two
    // aggregate facts and never handed onward: the class count, and which
    // template-serving packs reference classes that never registered.
    if let Ok(object_info) = get_json(&client, &format!("{base}/object_info")).await {
        if let Some(map) = object_info.as_object() {
            info.node_classes = Some(map.len());
            let registered: BTreeSet<&str> = map.keys().map(String::as_str).collect();
            if let Ok(templates) =
                get_json(&client, &format!("{base}/api/workflow_templates")).await
            {
                info.broken_packs = broken_packs(&templates, &registered);
            }
        }
    }

    Ok(info)
}

async fn get_json(client: &reqwest::Client, url: &str) -> std::result::Result<Value, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Which template-serving packs reference node classes that never registered.
///
/// Templates are read from the pack's own directory on disk, because
/// `/api/workflow_templates` lists only names. A pack whose directory we cannot
/// read is skipped rather than guessed at: absence of evidence is not a fault.
fn broken_packs(templates: &Value, registered: &BTreeSet<&str>) -> Vec<BrokenPack> {
    let Some(packs) = templates.as_object() else {
        return Vec::new();
    };
    let root = install_path().join("custom_nodes");
    let mut out = Vec::new();

    for (pack, names) in packs {
        let names: Vec<&str> = names
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut missing = BTreeSet::new();
        let mut affected = 0usize;

        for name in &names {
            let Some(types) = template_node_types(&root, pack, name) else {
                continue;
            };
            let gaps: Vec<String> = types
                .into_iter()
                .filter(|t| !registered.contains(t.as_str()))
                .collect();
            if !gaps.is_empty() {
                affected += 1;
                missing.extend(gaps);
            }
        }
        if affected > 0 {
            out.push(BrokenPack {
                pack: pack.clone(),
                templates: affected,
                missing_nodes: missing.into_iter().take(12).collect(),
            });
        }
    }
    out
}

/// The node types a pack's bundled template references.
fn template_node_types(root: &std::path::Path, pack: &str, name: &str) -> Option<BTreeSet<String>> {
    // Packs bundle templates under a couple of conventional directory names.
    let candidates = ["example_workflows", "templates", "workflows"];
    let body = candidates.iter().find_map(|dir| {
        std::fs::read_to_string(root.join(pack).join(dir).join(format!("{name}.json"))).ok()
    })?;
    let doc: Value = serde_json::from_str(&body).ok()?;
    Some(node_types(&doc))
}

/// Node `type` values in a workflow document, in either of ComfyUI's two shapes:
/// the UI graph (`{nodes: [{type}]}`) and the API prompt (`{"3": {class_type}}`).
fn node_types(doc: &Value) -> BTreeSet<String> {
    if let Some(nodes) = doc.get("nodes").and_then(Value::as_array) {
        return nodes.iter().filter_map(|n| str_field(n, "type")).collect();
    }
    doc.as_object()
        .map(|m| {
            m.values()
                .filter_map(|v| str_field(v, "class_type"))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_types_reads_the_ui_graph_shape() {
        let doc = serde_json::json!({
            "nodes": [{ "type": "KSampler" }, { "type": "Trellis2LoadModel" }, { "id": 3 }]
        });
        let types = node_types(&doc);
        assert!(types.contains("KSampler") && types.contains("Trellis2LoadModel"));
        assert_eq!(types.len(), 2, "a node without a type must be skipped");
    }

    /// The API prompt format is the other thing a `.json` in a pack might be, and
    /// reading it as the UI shape would silently find no nodes — reporting a
    /// broken pack as healthy.
    #[test]
    fn node_types_reads_the_api_prompt_shape() {
        let doc = serde_json::json!({
            "3": { "class_type": "KSampler", "inputs": {} },
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": {} }
        });
        let types = node_types(&doc);
        assert_eq!(types.len(), 2);
        assert!(types.contains("CheckpointLoaderSimple"));
    }

    /// A pack with every class registered must not be reported. False positives
    /// here would train the user to ignore the row.
    #[test]
    fn a_healthy_pack_is_not_reported() {
        let registered: BTreeSet<&str> = ["KSampler"].into_iter().collect();
        let doc = serde_json::json!({ "nodes": [{ "type": "KSampler" }] });
        let types = node_types(&doc);
        let missing: Vec<&String> = types
            .iter()
            .filter(|t| !registered.contains(t.as_str()))
            .collect();
        assert!(missing.is_empty());
    }

    /// `ready` needs both halves: a runnable MCP server with no ComfyUI behind it
    /// cannot answer a single generation call.
    #[test]
    fn ready_needs_both_the_mcp_server_and_comfyui() {
        let mut s = ComfyStatus {
            mcp_found: true,
            mcp_runnable: true,
            ..Default::default()
        };
        assert!(!s.ready());
        s.comfy_reachable = true;
        assert!(s.ready());
        s.mcp_runnable = false;
        assert!(!s.ready());
    }

    /// The dashboard link is the card's one outbound action, so it has to be a
    /// real absolute URL and follow the port override.
    #[test]
    fn dashboard_url_is_absolute_and_follows_the_port() {
        let url = dashboard_url();
        assert!(url.starts_with("http://"), "not absolute: {url}");
        let (_, port) = address();
        assert!(url.ends_with(&format!(":{port}")), "port missing: {url}");
    }

    /// The browsable URL uses a hostname and our own calls use the loopback IP.
    /// Both halves matter: some browsers refuse `http://127.0.0.1:<port>` as a
    /// less-trusted origin, while `localhost` resolves to `::1` first and would be
    /// refused by a server bound to 127.0.0.1. Collapsing them breaks one or other.
    #[test]
    fn the_browse_url_and_the_api_base_use_different_hosts() {
        // Only meaningful without an explicit override, which wins for both.
        if std::env::var_os("COMFYUI_HOST").is_some() {
            return;
        }
        assert!(
            dashboard_url().contains("localhost"),
            "the browser link must not be a bare loopback IP: {}",
            dashboard_url()
        );
        assert!(
            api_base().contains("127.0.0.1"),
            "our own calls must stay numeric: {}",
            api_base()
        );
        assert_ne!(dashboard_url(), api_base());
    }

    /// `COMFYUI_PATH` is the same variable `comfyui-mcp` reads. This machine has
    /// two ComfyUI checkouts, so a default that silently disagreed with the MCP
    /// server's own pick would point the card at the wrong install.
    #[test]
    fn install_path_is_overridable() {
        assert_eq!(PATH_ENV, "COMFYUI_PATH");
        assert!(install_path().is_absolute() || !install_path().as_os_str().is_empty());
    }
}
