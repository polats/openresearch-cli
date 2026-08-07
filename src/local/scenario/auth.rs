//! Token storage and the one-time interactive login for the Scenario MCP server.
//!
//! Scenario's OAuth server is Clerk, reached through dynamic client
//! registration — there is no app to pre-register and no API key to paste. The
//! flow is split the way tris-bot splits it (`.pi/extensions/scenario/`):
//!
//! - **Interactive** ([`begin`] + [`Login::finish`]): opens the user's real
//!   browser at the authorize URL and catches the redirect. Runs only from
//!   `crux scenario connect` or the dashboard's Settings action.
//! - **Silent** (everything else): refreshes tokens but can never open a
//!   browser. A missing or dead login surfaces as [`ScenarioError::NotConnected`]
//!   so callers can say "reconnect" instead of leaking an OAuth error.
//!
//! The two halves meet at one file, `<config>/scenario/auth.json`, mode 0600.
//! `crux up` and one-off `orx` invocations are separate processes, so writes are
//! atomic (temp + rename) and the file is re-read on every load rather than
//! cached — otherwise a refresh in one process would leave the other holding a
//! stale refresh token, which Clerk invalidates once the new one is issued.
//!
//! The interactive half is itself split by *where the redirect lands* — see
//! [`Redirect`]. The CLI owns a private loopback port because it must work with
//! no server running; the dashboard uses its own route so the remote modes work.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, CredentialStore, OAuthClientConfig, StoredCredentials,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::ScenarioError;
use crate::error::{anyhow, Result};

/// Scenario's hosted MCP endpoint. Also the OAuth resource whose authorization
/// metadata the manager discovers on construction.
pub(crate) const MCP_URL: &str = "https://mcp.scenario.com/mcp";

/// Client name shown on Scenario's OAuth consent screen.
const CLIENT_NAME: &str = "Crux";

/// Loopback port the authorize redirect lands on for the CLI flow. Fixed rather
/// than ephemeral because it is baked into the registered `redirect_uris`: Clerk
/// rejects a redirect that doesn't match the registration, so picking a fresh
/// port per run would mean re-registering the client every time. Deliberately
/// not tris-bot's 41899, so a Crux login and a tris login can't fight over the
/// socket.
const CALLBACK_PORT: u16 = 41900;

/// Route the dashboard mounts for the authorize redirect. Under `/api/` so it
/// lands with the rest of the server's surface instead of shadowing a UI path.
pub(crate) const DASHBOARD_CALLBACK_PATH: &str = "/api/scenario/callback";

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

/// Where the authorize redirect should land.
///
/// This is not a cosmetic choice: the URI is part of the dynamic client
/// registration, so each variant needs its own registered client, and the two
/// variants have genuinely different reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Redirect {
    /// A listener bound for this one login on [`CALLBACK_PORT`]. The CLI path:
    /// `crux scenario connect` has to work with no server running, and it's the
    /// escape hatch when the dashboard is on another machine.
    Loopback,
    /// The running dashboard's own route.
    ///
    /// Required for the remote modes rather than merely tidier: `crux up
    /// --remote` SSH-forwards `--port` to the user's laptop, so a callback on
    /// that port reaches this server through the tunnel that's already there.
    /// `localhost:41900` typed into that same browser would resolve to the
    /// laptop, where nothing is listening.
    Dashboard { port: u16 },
}

impl Redirect {
    pub(crate) fn uri(&self) -> String {
        match self {
            Redirect::Loopback => format!("http://localhost:{CALLBACK_PORT}/callback"),
            Redirect::Dashboard { port } => {
                format!("http://localhost:{port}{DASHBOARD_CALLBACK_PATH}")
            }
        }
    }
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

/// `<config>/scenario/client.json` — which OAuth client we registered, and the
/// redirect it was registered for.
///
/// Separate from the token file because rmcp owns that one: [`StoredCredentials`]
/// carries the `client_id` but has no room for the redirect, and rmcp rewrites
/// the whole file on every token refresh.
fn client_path() -> PathBuf {
    crate::config::config_dir()
        .join("scenario")
        .join("client.json")
}

/// A dynamic client registration we can reuse.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientRecord {
    client_id: String,
    /// The redirect this client was registered against. Part of the record
    /// because it's part of the registration — a client registered for the CLI's
    /// loopback port is useless for a dashboard login, and Clerk rejects the
    /// mismatch at authorize time rather than at registration time.
    redirect_uri: String,
}

