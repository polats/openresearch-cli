//! Token storage and the one-time interactive login for the Scenario MCP server.
//!
//! Scenario's OAuth server is Clerk, reached through dynamic client
//! registration — there is no app to pre-register and no API key to paste. The
//! flow is split the way tris-bot splits it (`.pi/extensions/scenario/`):
//!
//! - **Interactive** (`connect`): opens the user's real browser at the authorize
//!   URL and catches the redirect on a one-shot loopback listener. Runs only
//!   from `crux scenario connect` or the Settings action.
//! - **Silent** (everything else): refreshes tokens but can never open a
//!   browser. A missing or dead login surfaces as [`ScenarioError::NotConnected`]
//!   so callers can say "reconnect" instead of leaking an OAuth error.
//!
//! The two halves meet at one file, `<config>/scenario/auth.json`, mode 0600.
//! `crux up` and one-off `orx` invocations are separate processes, so writes are
//! atomic (temp + rename) and the file is re-read on every load rather than
//! cached — otherwise a refresh in one process would leave the other holding a
//! stale refresh token, which Clerk invalidates once the new one is issued.

use std::path::PathBuf;
use std::time::Duration;

use rmcp::transport::auth::{AuthError, AuthorizationManager, CredentialStore, StoredCredentials};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use super::ScenarioError;
use crate::error::{anyhow, Result};

/// Scenario's hosted MCP endpoint. Also the OAuth resource whose authorization
/// metadata the manager discovers on construction.
pub(crate) const MCP_URL: &str = "https://mcp.scenario.com/mcp";

/// Client name shown on Scenario's OAuth consent screen.
const CLIENT_NAME: &str = "Crux";

/// Loopback port the authorize redirect lands on. Fixed rather than ephemeral
/// because it is baked into the registered `redirect_uris`: Clerk rejects a
/// redirect that doesn't match the registration, so picking a fresh port per run
/// would mean re-registering the client every time. Deliberately not tris-bot's
/// 41899, so a Crux login and a tris login can't fight over the socket.
const CALLBACK_PORT: u16 = 41900;

/// How long [`connect`] waits for the user to finish logging in before giving up
/// and releasing the port.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Scopes requested at authorize time.
///
/// Hardcoded because Scenario's OAuth server publishes no `scopes_supported` to
/// discover them from, yet rejects an authorize request that omits `scope`
/// entirely — a combination that yields an opaque failure if you assume
/// discovery works. Verified against the live metadata at
/// `https://mcp.scenario.com/.well-known/oauth-authorization-server`, which
/// advertises `registration_endpoint` and `grant_types_supported:
/// [authorization_code, refresh_token]` but no `scopes_supported` at all.
///
/// `offline_access` is what earns a refresh token; without it every login would
/// die at the first access-token expiry.
const SCOPES: &[&str] = &["email", "offline_access", "profile"];

fn redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}/callback")
}

/// `<config>/scenario/auth.json` — the shared token file.
///
/// Lives in the config dir, not the data dir: the data dir is user-relocatable
/// and gets snapshotted for remote boxes, and an OAuth refresh token should not
/// travel with a copied workspace.
pub(crate) fn auth_path() -> PathBuf {
    crate::config::config_dir()
        .join("scenario")
        .join("auth.json")
}

/// File-backed [`CredentialStore`], replacing rmcp's in-memory default so a
/// login survives process exit and is visible to every other `orx`/`crux`
/// invocation on the machine.
#[derive(Debug, Default)]
pub(crate) struct FileCredentialStore;

#[async_trait::async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let path = auth_path();
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            // Not connected yet is the normal cold state, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AuthError::InternalError(format!("read {path:?}: {e}"))),
        };
        // A corrupt or half-written file reads as "not connected" rather than a
        // hard failure: the recovery is the same (log in again), and a stored
        // credential we can't parse is worth nothing either way.
        Ok(serde_json::from_str(&body).ok())
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        write_credentials(&credentials)
            .map_err(|e| AuthError::InternalError(format!("save scenario credentials: {e}")))
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        match std::fs::remove_file(auth_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::InternalError(format!("clear credentials: {e}"))),
        }
    }
}

