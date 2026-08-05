//! Scenario MCP client.
//!
//! Scenario's asset platform is reached over MCP (`https://mcp.scenario.com/mcp`,
//! 60+ tools, 550+ models) rather than its REST API, so nobody has to hold an API
//! key: authentication is OAuth against the user's own Scenario SSO. See
//! [`auth`] for the login split.
//!
//! This module is the *silent* half — everything that runs inside `crux up` or a
//! plain command. It refreshes tokens but never opens a browser, so a request
//! can't block on a login nobody is watching. A dead or missing login is
//! [`ScenarioError::NotConnected`], which the UI renders as "reconnect" instead
//! of an OAuth stack trace.
//!
//! Why a fresh connection per call rather than a cached one: an MCP session
//! holds server state, and a session that dies (token expiry, server restart,
//! the machine sleeping) fails the *next* call with something that looks like a
//! protocol error rather than an auth error. tris-bot caches a client and evicts
//! it on any failure; we skip the cache entirely until there's a measured reason
//! to add it, because generation calls are seconds-to-minutes long and dominated
//! by the work, not the handshake.

pub(crate) mod auth;

use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{anyhow, Error};

/// Per-call ceiling. Generous because Scenario generation tools do real work
/// (a video job can take minutes) — but bounded, so a wedged call surfaces as an
/// error rather than holding a dashboard request open forever.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// What went wrong talking to Scenario.
///
/// The split exists so exactly one condition — "the user must log in again" —
/// is distinguishable everywhere it matters: the CLI prints a `connect` hint,
/// the HTTP layer answers 401, and the Settings card offers Reconnect.
#[derive(Debug)]
pub(crate) enum ScenarioError {
    /// No stored login, or the stored one no longer works.
    NotConnected,
    /// Anything else: transport, protocol, or a tool reporting failure.
    Other(Error),
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScenarioError::NotConnected => write!(
                f,
                "Not connected to Scenario — run `crux scenario connect`, then retry."
            ),
            ScenarioError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl From<ScenarioError> for Error {
    fn from(e: ScenarioError) -> Error {
        match e {
            ScenarioError::NotConnected => anyhow!("{e}"),
            ScenarioError::Other(inner) => inner,
        }
    }
}

pub(crate) type ScenarioResult<T> = std::result::Result<T, ScenarioError>;

/// True when an error text looks like an authorization failure.
///
/// Needed in two places because MCP reports failure two different ways: a
/// transport error can be thrown, *or* a tool can answer normally with
/// `isError` and the reason only in its text content. Checking one and not the
/// other is how an expired login ends up displayed as a generic tool failure.
fn looks_like_auth_failure(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("invalid_token")
        || lower.contains("invalid token")
}

/// Open an authenticated MCP session.
///
/// Resolves the bearer token up front and hands it to the transport as
/// `auth_header`, rather than wrapping an HTTP client in rmcp's `AuthClient`.
/// Two reasons: `AuthClient` is generic over rmcp's own `reqwest` (0.13) while
/// crux is on 0.12, and those are distinct types that don't unify — and
/// `from_config` builds the transport's client internally, so this file never has
/// to name a `reqwest` version at all. Refresh still happens, inside
/// `get_access_token`; it just happens here, once per session, instead of
/// per-request. Fine for sessions that live for a single call.
async fn connect_session() -> ScenarioResult<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let manager = auth::silent_manager().await?;
    let token = manager.get_access_token().await.map_err(|e| {
        let text = e.to_string();
        // A refresh that fails is a dead login, not a transport fault: Clerk
        // invalidates a refresh token once it's been redeemed, so a token file
        // that lost the race is unrecoverable without a new login.
        if looks_like_auth_failure(&text) || text.contains("refresh") {
            ScenarioError::NotConnected
        } else {
            ScenarioError::Other(anyhow!("cannot get a Scenario access token: {text}"))
        }
    })?;

    // Both rmcp config structs are `#[non_exhaustive]`, so they're built and
    // then mutated rather than constructed with a struct literal.
    let mut config = StreamableHttpClientTransportConfig::with_uri(auth::MCP_URL);
    // The BARE token, with no "Bearer " prefix. Despite the field name, rmcp
    // passes this to reqwest's `bearer_auth`, which adds the scheme itself —
    // prefixing here sends `Authorization: Bearer Bearer <token>` and every
    // request 401s with an unhelpful "Auth required".
    config.auth_header = Some(token);
    let transport = StreamableHttpClientTransport::from_config(config);
    ().serve(transport).await.map_err(|e| {
        let text = e.to_string();
        if looks_like_auth_failure(&text) {
            ScenarioError::NotConnected
        } else {
            ScenarioError::Other(anyhow!("cannot open a Scenario MCP session: {text}"))
        }
    })
}

/// The tool list the connected workspace advertises.
///
/// Scenario documents "60+ tools" without publishing their names, so this is how
/// you find out what's actually callable — the exact names for generation and
/// usage reporting come from here, not from documentation.
pub(crate) async fn list_tools() -> ScenarioResult<Vec<Tool>> {
    let session = connect_session().await?;
    let tools = session.list_all_tools().await.map_err(|e| {
        let text = e.to_string();
        if looks_like_auth_failure(&text) {
            ScenarioError::NotConnected
        } else {
            ScenarioError::Other(anyhow!("cannot list Scenario tools: {text}"))
        }
    });
    let _ = session.cancel().await;
    tools
}