fn load_client_record() -> Option<ClientRecord> {
    let body = std::fs::read_to_string(client_path()).ok()?;
    serde_json::from_str(&body).ok()
}

fn save_client_record(record: &ClientRecord) -> Result<()> {
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| anyhow!("cannot serialize the Scenario client record: {e}"))?;
    write_private(&client_path(), &body)
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
    let body = serde_json::to_string_pretty(credentials)
        .map_err(|e| anyhow!("cannot serialize scenario credentials: {e}"))?;
    write_private(&auth_path(), &body)
}

/// Atomic 0600 write, shared by the token file and the client record.
fn write_private(path: &std::path::Path, body: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("cannot create {}: {e}", parent.display()))?;

    let tmp = parent.join(format!(".scenario.{}.tmp", uuid::Uuid::new_v4()));

    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, format!("{body}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
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

/// Forget the stored login. Used by `crux scenario disconnect` and the Settings
/// card.
///
/// Drops the client registration along with the tokens. Keeping it would save one
/// request on the next login, but disconnect is the "give me a clean slate"
/// escape hatch — and a client registration the server has since forgotten would
/// otherwise fail every future login with no way to clear it from the UI.
pub(crate) fn forget() -> Result<()> {
    remove_if_present(&auth_path())?;
    remove_if_present(&client_path())
}

fn remove_if_present(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("cannot remove {}: {e}", path.display())),
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

/// A login that has been started but not finished: the user still has to visit
/// [`Login::url`].
///
/// The two phases are separate because the dashboard has to answer its HTTP
/// request with the authorize URL *immediately*. Holding that response open for
/// the five minutes a human takes to log in would present as a hung request and
/// would die to any proxy's read timeout, so the endpoint returns the URL and
/// leaves [`Login::finish`] running on a background task.
pub(crate) struct Login {
    manager: AuthorizationManager,
    csrf: String,
    /// Where the user has to go to sign in.
    pub(crate) url: String,
    source: CodeSource,
}

/// How this login expects to receive its authorization code.
enum CodeSource {
    /// Our own listener, bound before the browser opened.
    Loopback(TcpListener),
    /// The dashboard's callback route hands it over through [`pending`].
    Dashboard(oneshot::Receiver<std::result::Result<String, String>>),
}

/// Start an interactive login: register or reuse a client, build the authorize
/// URL, and arrange to receive the redirect.
///
/// Does everything up to "the user must now visit this URL", and nothing that
/// waits on a human.
pub(crate) async fn begin(redirect: Redirect) -> Result<Login> {
    // Refuse a second concurrent login before doing any network work. For
    // `Loopback` the port bind below would catch it anyway; for `Dashboard` this
    // is the only guard, and matching the CLI's behaviour means a double-clicked
    // Connect button gets the same honest answer either way.
    if matches!(redirect, Redirect::Dashboard { .. }) && pending().lock().unwrap().is_some() {
        return Err(anyhow!(
            "A Scenario login is already in progress — finish it in the browser tab that opened, \
             or wait for it to time out."
        ));
    }

    let (mut manager, _) = manager().await?;

    // Bind before opening any browser: if the port is busy — a stale login still
    // holding it, or another tool on the same port — say so now rather than
    // sending the user to a page whose redirect can't land.
    let source = match redirect {
        Redirect::Loopback => CodeSource::Loopback(
            TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
                .await
                .map_err(|e| {
                    anyhow!(
                        "cannot listen on 127.0.0.1:{CALLBACK_PORT} for the Scenario login \
                         redirect: {e}\n\
                         Another login may still be in progress — wait a moment and retry."
                    )
                })?,
        ),
        // Filled in below, once the CSRF token exists to key it by.
        Redirect::Dashboard { .. } => CodeSource::Dashboard(oneshot::channel().1),
    };

    let redirect_uri = redirect.uri();
    // Reuse the client we already registered for this exact redirect. Registering
    // unconditionally would mint a fresh Clerk client on every login; tris-bot
    // saves its `clientInformation` for the same reason. The redirect has to be
    // part of the match because it's part of the registration — a CLI login and a
    // dashboard login use different redirects and so cannot share a client.
    match load_client_record().filter(|r| r.redirect_uri == redirect_uri) {
        Some(record) => manager
            .configure_client(
                OAuthClientConfig::new(record.client_id, redirect_uri.clone())
                    .with_scopes(SCOPES.iter().map(|s| s.to_string()).collect()),
            )
            .map_err(|e| anyhow!("cannot reuse the registered Scenario client: {e}"))?,
        None => {
            let config = manager
                .register_client(CLIENT_NAME, &redirect_uri, SCOPES)
                .await
                .map_err(|e| anyhow!("Scenario rejected the client registration: {e}"))?;
            save_client_record(&ClientRecord {
                client_id: config.client_id,
                redirect_uri: redirect_uri.clone(),
            })?;
        }
    }

    let url = manager
        .get_authorization_url(SCOPES)
        .await
        .map_err(|e| anyhow!("cannot build the Scenario authorize URL: {e}"))?;

    // CSRF token is echoed back as `state`; the exchange needs it to find the
    // PKCE verifier the manager stashed for this attempt.
    let csrf = query_param(&url, "state")
        .ok_or_else(|| anyhow!("Scenario authorize URL carried no state parameter"))?;

    let source = match source {
        CodeSource::Loopback(l) => CodeSource::Loopback(l),
        CodeSource::Dashboard(_) => {
            let (tx, rx) = oneshot::channel();
            // Any login that appeared during the network work above loses the
            // slot. Benign: its `finish` reports the cancellation, and the user
            // is looking at whichever tab opened last anyway.
            *pending().lock().unwrap() = Some(Pending {
                csrf: csrf.clone(),
                tx,
            });
            CodeSource::Dashboard(rx)
        }
    };

    Ok(Login {
        manager,
        csrf,
        url,
        source,
    })
}

impl Login {
    /// Wait for the redirect, exchange the code, and persist the tokens. Returns
    /// the granted scopes.
    pub(crate) async fn finish(self) -> Result<Vec<String>> {
        let code = match self.source {
            CodeSource::Loopback(listener) => {
                tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, &self.csrf))
                    .await
                    .map_err(|_| {
                        anyhow!("timed out after 5 minutes waiting for the Scenario login")
                    })??
            }
            CodeSource::Dashboard(rx) => {
                let outcome = tokio::time::timeout(LOGIN_TIMEOUT, rx).await;
                // Release the slot however this ended, or an abandoned login
                // would wedge the Connect button until the process restarts.
                clear_pending(&self.csrf);
                match outcome {
                    Ok(Ok(Ok(code))) => code,
                    Ok(Ok(Err(reason))) => {
                        return Err(anyhow!("Scenario login failed: {reason}"));
                    }
                    // Sender dropped: a later login took the slot.
                    Ok(Err(_)) => {
                        return Err(anyhow!("the Scenario login was replaced by a newer one"));
                    }
                    Err(_) => {
                        return Err(anyhow!(
                            "timed out after 5 minutes waiting for the Scenario login"
                        ));
                    }
                }
            }
        };

        self.manager
            .exchange_code_for_token(&code, &self.csrf)
            .await
            .map_err(|e| anyhow!("Scenario refused the authorization code: {e}"))?;

        // `exchange_code_for_token` persists through the credential store, so the
        // token file exists by now; read the granted scopes back for the caller.
        Ok(self.manager.get_current_scopes().await)
    }
}