/// Write the token file atomically at mode 0600.
///
/// Temp + rename so a concurrent reader never sees a torn file (the same reason
/// `telemetry::write_settings` does it), and 0600 set on the temp file *before*
/// the rename so the secret is never briefly world-readable.
fn write_credentials(credentials: &StoredCredentials) -> Result<()> {
    let path = auth_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("scenario auth path has no parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("cannot create {}: {e}", parent.display()))?;

    let body = serde_json::to_string_pretty(credentials)
        .map_err(|e| anyhow!("cannot serialize scenario credentials: {e}"))?;
    let tmp = parent.join(format!(".auth.json.{}.tmp", uuid::Uuid::new_v4()));

    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, format!("{body}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("cannot write {}: {e}", path.display()));
    }
    Ok(())
}

/// What discovery found, for `crux scenario status`.
///
/// Worth surfacing separately from "connected": a reachable-but-unauthenticated
/// state and a broken-discovery state look identical from the outside, and only
/// the second one means the login *can't* work. Checking it costs one request and
/// needs no credentials.
pub(crate) struct Discovery {
    pub authorize: String,
    pub token: String,
    pub registration: Option<String>,
    pub source: String,
}

/// Probe Scenario's OAuth metadata without touching stored credentials.
pub(crate) async fn discover() -> Result<Discovery> {
    let manager = AuthorizationManager::new(MCP_URL)
        .await
        .map_err(|e| anyhow!("cannot reach {MCP_URL}: {e}"))?;
    let resolved = manager
        .resolve_metadata()
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok(Discovery {
        authorize: resolved.metadata.authorization_endpoint.clone(),
        token: resolved.metadata.token_endpoint.clone(),
        registration: resolved.metadata.registration_endpoint.clone(),
        source: format!("{:?}", resolved.source),
    })
}

/// True when a token file exists and parses. Cheap enough for a status probe —
/// it says nothing about whether the token still works, only that a login was
/// completed at some point.
pub(crate) fn has_stored_login() -> bool {
    std::fs::read_to_string(auth_path())
        .ok()
        .and_then(|b| serde_json::from_str::<StoredCredentials>(&b).ok())
        .is_some()
}

/// Forget the stored login. Used by `crux scenario disconnect`.
pub(crate) fn forget() -> Result<()> {
    match std::fs::remove_file(auth_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("cannot remove {}: {e}", auth_path().display())),
    }
}

/// An authorization manager wired to the shared token file, with any stored
/// login already restored.
///
/// The `bool` is whether credentials were found: callers that must not start an
/// interactive flow use it to fail fast with [`ScenarioError::NotConnected`].
pub(crate) async fn manager() -> Result<(AuthorizationManager, bool)> {
    let mut manager = AuthorizationManager::new(MCP_URL)
        .await
        .map_err(|e| anyhow!("cannot reach Scenario's authorization metadata: {e}"))?;

    // `new` only builds the manager — it discovers nothing. Without an explicit
    // resolve+set, `metadata` stays `None` and `register_client` fails with the
    // unhelpful "No authorization support detected".
    //
    // Scenario resolves via RFC 9728 protected-resource metadata: an
    // unauthenticated request answers 401 with
    // `WWW-Authenticate: Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp"`,
    // and that document names Clerk as the authorization server. Note the
    // suffixed path — the unsuffixed `/.well-known/oauth-protected-resource`
    // serves the marketing site's HTML with a 404, so a hand-rolled probe of the
    // obvious URL would conclude there's no OAuth support at all.
    let resolved = manager
        .resolve_metadata()
        .await
        .map_err(|e| anyhow!("cannot discover Scenario's OAuth endpoints: {e}"))?;

    // `resolve_metadata` never fails outright: when nothing is discoverable it
    // synthesizes `/authorize`, `/token`, `/register` off the base URL for
    // pre-2025-06 servers. Accepting that here would send a login to endpoints
    // Scenario never advertised and fail later with something far less legible,
    // so treat the fallback as the error it is.
    if !resolved.source.is_discovered() {
        return Err(anyhow!(
            "Scenario published no OAuth metadata at {MCP_URL} — endpoints would have to be \
             guessed, so refusing. The service may be down or its discovery paths may have moved."
        ));
    }
    manager.set_metadata(resolved.metadata);

    manager.set_credential_store(FileCredentialStore);
    let restored = manager
        .initialize_from_store()
        .await
        .map_err(|e| anyhow!("cannot restore the stored Scenario login: {e}"))?;
    Ok((manager, restored))
}

/// A manager with a working login, or [`ScenarioError::NotConnected`].
///
/// This is the silent path: it will refresh an expired access token but never
/// opens a browser, so a caller inside `crux up` can't hang a request on a login
/// nobody is watching for.
pub(crate) async fn silent_manager() -> std::result::Result<AuthorizationManager, ScenarioError> {
    if !has_stored_login() {
        return Err(ScenarioError::NotConnected);
    }
    let (manager, restored) = manager().await.map_err(ScenarioError::Other)?;
    if !restored {
        return Err(ScenarioError::NotConnected);
    }
    Ok(manager)
}