/// Call a Scenario tool and return its raw result.
pub(crate) async fn call_raw(name: &str, args: Value) -> ScenarioResult<CallToolResult> {
    let arguments = match args {
        Value::Null => None,
        Value::Object(map) => Some(map),
        other => {
            return Err(ScenarioError::Other(anyhow!(
                "Scenario tool arguments must be a JSON object, got {other}"
            )))
        }
    };

    let session = connect_session().await?;
    let mut params = CallToolRequestParams::new(name.to_string());
    if let Some(arguments) = arguments {
        params = params.with_arguments(arguments);
    }
    let called = tokio::time::timeout(CALL_TIMEOUT, session.call_tool(params)).await;
    let _ = session.cancel().await;

    let result = match called {
        Err(_) => {
            return Err(ScenarioError::Other(anyhow!(
                "Scenario tool `{name}` did not answer within {}s",
                CALL_TIMEOUT.as_secs()
            )))
        }
        Ok(Err(e)) => {
            let text = e.to_string();
            return Err(if looks_like_auth_failure(&text) {
                ScenarioError::NotConnected
            } else {
                ScenarioError::Other(anyhow!("Scenario tool `{name}` failed: {text}"))
            });
        }
        Ok(Ok(result)) => result,
    };

    if result.is_error.unwrap_or(false) {
        let text = result_text(&result);
        return Err(if looks_like_auth_failure(&text) {
            ScenarioError::NotConnected
        } else {
            // Tool errors can be long; keep the message readable.
            let brief: String = text.chars().take(500).collect();
            ScenarioError::Other(anyhow!(
                "Scenario tool `{name}` reported an error: {}",
                if brief.is_empty() {
                    "no detail given".to_string()
                } else {
                    brief
                }
            ))
        });
    }
    Ok(result)
}

/// Call a Scenario tool and deserialize its payload.
///
/// Prefers `structuredContent` when the server provides it and falls back to
/// parsing the joined text blocks. Both paths are live: newer MCP servers return
/// structured JSON, older tools only stuff JSON into a text block, and Scenario's
/// 60+ tools are not guaranteed to be uniform.
pub(crate) async fn call<T: DeserializeOwned>(name: &str, args: Value) -> ScenarioResult<T> {
    let result = call_raw(name, args).await?;
    if let Some(structured) = result.structured_content.clone() {
        return serde_json::from_value(structured).map_err(|e| {
            ScenarioError::Other(anyhow!(
                "Scenario tool `{name}` returned unexpected structured content: {e}"
            ))
        });
    }
    let text = result_text(&result);
    if text.trim().is_empty() {
        return Err(ScenarioError::Other(anyhow!(
            "Scenario tool `{name}` returned no content"
        )));
    }
    serde_json::from_str(&text).map_err(|e| {
        let brief: String = text.chars().take(200).collect();
        ScenarioError::Other(anyhow!(
            "Scenario tool `{name}` returned non-JSON ({e}): {brief}"
        ))
    })
}

/// Join a result's text blocks, dropping non-text content (images and resources
/// carry no message for us).
fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a login is stored. Says nothing about whether it still works — a
/// probe that answers without a network round trip.
pub(crate) fn is_connected() -> bool {
    auth::has_stored_login()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failures_are_recognized_in_either_shape() {
        // Thrown transport errors tend to carry the status code…
        assert!(looks_like_auth_failure("HTTP status 401 Unauthorized"));
        // …while tool-reported errors carry prose or an OAuth error code.
        assert!(looks_like_auth_failure("Unauthorized: token expired"));
        assert!(looks_like_auth_failure("invalid_token"));
        assert!(looks_like_auth_failure("Invalid Token"));
    }

    #[test]
    fn ordinary_failures_are_not_mistaken_for_auth() {
        assert!(!looks_like_auth_failure("model not found"));
        assert!(!looks_like_auth_failure(
            "HTTP status 500 Internal Server Error"
        ));
        // 404 shares no substring with the auth markers.
        assert!(!looks_like_auth_failure("404 Not Found"));
    }

    /// The distinction the whole error enum exists for: `NotConnected` must
    /// carry the reconnect instruction, since that text reaches the user.
    #[test]
    fn not_connected_tells_the_user_what_to_run() {
        let msg = ScenarioError::NotConnected.to_string();
        assert!(msg.contains("crux scenario connect"), "got: {msg}");
    }

    #[test]
    fn other_errors_pass_their_message_through() {
        let e = ScenarioError::Other(anyhow!("model not found"));
        assert_eq!(e.to_string(), "model not found");
    }

    #[test]
    fn result_text_joins_text_blocks_and_ignores_the_rest() {
        let result = CallToolResult::success(vec![
            ContentBlock::text("first"),
            ContentBlock::text("second"),
        ]);
        assert_eq!(result_text(&result), "first\nsecond");
    }
}