/// Run the whole interactive login on the CLI's own loopback port.
pub(crate) async fn connect() -> Result<Vec<String>> {
    let login = begin(Redirect::Loopback).await?;
    eprintln!("Opening your browser to sign in to Scenario…");
    eprintln!("If it doesn't open, visit:\n  {}", login.url);
    crate::browser::open_browser(&login.url);
    login.finish().await
}

/// The one in-flight dashboard login, if any.
///
/// A single slot rather than a map: the CLI path is already serialized by its
/// fixed-port bind, so allowing several concurrent dashboard logins would be the
/// odd one out — and there is exactly one person at the browser.
struct Pending {
    csrf: String,
    tx: oneshot::Sender<std::result::Result<String, String>>,
}

fn pending() -> &'static Mutex<Option<Pending>> {
    static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

/// Drop the pending slot if it is still the one this login installed. The guard
/// matters: without it, a login that times out would evict the *successor* that
/// replaced it.
fn clear_pending(csrf: &str) {
    let mut slot = pending().lock().unwrap();
    if slot.as_ref().is_some_and(|p| p.csrf == csrf) {
        *slot = None;
    }
}

/// True while a dashboard login is waiting for its redirect. Lets the status
/// endpoint report "waiting for the browser" instead of a bare "not connected".
pub(crate) fn login_in_progress() -> bool {
    pending().lock().unwrap().is_some()
}