/// Run the interactive login and persist the result.
///
/// Registers a client (Scenario supports dynamic registration, so there is
/// nothing to configure), opens the browser at the authorize URL, and waits for
/// the redirect on [`CALLBACK_PORT`]. Returns the granted scopes.
pub(crate) async fn connect() -> Result<Vec<String>> {
    let (mut manager, _) = manager().await?;

    // Bind before opening the browser: if the port is busy — a stale login still
    // holding it, or another tool on the same port — say so now rather than
    // sending the user to a page whose redirect can't land.
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .map_err(|e| {
            anyhow!(
                "cannot listen on 127.0.0.1:{CALLBACK_PORT} for the Scenario login redirect: {e}\n\
                 Another login may still be in progress — wait a moment and retry."
            )
        })?;

    let redirect = redirect_uri();
    manager
        .register_client(CLIENT_NAME, &redirect, SCOPES)
        .await
        .map_err(|e| anyhow!("Scenario rejected the client registration: {e}"))?;

    let url = manager
        .get_authorization_url(SCOPES)
        .await
        .map_err(|e| anyhow!("cannot build the Scenario authorize URL: {e}"))?;

    // CSRF token is echoed back as `state`; the exchange needs it to find the
    // PKCE verifier the manager stashed for this attempt.
    let csrf = query_param(&url, "state")
        .ok_or_else(|| anyhow!("Scenario authorize URL carried no state parameter"))?;

    eprintln!("Opening your browser to sign in to Scenario…");
    eprintln!("If it doesn't open, visit:\n  {url}");
    crate::browser::open_browser(&url);

    let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, &csrf))
        .await
        .map_err(|_| anyhow!("timed out after 5 minutes waiting for the Scenario login"))??;

    manager
        .exchange_code_for_token(&code, &csrf)
        .await
        .map_err(|e| anyhow!("Scenario refused the authorization code: {e}"))?;

    // `exchange_code_for_token` persists through the credential store, so the
    // token file exists by now; read the granted scopes back for the caller.
    Ok(manager.get_current_scopes().await)
}

/// Accept exactly one `/callback` hit and return its `code`.
///
/// Hand-rolled HTTP rather than mounting this on axum: the listener must exist
/// for one login and then release the fixed port, and it has to work from a
/// plain `orx scenario connect` where no server is running.
async fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut sock, _) = listener
            .accept()
            .await
            .map_err(|e| anyhow!("Scenario login callback failed: {e}"))?;

        let mut reader = BufReader::new(&mut sock);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await.is_err() {
            continue;
        }
        // "GET /callback?code=…&state=… HTTP/1.1"
        let target = request_line.split_whitespace().nth(1).unwrap_or("");
        if !target.starts_with("/callback") {
            respond(&mut sock, "404 Not Found", "Not found.").await;
            continue;
        }
        let full = format!("http://localhost:{CALLBACK_PORT}{target}");
        let state = query_param(&full, "state");
        let code = query_param(&full, "code");

        // A stale browser tab from an earlier attempt replays an old state; that
        // code is worthless and exchanging it would fail confusingly. Reject and
        // keep listening for the real redirect.
        if state.as_deref() != Some(expected_state) {
            respond(
                &mut sock,
                "400 Bad Request",
                "Login failed: state mismatch (a stale login tab?). Close this and run `crux scenario connect` again.",
            )
            .await;
            continue;
        }
        match code {
            Some(code) => {
                respond(
                    &mut sock,
                    "200 OK",
                    "Scenario connected — you can close this tab and return to Crux.",
                )
                .await;
                return Ok(code);
            }
            None => {
                let reason = query_param(&full, "error_description")
                    .or_else(|| query_param(&full, "error"))
                    .unwrap_or_else(|| "no code returned".to_string());
                respond(
                    &mut sock,
                    "400 Bad Request",
                    &format!("Login failed: {reason}"),
                )
                .await;
                return Err(anyhow!("Scenario login failed: {reason}"));
            }
        }
    }
}

/// Minimal HTML response. Best-effort: the user's browser seeing a broken page
/// must not fail a login whose code we already hold.
async fn respond(sock: &mut tokio::net::TcpStream, status: &str, message: &str) {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><h2>{message}</h2>");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(response.as_bytes()).await;
    let _ = sock.flush().await;
}

/// Pull a single query parameter out of a URL, percent-decoded.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| {
            urlencoding::decode(v)
                .unwrap_or(std::borrow::Cow::Borrowed(v))
                .into_owned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_decodes_and_selects() {
        let url = "http://localhost:41900/callback?code=abc%20123&state=xyz";
        assert_eq!(query_param(url, "code").as_deref(), Some("abc 123"));
        assert_eq!(query_param(url, "state").as_deref(), Some("xyz"));
        assert_eq!(query_param(url, "missing"), None);
    }

    /// A parameter whose name is a suffix of another must not match: `state`
    /// should never be answered by `expected_state`.
    #[test]
    fn query_param_matches_whole_names_only() {
        let url = "http://x/callback?expected_state=no&state=yes";
        assert_eq!(query_param(url, "state").as_deref(), Some("yes"));
    }

    #[test]
    fn query_param_without_query_is_none() {
        assert_eq!(query_param("http://localhost/callback", "code"), None);
    }

    /// The redirect URI is part of the client registration, so it has to match
    /// the port we actually bind — a drift here fails only at login time.
    #[test]
    fn redirect_uri_matches_the_callback_port() {
        assert_eq!(
            redirect_uri(),
            format!("http://localhost:{CALLBACK_PORT}/callback")
        );
        assert!(redirect_uri().ends_with("/callback"));
    }

    /// `offline_access` is what earns a refresh token; losing it would make
    /// every login expire within the hour.
    #[test]
    fn scopes_request_offline_access() {
        assert!(SCOPES.contains(&"offline_access"));
    }
}
