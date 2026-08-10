//! UniRig — automatic skeleton + skin weights for a mesh, as a local HTTP service.
//!
//! **No MCP harness, deliberately.** The rule the sprite pipeline settled on is
//! "MCP harness = stateful service with a lifecycle; plain HTTP/CLI = stateless
//! transform". UniRig ships as a container exposing a FastAPI endpoint, so crux can
//! `POST /rig` directly; wrapping that in a managed MCP server would add a process
//! to supervise and answer no question the container doesn't already answer. What
//! is still wanted is the *status card* — "is it up, and where" — which is what this
//! module provides.
//!
//! The probe is layered like `blender::detect`, for the same reason: "no container"
//! and "container up but not answering" need different advice.

use serde::Serialize;

/// Default published port of the `unirig:local` container (8081 → 8080 inside).
const DEFAULT_PORT: u16 = 8081;
const DEFAULT_HOST: &str = "127.0.0.1";

/// What we know about the UniRig side, one field per independently fixable fact.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnirigStatus {
    /// Something is listening on the rig port.
    pub port_open: bool,
    /// …and it answers as the UniRig API rather than some other service that
    /// happens to hold the port. Separate from `port_open` because a stale
    /// container or an unrelated process both look "open" from a TCP probe.
    pub service_reachable: bool,
    /// `host:port` actually probed, so the card names the service it means.
    pub address: String,
    /// The rig endpoint, for the card to show and for callers to POST to.
    pub rig_url: String,
    /// FastAPI's interactive docs, when present — the quickest way to see the
    /// real request shape without reading our code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UnirigStatus {
    /// Usable for a rig call.
    pub fn ready(&self) -> bool {
        self.service_reachable
    }
}

/// `host:port`, honouring an override so a remote or re-published container can be
/// pointed at without a rebuild.
fn address() -> (String, u16) {
    let host = std::env::var("UNIRIG_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port = std::env::var("UNIRIG_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    (host, port)
}

pub async fn detect() -> UnirigStatus {
    let (host, port) = address();
    let base = format!("http://{host}:{port}");
    let mut status = UnirigStatus {
        address: format!("{host}:{port}"),
        rig_url: format!("{base}/rig"),
        ..Default::default()
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            status.error = Some(e.to_string());
            return status;
        }
    };

    // `/docs` rather than `/` — the app's root is a 404 by design, so it cannot
    // distinguish "UniRig is up" from "nothing is there". FastAPI serves `/docs`,
    // which both proves the port is open AND that it is a FastAPI app.
    match client.get(format!("{base}/docs")).send().await {
        Ok(resp) => {
            status.port_open = true;
            if resp.status().is_success() {
                status.service_reachable = true;
                status.docs_url = Some(format!("{base}/docs"));
            } else {
                status.error = Some(format!(
                    "something answered on {host}:{port} but not the UniRig API (HTTP {})",
                    resp.status().as_u16()
                ));
            }
        }
        Err(e) => {
            status.error = Some(if e.is_connect() {
                format!(
                    "nothing listening on {host}:{port} — start the container with \
                     `docker start unirig` (published as {port}→8080)."
                )
            } else {
                e.to_string()
            });
        }
    }
    status
}