/// Hand a redirect's `code` to the waiting dashboard login.
///
/// `Ok` carries the message to show in the user's browser tab, `Err` the reason
/// it couldn't be accepted — the route renders either as a small page, since this
/// tab is the only place the user can see it.
pub(crate) fn deliver_callback(
    state: Option<&str>,
    code: Option<&str>,
    error: Option<&str>,
) -> std::result::Result<&'static str, String> {
    let mut slot = pending().lock().unwrap();
    let Some(p) = slot.as_ref() else {
        return Err(
            "No Scenario login is in progress. Start one from Settings → Scenario in Crux.".into(),
        );
    };
    // A stale browser tab from an earlier attempt replays an old state; that code
    // is worthless, and consuming the slot for it would cancel the live login. So
    // reject without taking it.
    if state != Some(p.csrf.as_str()) {
        return Err(
            "Login failed: state mismatch (a stale login tab?). Start the login again \
                    from Settings → Scenario in Crux."
                .into(),
        );
    }
    let p = slot.take().expect("checked immediately above");
    match code {
        Some(code) => {
            // A dropped receiver means the login already gave up (timed out);
            // nothing to report to it, and the tab still gets an honest page.
            if p.tx.send(Ok(code.to_string())).is_err() {
                return Err(
                    "That login already timed out. Start a new one from Settings → \
                            Scenario in Crux."
                        .into(),
                );
            }
            // Deliberately not "connected": the token exchange happens after this
            // response and can still fail. The Settings card is watching the SSE
            // stream and reports the real outcome — this page must not contradict
            // it by claiming a success it cannot yet see.
            Ok("Signed in — you can close this tab. Crux is finishing the login.")
        }
        None => {
            let reason = error.unwrap_or("no code returned").to_string();
            let _ = p.tx.send(Err(reason.clone()));
            Err(format!("Login failed: {reason}"))
        }
    }
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
                    // Same caution as the dashboard's page: the exchange comes
                    // after this, so the terminal — not this tab — is where the
                    // real outcome is reported.
                    "Signed in — you can close this tab and return to your terminal.",
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
    fn loopback_redirect_matches_the_callback_port() {
        assert_eq!(
            Redirect::Loopback.uri(),
            format!("http://localhost:{CALLBACK_PORT}/callback")
        );
    }

    /// The dashboard redirect must name the route the server actually mounts;
    /// they're set in different files, and a mismatch only shows up mid-login.
    #[test]
    fn dashboard_redirect_uses_the_mounted_path() {
        assert_eq!(
            Redirect::Dashboard { port: 3333 }.uri(),
            format!("http://localhost:3333{DASHBOARD_CALLBACK_PATH}")
        );
        assert!(DASHBOARD_CALLBACK_PATH.starts_with("/api/"));
    }

    /// The two redirects must differ, or the client-reuse check would hand a
    /// dashboard login a client registered for the CLI's port.
    #[test]
    fn the_two_redirects_are_distinct() {
        assert_ne!(
            Redirect::Loopback.uri(),
            Redirect::Dashboard {
                port: CALLBACK_PORT
            }
            .uri()
        );
    }

    /// The pending slot is process-global, so the tests below have to take turns.
    /// Each also resets the slot, so an earlier failure can't cascade.
    fn claim_pending_slot() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        *pending().lock().unwrap() = None;
        guard
    }

    /// A callback arriving with no login in flight must be refused, not panic —
    /// anyone can hit the route, and a stale bookmark will.
    #[test]
    fn callback_without_a_pending_login_is_refused() {
        let _guard = claim_pending_slot();
        let out = deliver_callback(Some("nope"), Some("code"), None);
        assert!(out.is_err(), "expected a refusal, got {out:?}");
    }

    /// A pending login must survive a stale tab replaying an old `state`: the
    /// slot stays put so the real redirect can still land.
    ///
    /// Synchronous, and reads the channel with `try_recv`: `deliver_callback`
    /// sends before it returns, so there is nothing to wait for — and holding the
    /// serializing guard across an await would be its own bug.
    #[test]
    fn stale_state_does_not_consume_the_pending_login() {
        let _guard = claim_pending_slot();
        let (tx, mut rx) = oneshot::channel();
        *pending().lock().unwrap() = Some(Pending {
            csrf: "live".to_string(),
            tx,
        });

        assert!(deliver_callback(Some("stale"), Some("code"), None).is_err());
        assert!(login_in_progress(), "the live login was evicted");
        assert!(rx.try_recv().is_err(), "a stale tab delivered a code");

        // The real redirect still works, and clears the slot.
        assert!(deliver_callback(Some("live"), Some("real-code"), None).is_ok());
        assert_eq!(rx.try_recv().unwrap().unwrap(), "real-code");
        assert!(!login_in_progress());
    }

    /// An authorize error (user declined) must reach the waiting login as a
    /// failure rather than leaving it to time out five minutes later.
    #[test]
    fn callback_error_is_delivered_to_the_waiting_login() {
        let _guard = claim_pending_slot();
        let (tx, mut rx) = oneshot::channel();
        *pending().lock().unwrap() = Some(Pending {
            csrf: "s".to_string(),
            tx,
        });

        assert!(deliver_callback(Some("s"), None, Some("access_denied")).is_err());
        assert_eq!(rx.try_recv().unwrap().unwrap_err(), "access_denied");
        assert!(!login_in_progress());
    }

    /// `clear_pending` must not evict a login that replaced the caller's — that
    /// would cancel a live login when an abandoned one times out.
    #[test]
    fn clear_pending_only_clears_its_own_login() {
        let _guard = claim_pending_slot();
        let (tx, _rx) = oneshot::channel();
        *pending().lock().unwrap() = Some(Pending {
            csrf: "successor".to_string(),
            tx,
        });

        clear_pending("abandoned");
        assert!(login_in_progress(), "the successor login was evicted");

        clear_pending("successor");
        assert!(!login_in_progress());
    }

    /// `offline_access` is what earns a refresh token; losing it would make
    /// every login expire within the hour.
    #[test]
    fn scopes_request_offline_access() {
        assert!(SCOPES.contains(&"offline_access"));
    }
}
