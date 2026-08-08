//! The managed local MCP servers offered to every harness.
//!
//! Crux runs a small set of MCP servers as child processes (Blender, ComfyUI) and
//! hands their URLs to Claude, Codex and OpenCode at spawn. Each harness takes
//! them in a different shape — Claude a JSON `mcpServers` map, Codex a `-c` TOML
//! override, OpenCode an `mcp` key in the config crux authors — so without a
//! single list, adding one server means editing three spawn paths and it is easy
//! for them to drift out of sync.
//!
//! This is that list. A new provider registers here and all three harnesses pick
//! it up.
//!
//! Registration is a process-global set once at startup, not a value threaded
//! through the spawn chains: the three harnesses are plumbed completely
//! differently (Claude's spawn takes a `ChatHost`, Codex's takes neither,
//! OpenCode's config writer runs on a blocking task) and all three want the same
//! startup fact. See the ordering note in `commands::up::run` — every server must
//! be registered before anything can spawn a harness, because a harness child is
//! configured once and then lives for many turns.

use std::sync::{Mutex, OnceLock};

/// One managed server: the name the agent sees its tools under, and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpServer {
    /// Tool prefix in the agent's namespace, e.g. `blender` → `mcp__blender__*`.
    pub name: &'static str,
    pub url: String,
}

fn registry() -> &'static Mutex<Vec<McpServer>> {
    static REGISTRY: OnceLock<Mutex<Vec<McpServer>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Offer a server's tools to every harness spawned from now on.
///
/// Idempotent per name: re-registering replaces the URL rather than adding a
/// duplicate, so a server that restarts on a different port can update itself
/// without the agent seeing two entries.
pub(crate) fn register(name: &'static str, url: String) {
    let mut servers = registry().lock().unwrap();
    match servers.iter_mut().find(|s| s.name == name) {
        Some(existing) => existing.url = url,
        None => servers.push(McpServer { name, url }),
    }
}

/// Every managed server, for a harness spawn to inject.
///
/// Empty is the normal state on a machine with none of the providers installed —
/// harnesses then get no extra MCP config at all, which is deliberate: a
/// configured server that nothing answers shows up as a broken tool in the
/// agent's list, and that is worse than its absence.
pub(crate) fn all() -> Vec<McpServer> {
    registry().lock().unwrap().clone()
}

/// The `mcpServers` entries for Claude's `--mcp-config`.
pub(crate) fn claude_entries() -> Vec<(&'static str, serde_json::Value)> {
    all()
        .into_iter()
        .map(|s| (s.name, serde_json::json!({ "type": "http", "url": s.url })))
        .collect()
}

/// Codex `-c` overrides, one per server.
///
/// Dotted paths on purpose: each sets a single key *inside* `mcp_servers`, so the
/// user's own servers in `~/.codex/config.toml` survive — verified, and the
/// reason crux still never writes that file. (Contrast the title path's
/// `-c mcp_servers={}`, which clears the whole table deliberately.)
pub(crate) fn codex_overrides() -> Vec<String> {
    all()
        .into_iter()
        .map(|s| format!("mcp_servers.{}={{url=\"{}\"}}", s.name, s.url))
        .collect()
}

/// OpenCode's `mcp` config object, or `None` when there are no servers (so the
/// key is omitted rather than written empty).
pub(crate) fn opencode_config() -> Option<serde_json::Value> {
    let servers = all();
    if servers.is_empty() {
        return None;
    }
    let mut map = serde_json::Map::new();
    for s in servers {
        map.insert(
            s.name.to_string(),
            serde_json::json!({ "type": "remote", "url": s.url, "enabled": true }),
        );
    }
    Some(serde_json::Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process-global, so these tests take turns and reset it.
    fn claim() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        registry().lock().unwrap().clear();
        guard
    }

    /// No servers means no config for any harness — an absent entry beats one
    /// pointing at nothing.
    #[test]
    fn empty_registry_yields_no_config() {
        let _g = claim();
        assert!(claude_entries().is_empty());
        assert!(codex_overrides().is_empty());
        assert!(opencode_config().is_none());
    }

    /// Each harness's own shape, from one registration.
    #[test]
    fn one_server_renders_in_all_three_shapes() {
        let _g = claim();
        register("blender", "http://127.0.0.1:41000/".to_string());

        let claude = claude_entries();
        assert_eq!(claude.len(), 1);
        assert_eq!(claude[0].0, "blender");
        assert_eq!(claude[0].1["type"], "http");
        assert_eq!(claude[0].1["url"], "http://127.0.0.1:41000/");

        assert_eq!(
            codex_overrides(),
            vec![r#"mcp_servers.blender={url="http://127.0.0.1:41000/"}"#]
        );

        let oc = opencode_config().unwrap();
        assert_eq!(oc["blender"]["type"], "remote");
        assert_eq!(oc["blender"]["enabled"], true);
        assert_eq!(oc["blender"]["url"], "http://127.0.0.1:41000/");
    }

    /// Two providers must both reach every harness — the whole point of the list.
    #[test]
    fn several_servers_all_reach_every_harness() {
        let _g = claim();
        register("blender", "http://127.0.0.1:41000/".to_string());
        register("comfyui", "http://127.0.0.1:9100/mcp".to_string());

        assert_eq!(claude_entries().len(), 2);
        assert_eq!(codex_overrides().len(), 2);
        let oc = opencode_config().unwrap();
        assert!(oc.get("blender").is_some() && oc.get("comfyui").is_some());
    }

    /// A server that restarts on a new port updates in place; the agent must not
    /// end up with two entries under one name.
    #[test]
    fn re_registering_replaces_the_url() {
        let _g = claim();
        register("comfyui", "http://127.0.0.1:9100/mcp".to_string());
        register("comfyui", "http://127.0.0.1:9200/mcp".to_string());

        let all = all();
        assert_eq!(all.len(), 1, "duplicate entry for one name");
        assert_eq!(all[0].url, "http://127.0.0.1:9200/mcp");
    }

    /// The URL is quoted into TOML, so a server whose path differs from the
    /// others (ComfyUI serves `/mcp`, Blender serves `/`) must survive intact.
    #[test]
    fn codex_override_preserves_the_endpoint_path() {
        let _g = claim();
        register("comfyui", "http://127.0.0.1:9100/mcp".to_string());
        let over = codex_overrides().remove(0);
        assert!(over.contains("/mcp\""), "path lost: {over}");
        assert!(over.starts_with("mcp_servers.comfyui="));
    }
}
