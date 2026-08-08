//! `orx up` — the local autoresearch dashboard server.
//!
//! One axum process on 127.0.0.1 serving three surfaces:
//!   /            embedded SPA (rust-embed over ui/dist, index.html fallback)
//!   /api/*       JSON over the local SQLite store + run-log files
//!   /api/events  SSE: 500ms store + log-file diff loop (serve.rs idiom)
//!   /opencode/*  streaming reverse proxy to the locally spawned `opencode serve`
//!
//! Fully local: no OpenResearch api anywhere on these paths (the /api/papers
//! routes proxy alphaXiv's public, token-free endpoints — needed because the
//! browser can't call api.alphaxiv.org cross-origin). No auth — the bind is
//! loopback-only by default; `--host` can widen it (e.g. 0.0.0.0 for the
//! local network), which the startup output flags loudly since every surface
//! is then open to whoever can reach the port.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::error::{anyhow, Result};
use crate::local;
use crate::local::chat::ChatHost;
use crate::local::opencode::AgentHost;
use crate::store::{log_path, now_ms, SshHostTest, Store, StoredChatSession, StoredRun};
use crate::{browser, UpArgs};

pub async fn run(args: UpArgs) -> Result<()> {
    let port = args.port;
    let host: std::net::IpAddr = args.host.parse().map_err(|_| {
        anyhow!(
            "Invalid --host '{}': expected an IP address (e.g. 127.0.0.1, or 0.0.0.0 for the local network)",
            args.host
        )
    })?;
    let listener = tokio::net::TcpListener::bind((host, port))
        .await
        .map_err(|e| anyhow!("Could not bind {}:{}: {}", host, port, e))?;
    // Open early so the schema exists before any request or agent spawn.
    Store::open()?;

    // Harnesses spawn lazily on the first message to one of their sessions;
    // no eager agent bring-up. (--no-agent is now a no-op kept for compat.)
    let agent = Arc::new(AgentHost::new(args.model.clone()));
    let codex = Arc::new(local::codex::CodexHost::new());
    let claude = Arc::new(local::claude::ClaudeHost::new());
    let state = AppState {
        agent: agent.clone(),
        chat: Arc::new(ChatHost::new(agent.clone(), codex.clone(), claude.clone())),
        claude: claude.clone(),
        harnesses: Arc::new(tokio::sync::Mutex::new(None)),
        blender: Arc::new(tokio::sync::Mutex::new(None)),
        blender_status: Arc::new(tokio::sync::Mutex::new(None)),
        comfyui: Arc::new(tokio::sync::Mutex::new(None)),
        comfyui_status: Arc::new(tokio::sync::Mutex::new(None)),
        project_lifecycle: Arc::new(ProjectLifecycle::default()),
        data_dir_move_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    // Plan-mode turns hand this port to the `orx mcp-gate` permission bridge.
    state.chat.set_up_port(port);

    // Start the Blender MCP server before anything can spawn a harness. The order
    // is the point: harness children are configured at spawn and live for many
    // turns, so a server that arrived later would leave the first child without
    // Blender tools for its whole life.
    if let Some(server) = local::blender::server::start_if_available().await {
        local::blender::server::spawn_watchdog(server.clone());
        *state.blender.lock().await = Some(server);
    }
    // Same ordering requirement. ComfyUI itself is *not* started here — it's a GPU
    // server that loads models, so it's adopted if already running and otherwise
    // started on request from its card. The MCP server tolerates it being down.
    if let Some(server) = local::comfyui::server::start_if_available().await {
        local::comfyui::server::spawn_watchdog(server.clone());
        *state.comfyui.lock().await = Some(server);
    }

    spawn_hf_preflight();
    spawn_k8s_preflight();
    spawn_agent_git_preflight();
    // End play-session runs whose browser vanished without a clean end —
    // the play tab beats every 10s; minutes of silence means it's gone.
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Ok(store) = Store::open() {
                match store.reap_stale_play_sessions(now_ms() - PLAY_SESSION_STALE_MS) {
                    Ok(0) | Err(_) => {}
                    Ok(n) => eprintln!("orx up: ended {n} stale play session(s)"),
                }
            }
        }
    });
    // Wake an idle chat session when a run completes (the agent's wait loop
    // covers the busy case; this covers turns that ended early).
    tokio::spawn(local::chat::watch_runs(state.chat.clone()));
    spawn_claude_auth_monitor(state.chat.clone(), claude.clone(), state.harnesses.clone());

    let app = router(state);
    // Loopback stays the browser URL when it's reachable (0.0.0.0 includes
    // it); a specific non-loopback bind excludes loopback, so use it directly.
    let url = if host.is_loopback() || host.is_unspecified() {
        format!("http://127.0.0.1:{port}")
    } else {
        ip_url(host, port)
    };
    if !host.is_loopback() {
        let lan = if host.is_unspecified() {
            lan_ip()
        } else {
            Some(host)
        };
        if let Some(ip) = lan {
            eprintln!(
                "orx up: dashboard reachable on your network at {}",
                ip_url(ip, port)
            );
        }
        eprintln!(
            "orx up: warning: --host {} exposes the unauthenticated dashboard beyond this \
             machine — anyone who can reach the port can run code and read files as you.",
            args.host
        );
    }
    // In an SSH session the loopback URL only works on the remote box and there's
    // no local browser to open — print forwarding guidance instead of the bare
    // URL, and skip the (futile) browser-open. Otherwise, today's local flow.
    if let Some(session) = crate::remote::detect_ssh_session() {
        eprint!("{}", session.instructions(port));
    } else {
        eprintln!("orx up: dashboard on {url}");
        if !args.no_browser {
            browser::open_browser(&url);
        }
    }

    // select! instead of graceful shutdown: open SSE streams never complete,
    // so waiting on connections would hang Ctrl-C forever.
    //
    // We wait on SIGTERM/SIGHUP as well as SIGINT: when this server is started
    // over SSH by `orx up --remote`, closing that tunnel (the launcher's Ctrl-C)
    // delivers SIGHUP here as the channel tears down — without handling it the
    // remote server would leak, staying bound to its port after the tunnel dies.
    tokio::select! {
        r = axum::serve(listener, app) => r.map_err(|e| anyhow!("orx up: server error: {e}"))?,
        _ = shutdown_signal() => eprintln!("orx up: shutting down"),
    }
    agent.shutdown().await;
    codex.shutdown().await;
    claude.shutdown().await;
    Ok(())
}

/// `http://` URL for an IP:port, with the brackets IPv6 needs.
fn ip_url(ip: std::net::IpAddr, port: u16) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        std::net::IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

/// Best-effort LAN address for the "reachable at" line when bound to
/// 0.0.0.0: "connecting" a UDP socket picks the interface the OS would route
/// through, without sending any packet. None (e.g. no network) just skips
/// the line.
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Resolves when the process is asked to stop. SIGINT everywhere; on Unix also
/// SIGTERM and SIGHUP (SIGHUP is what an SSH tunnel delivers on disconnect, so
/// a `--remote`-launched server exits with its tunnel instead of leaking).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, Signal, SignalKind};
        // If a signal stream can't be installed, that arm simply never fires —
        // fall back to whatever handlers do register rather than aborting.
        async fn wait(s: &mut Option<Signal>) {
            match s {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending().await,
            }
        }
        let mut term = signal(SignalKind::terminate()).ok();
        let mut hup = signal(SignalKind::hangup()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = wait(&mut term) => {}
            _ = wait(&mut hup) => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[derive(Clone)]
struct AppState {
    agent: Arc<AgentHost>,
    chat: Arc<ChatHost>,
    claude: Arc<local::claude::ClaudeHost>,
    /// Harness detection cache — detection shells out to CLIs, so it's rate-
    /// limited to once per TTL unless the UI asks for a refresh.
    harnesses: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>>,
    /// The managed `blender-mcp` child, when one started. `None` on a machine
    /// without a usable install — Blender is optional, so that is a normal state,
    /// not a failure.
    blender: Arc<tokio::sync::Mutex<Option<local::blender::server::BlenderServer>>>,
    /// Blender status cache. Same reason as `harnesses`: detection shells out and
    /// opens a socket, so it must not run on every poll.
    blender_status: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>>,
    /// The managed `comfyui-mcp` child, when one started.
    comfyui: Arc<tokio::sync::Mutex<Option<local::comfyui::server::ComfyMcpServer>>>,
    /// ComfyUI status cache. Its probe is the most expensive of the three — a
    /// `--help` subprocess plus several HTTP calls, one of which pulls a ~1.4 MB
    /// `/object_info` — so it must not run per poll.
    comfyui_status: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>>,
    project_lifecycle: Arc<ProjectLifecycle>,
    /// Set while a data-dir move is running. New chat turns and run launches
    /// check it and refuse (409) so nothing starts writing the store mid-move —
    /// closing the window between the move's in-flight check and its completion.
    data_dir_move_in_progress: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct ProjectLifecycle {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
}

#[derive(Default)]
struct ProjectLifecycleState {
    deleting: HashSet<String>,
    admissions: HashMap<String, usize>,
}

struct ProjectAdmissionLease {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
    project_id: String,
}

impl Drop for ProjectAdmissionLease {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap();
        if let Some(count) = state.admissions.get_mut(&self.project_id) {
            *count -= 1;
            if *count == 0 {
                state.admissions.remove(&self.project_id);
            }
        }
    }
}

struct ProjectDeletionLease {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
    project_id: String,
}

impl Drop for ProjectDeletionLease {
    fn drop(&mut self) {
        self.inner.lock().unwrap().deleting.remove(&self.project_id);
    }
}

impl ProjectLifecycle {
    fn admit(&self, project_id: &str) -> Option<ProjectAdmissionLease> {
        let mut state = self.inner.lock().unwrap();
        if state.deleting.contains(project_id) {
            return None;
        }
        *state.admissions.entry(project_id.to_string()).or_default() += 1;
        Some(ProjectAdmissionLease {
            inner: self.inner.clone(),
            project_id: project_id.to_string(),
        })
    }

    fn begin_delete(&self, project_id: &str) -> Option<ProjectDeletionLease> {
        let mut state = self.inner.lock().unwrap();
        if state.deleting.contains(project_id)
            || state
                .admissions
                .get(project_id)
                .copied()
                .unwrap_or_default()
                > 0
        {
            return None;
        }
        state.deleting.insert(project_id.to_string());
        Some(ProjectDeletionLease {
            inner: self.inner.clone(),
            project_id: project_id.to_string(),
        })
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/api/projects/{id}/open", post(open_project))
        .route(
            "/api/projects/{id}/experiments",
            get(list_experiments).post(create_experiment),
        )
        .route("/api/projects/{id}/runs", get(list_project_runs))
        .route("/api/projects/{id}/branches", get(list_project_branches))
        .route("/api/papers/search", get(search_papers_api))
        .route("/api/papers/resolve", get(resolve_paper_api))
        .route("/api/instances", get(list_instances))
        .route("/api/instances/reconcile", post(reconcile_instances))
        .route("/api/experiments/{id}/run", post(run_experiment))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route("/api/runs/{id}/verdict", post(set_run_verdict))
        .route("/api/runs/{id}/metrics", get(run_metrics))
        .route("/api/runs/{id}/artifacts", get(list_run_artifacts))
        .route("/api/runs/{id}/artifacts/file", get(serve_run_artifact))
        .route("/api/experiments/{id}/play-build", post(play_build))
        .route(
            "/api/experiments/{id}/play-session/beat",
            post(play_session_beat),
        )
        .route(
            "/api/experiments/{id}/play-session/end",
            post(play_session_end),
        )
        .route(
            "/api/experiments/{id}/play-session/events",
            post(play_session_events),
        )
        .route(
            "/api/experiments/{id}",
            axum::routing::patch(update_experiment),
        )
        .route("/play/{id}", get(play_root))
        .route("/play/{id}/", get(play_index))
        .route("/play/{id}/{*path}", get(play_file))
        .route("/api/experiments/{id}/verdict", post(set_exp_verdict))
        .route("/api/runs/{id}/log", get(run_log))
        .route("/api/runs/{id}/diff", get(run_diff))
        .route("/api/experiments/{id}/commits", get(experiment_commits))
        .route(
            "/api/experiments/{id}/commits/{sha}/diff",
            get(experiment_commit_diff),
        )
        .route("/api/projects/{id}/working-tree", get(project_working_tree))
        .route("/api/projects/{id}/code-tree", get(project_code_tree))
        .route("/api/projects/{id}/file", get(project_file))
        .route(
            "/api/projects/{id}/files",
            get(list_files).delete(delete_file),
        )
        .route("/api/projects/{id}/files/report", get(file_report))
        .route("/api/projects/{id}/files/file", get(serve_file))
        .route("/api/events", get(events))
        .route("/api/settings/hf", get(hf_settings).post(set_hf_token))
        .route(
            "/api/settings/k8s",
            get(k8s_settings).post(set_k8s_settings),
        )
        .route("/api/settings/modal", get(modal_settings))
        .route("/api/settings/modal/provision", post(provision_modal))
        .route("/api/settings/genai", get(genai_settings))
        .route("/api/settings/comfyui/start", post(start_comfyui))
        .route("/api/settings/scenario", get(scenario_settings))
        .route("/api/settings/scenario/connect", post(connect_scenario))
        .route(
            "/api/settings/scenario/disconnect",
            post(disconnect_scenario),
        )
        // Not under /api/settings: this is where Scenario's OAuth server sends the
        // user's browser, and the path is baked into the client registration
        // (`auth::DASHBOARD_CALLBACK_PATH`), so it can't be moved freely.
        .route(
            local::scenario::auth::DASHBOARD_CALLBACK_PATH,
            get(scenario_callback),
        )
        .route("/api/settings/env", get(env_settings).post(set_env_var))
        .route(
            "/api/settings/env/{key}",
            axum::routing::delete(delete_env_var),
        )
        .route(
            "/api/settings/data-dir",
            get(data_dir_settings).post(set_data_dir),
        )
        .route("/api/settings/data-dir/validate", post(validate_data_dir))
        .route("/api/settings/data-dir/move", post(move_data_dir))
        .route("/api/github/repo-access", get(github_repo_access))
        .route("/api/github/account", get(github_account))
        .route(
            "/api/settings/git",
            get(git_settings).post(set_git_settings),
        )
        .route(
            "/api/settings/git/token",
            post(set_git_token).delete(delete_git_token),
        )
        .route(
            "/api/settings/telemetry",
            get(telemetry_settings).post(set_telemetry_settings),
        )
        .route(
            "/api/settings/telemetry/consent",
            post(record_telemetry_consent),
        )
        .route("/api/settings/ssh", get(ssh_settings))
        .route("/api/settings/ssh/preflight", post(ssh_preflight))
        .route(
            "/api/settings/slurm",
            get(slurm_settings).post(set_slurm_settings),
        )
        .route("/api/settings/slurm/preflight", post(slurm_preflight))
        .route(
            "/api/settings/ray",
            get(ray_settings).post(set_ray_settings),
        )
        .route("/api/settings/ray/preflight", post(ray_preflight))
        .route("/api/settings/compute", get(compute_settings))
        .route("/api/settings/compute/default", post(set_compute_default))
        .route("/api/settings/local", get(local_machine_settings))
        .route("/api/settings/openresearch", get(openresearch_settings))
        .route("/api/harnesses", get(list_harnesses))
        .route("/api/skills", get(list_skills))
        .route("/api/personas", get(list_personas))
        .route("/api/github/repos", get(list_github_repos))
        .route(
            "/api/chat/sessions",
            get(list_chat_sessions).post(create_chat_session),
        )
        .route(
            "/api/chat/sessions/{id}",
            axum::routing::delete(delete_chat_session).patch(update_chat_session),
        )
        .route("/api/chat/sessions/{id}/messages", get(chat_messages))
        .route("/api/chat/sessions/{id}/worktree", get(session_worktree))
        .route("/api/chat/sessions/{id}/message", post(send_chat_message))
        .route("/api/chat/sessions/{id}/interrupt", post(interrupt_chat))
        .route("/api/chat/sessions/{id}/respond", post(respond_chat))
        // Internal: the `orx mcp-gate` permission bridge's long-poll (plan
        // mode). Token-authenticated in the handler; blocks until the surfaced
        // card is answered.
        .route("/api/internal/permissions", post(bridge_permission))
        .route("/api/chat/attachments/{name}", get(chat_attachment))
        // Subagent dispatch: the orchestrator suggests (via `orx agent suggest`);
        // the human approves here, which spawns a session with the chosen persona.
        .route("/api/projects/{id}/proposals", get(list_proposals))
        .route("/api/agent/proposals/{id}/approve", post(approve_proposal))
        .route("/api/agent/proposals/{id}/dismiss", post(dismiss_proposal))
        .route("/api/agent/status", get(agent_status))
        .fallback(spa)
        .with_state(state)
}

// --- error plumbing -------------------------------------------------------

/// JSON error responses: `{"error": "..."}` with an explicit status.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<crate::error::Error> for ApiError {
    fn from(err: crate::error::Error) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

fn bad_request(err: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, err.to_string())
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

type ApiResult = std::result::Result<Json<Value>, ApiError>;

// --- wire types -----------------------------------------------------------

/// The Run entity the API serves: StoredRun with `backend_json` parsed into an
/// object and internal fields (cancel intent) dropped.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiRun {
    id: String,
    experiment_id: String,
    project_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_markdown: Option<String>,
    created_at: i64,
    updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    /// Watcher (supervisor process) state for a live run: "alive" when its
    /// heartbeat is fresh, "lost" when it stopped beating (reboot, kill).
    /// Absent on terminal runs — nothing should be watching those.
    #[serde(skip_serializing_if = "Option::is_none")]
    watcher: Option<&'static str>,
    /// job | play | sim (later: verify, ladder).
    kind: String,
    /// The `aggregate` object of the run's ingested metrics — kept small so
    /// list/SSE payloads stay light; the full doc is GET /api/runs/{id}/metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics_aggregate: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict_notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict_at: Option<i64>,
}

/// A watcher heartbeat this old means the supervisor is gone (it stamps every
/// 5s poll; three misses is decisive).
const WATCHER_FRESH_MS: i64 = 15_000;

fn watcher_alive(run: &StoredRun) -> bool {
    run.supervisor_heartbeat_ms
        .is_some_and(|t| now_ms() - t < WATCHER_FRESH_MS)
}

impl From<&StoredRun> for ApiRun {
    fn from(run: &StoredRun) -> Self {
        Self {
            id: run.id.clone(),
            experiment_id: run.experiment_id.clone(),
            project_id: run.project_id.clone(),
            status: run.status.clone(),
            backend: serde_json::from_str(&run.backend_json).ok(),
            command: Some(run.command.clone()).filter(|c| !c.is_empty()),
            commit_sha: run.commit_sha.clone(),
            result_markdown: run.result_markdown.clone(),
            created_at: run.created_at,
            updated_at: run.updated_at,
            ended_at: run.ended_at,
            exit_code: run.exit_code,
            watcher: if is_terminal(&run.status) {
                None
            } else if watcher_alive(run) {
                Some("alive")
            } else {
                Some("lost")
            },
            kind: run.kind.clone(),
            metrics_aggregate: run
                .metrics_json
                .as_deref()
                .and_then(|m| serde_json::from_str::<Value>(m).ok())
                .and_then(|v| crate::store::metrics_aggregate(&v)),
            verdict: run.verdict.clone(),
            verdict_notes: run.verdict_notes.clone(),
            verdict_at: run.verdict_at,
        }
    }
}

// --- basic routes ---------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}

/// Slash-skills the composer's `/` dropdown offers (expanded server-side).
async fn list_skills() -> Json<Value> {
    let skills: Vec<Value> = crate::local::skills::CATALOG
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "argHint": s.arg_hint,
            })
        })
        .collect();
    Json(json!({ "skills": skills }))
}

/// The project repo's pickable fork-point branches (origin, minus `orx/*`
/// experiment branches) plus the current baseline — backs the composer's
/// baseline-branch picker.
async fn list_project_branches(Path(id): Path<String>) -> ApiResult {
    let result = tokio::task::spawn_blocking(move || -> Result<(Vec<String>, String)> {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| anyhow!("project not found"))?;
        let repo = std::path::PathBuf::from(&project.repo_path);
        let _ = local::git::fetch_origin(&repo);
        let branches = local::git::list_remote_branches(&repo)?;
        Ok((branches, project.baseline_branch))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
    .map_err(bad_request)?;
    let (branches, baseline) = result;
    Ok(Json(json!({ "branches": branches, "baseline": baseline })))
}

/// The signed-in user's GitHub repos (most recently pushed first) for the
/// New Project repo autocomplete. Empty when no token is configured.
async fn list_github_repos() -> ApiResult {
    let repos = local::github::list_user_repos().await?;
    Ok(Json(json!({ "repos": repos })))
}

/// The available agent personas, for the dashboard's Persona tab: label,
/// blurb, the system-prompt template (repo-reader comment stripped, `{token}`
/// placeholders left visible), and the skill set each persona injects.
async fn list_personas() -> Json<Value> {
    let personas: Vec<Value> = local::agent_skills::Persona::ALL
        .iter()
        .map(|&p| {
            let skills: Vec<Value> = local::agent_skills::skills_for_persona(p)
                .iter()
                .map(|s| {
                    json!({
                        "name": s.name,
                        "description": s.description,
                        "content": s.content,
                    })
                })
                .collect();
            json!({
                "id": p.as_str(),
                "label": p.label(),
                "description": p.blurb(),
                "systemPrompt": local::opencode::persona_template(p),
                "skills": skills,
            })
        })
        .collect();
    Json(json!({ "personas": personas }))
}

/// Serialize a project for the UI, injecting the absolute `filesDir` so the
/// dashboard can recognize files-dir paths in chat links. Every project the UI
/// receives must go through this — the SSE `project.updated` diff (fired on
/// every visit, since `open_project` bumps `updated_at`) and `create_project`
/// upsert into the same `projects` state as `list_projects`, so enriching one
/// site alone would let a bare project overwrite `filesDir` mid-session.
fn project_json(p: &local::model::LocalProject) -> Value {
    // LocalProject is all String/Option/i64, so this can't realistically fail;
    // fail loud rather than emit a malformed `null` project if it ever does.
    let mut v = serde_json::to_value(p).expect("LocalProject serializes");
    if let Value::Object(map) = &mut v {
        map.insert(
            "filesDir".into(),
            Value::String(local::files::files_dir_display(p)),
        );
    }
    v
}

async fn list_projects() -> ApiResult {
    let projects = Store::open()?.list_local_projects()?;
    let projects: Vec<Value> = projects.iter().map(project_json).collect();
    Ok(Json(json!({ "projects": projects })))
}

// --- papers (new-project "from a paper" flow; proxies alphaXiv) ------------

#[derive(Deserialize)]
struct PaperSearchQ {
    q: String,
}

async fn search_papers_api(Query(q): Query<PaperSearchQ>) -> ApiResult {
    let query = q.q.trim();
    if query.is_empty() {
        return Ok(Json(json!({ "papers": [] })));
    }
    let papers = crate::client::search_papers_fast(query)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "papers": papers })))
}

#[derive(Deserialize)]
struct PaperResolveQ {
    id: String,
}

async fn resolve_paper_api(Query(q): Query<PaperResolveQ>) -> ApiResult {
    let id = super::paper::parse_paper_id(&q.id);
    if id.is_empty() {
        return Err(bad_request("paper id is required"));
    }
    let paper = crate::client::resolve_paper(&id)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "paper": paper })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectReq {
    name: String,
    github_owner: Option<String>,
    github_repo: Option<String>,
    /// Optional destination organization for a created or fork-copied repo.
    github_organization: Option<String>,
    baseline_branch: Option<String>,
    run_command: Option<String>,
    /// arXiv id of the paper this project starts from (versionless).
    paper_id: Option<String>,
    /// Create a blank private repo named after the project under the requested
    /// GitHub organization, or under the signed-in user when none is supplied.
    #[serde(default)]
    create_repo: bool,
    /// Fork-by-copy the entered repo into a fresh `<repo>-<hash>` repo on the
    /// requested organization or signed-in user's account. Also applied
    /// automatically when the user lacks push access to the entered repo.
    #[serde(default)]
    fork_repo: bool,
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name is required"));
    }
    let (owner, repo, baseline_branch) = if req.create_repo {
        let (owner, repo, default_branch) =
            local::github::create_repo(&local::slugify(&name), req.github_organization.as_deref())
                .await
                .map_err(bad_request)?;
        (owner, repo, Some(default_branch))
    } else {
        let owner = req.github_owner.unwrap_or_default().trim().to_string();
        let repo = req.github_repo.unwrap_or_default().trim().to_string();
        if owner.is_empty() || repo.is_empty() {
            return Err(bad_request("githubOwner and githubRepo are required"));
        }
        let branch = req.baseline_branch.filter(|b| !b.trim().is_empty());
        let meta = local::github::repo_meta(&owner, &repo).await;
        // No branch given → the repo's own default. Falling through to
        // create_project's hardcoded "main" breaks every master/dev repo with
        // "Branch 'main' not found" — and the form has no branch field to
        // recover with. The API answer needs a token, so fall back to git's
        // own credentials for the SSH-only user.
        let branch = match branch.or_else(|| meta.as_ref().and_then(|m| m.default_branch.clone())) {
            Some(b) => Some(b),
            None => {
                let (o, r) = (owner.clone(), repo.clone());
                // Bounded like authed_get: ls-remote tries ssh then https, and
                // a black-holed host blocks on TCP connect for ~75s each. A
                // stall degrades to create_project's "main" instead of hanging
                // the create request.
                tokio::time::timeout(
                    Duration::from_secs(15),
                    tokio::task::spawn_blocking(move || local::git::remote_default_branch(&o, &r)),
                )
                .await
                .ok()
                .and_then(|joined| joined.ok())
                .flatten()
            }
        };
        // Unknown access (no token / API hiccup) counts as access: forking
        // needs a token anyway, and surprise forks are worse than a later
        // push error.
        let fork = req.fork_repo || !meta.as_ref().map(|m| m.can_push).unwrap_or(true);
        if fork {
            // The entered branch picks what gets copied; the fork itself
            // starts at its default branch. The seed clone/push streams the
            // same progress events as a direct clone, tagged with the SOURCE
            // repo (the fork's name isn't known to the dialog).
            let (chat, ev_owner, ev_repo) = (state.chat.clone(), owner.clone(), repo.clone());
            let (owner, repo, default_branch) = local::github::fork_copy_repo(
                &owner,
                &repo,
                branch,
                req.github_organization.as_deref(),
                move |p: local::git::CloneProgress| {
                    chat.emit_event(
                        "project.clone.progress",
                        json!({
                            "owner": ev_owner,
                            "repo": ev_repo,
                            "phase": p.phase,
                            "percent": p.percent,
                        }),
                    );
                },
            )
            .await
            .map_err(bad_request)?;
            (owner, repo, Some(default_branch))
        } else {
            (owner, repo, branch)
        }
    };
    // The clone shells out to git (network); keep it off the async workers.
    let run_command = req.run_command;
    let paper_id = req.paper_id.filter(|p| !p.trim().is_empty());
    let chat = state.chat.clone();
    let clone = move || {
        let store = Store::open()?;
        // Clone progress → SSE, so the New Project dialog can show a live
        // bar (`emit_event` is a sync broadcast send — fine off-runtime).
        let (chat, ev_owner, ev_repo) = (chat.clone(), owner.clone(), repo.clone());
        let on_progress = move |p: local::git::CloneProgress| {
            chat.emit_event(
                "project.clone.progress",
                json!({
                    "owner": ev_owner,
                    "repo": ev_repo,
                    "phase": p.phase,
                    "percent": p.percent,
                }),
            );
        };
        local::projects::create_project(
            &store,
            &name,
            &owner,
            &repo,
            baseline_branch,
            run_command,
            paper_id,
            &on_progress,
        )
    };
    let mut result = tokio::task::spawn_blocking(clone.clone())
        .await
        .map_err(|e| anyhow!("clone task failed: {e}"))?;
    // A just-created repo can lag a beat before its auto-init commit is
    // cloneable; one retry covers it (the clone step is idempotent).
    if result.is_err() && req.create_repo {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        result = tokio::task::spawn_blocking(clone)
            .await
            .map_err(|e| anyhow!("clone task failed: {e}"))?;
    }
    Ok(Json(
        json!({ "project": project_json(&result.map_err(bad_request)?) }),
    ))
}

async fn get_project(Path(id): Path<String>) -> ApiResult {
    let project = Store::open()?
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

/// Mark a project visited: bumps updated_at, which drives the recency sort
/// and the SSE project.updated diff.
async fn open_project(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store.touch_local_project(&id)?;
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

/// Present-vs-absent for PATCH fields: absent = leave, null = clear.
fn double_option<'de, D>(d: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProjectReq {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    run_command: Option<Option<String>>,
    /// Agent persona wire id (`research` | `game-designer`).
    persona: Option<String>,
    /// Automatic `[orx]` prompt switches (whole object replaces the stored one).
    auto_prompts: Option<local::model::AutoPrompts>,
    /// Branch new baselines fork from (must exist on origin).
    baseline_branch: Option<String>,
}

async fn update_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProjectReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    if req.name.is_none()
        && req.run_command.is_none()
        && req.persona.is_none()
        && req.auto_prompts.is_none()
        && req.baseline_branch.is_none()
    {
        return Err(bad_request(
            "nothing to update: pass name, runCommand, persona, autoPrompts, and/or baselineBranch",
        ));
    }
    let store = Store::open()?;
    let mut project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(bad_request("name cannot be empty"));
        }
        project.name = name.trim().to_string();
    }
    if let Some(cmd) = req.run_command {
        project.run_command = cmd.filter(|c| !c.trim().is_empty());
    }
    if let Some(persona) = req.persona {
        let parsed =
            local::agent_skills::Persona::parse(Some(persona.trim())).map_err(bad_request)?;
        project.persona = Some(parsed.as_str().to_string());
    }
    if let Some(prompts) = req.auto_prompts {
        project.auto_prompts = Some(prompts);
    }
    if let Some(branch) = req.baseline_branch {
        let branch = branch.trim().to_string();
        if branch.is_empty() {
            return Err(bad_request("baselineBranch cannot be empty"));
        }
        // Must exist on origin — a typo'd fork point would surface much later
        // as an opaque branch-create failure.
        let repo_path = std::path::PathBuf::from(&project.repo_path);
        let check = branch.clone();
        let exists = tokio::task::spawn_blocking(move || {
            let _ = local::git::fetch_origin(&repo_path);
            local::git::resolve_branch_commit(&repo_path, &check)
        })
        .await
        .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))??;
        if exists.is_none() {
            return Err(bad_request(format!(
                "branch '{branch}' not found on origin"
            )));
        }
        project.baseline_branch = branch;
    }
    store.update_local_project(&project)?;
    // Re-read: update bumps updated_at, which is also what fires the SSE
    // project.updated diff.
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

/// Delete a project and everything hanging off it. Refuses while runs are in
/// flight (deleting their rows would strand the supervisor mid-job) — but
/// requests their cancellation, so a retry shortly after goes through. The
/// GitHub repo and the cache clone are left untouched.
async fn delete_project(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let _deleting_project = state
        .project_lifecycle
        .begin_delete(&id)
        .ok_or_else(|| bad_request("project has an operation or deletion in progress"))?;
    let store = Store::open()?;
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let in_flight: Vec<_> = store
        .list_runs_by_project(&id)?
        .into_iter()
        .filter(|r| !is_terminal(&r.status))
        .collect();
    if !in_flight.is_empty() {
        for run in &in_flight {
            let _ = store.set_cancel_requested(&run.id, true);
        }
        return Err(bad_request(format!(
            "{} run(s) still in flight — cancellation requested; retry once they stop",
            in_flight.len()
        )));
    }
    // Abort any in-flight chat turns before their rows disappear, and clean up
    // each session's serve child + worktree (the rows cascade with the project).
    let sessions = store.list_chat_sessions_by_project(&id)?;
    let mut _session_deletions = Vec::with_capacity(sessions.len());
    for session in &sessions {
        _session_deletions.push(
            state
                .chat
                .begin_session_delete(&session.id)
                .ok_or_else(|| bad_request("a chat session deletion is already in progress"))?,
        );
    }
    for session in &sessions {
        let _ = state.chat.interrupt(&session.id).await;
        state.chat.opencode.kill_session(&session.id).await;
        state.chat.codex.kill_session(&session.id).await;
        state.chat.claude.forget_session(&session.id).await;
    }
    store.delete_local_project(&id)?;
    for session in &sessions {
        local::chat::cleanup_session_worktree(&project, &session.id);
    }
    Ok(Json(json!({ "ok": true })))
}

async fn list_experiments(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let experiments = store.list_experiments_by_project(&id)?;
    Ok(Json(json!({ "experiments": experiments })))
}

async fn list_project_runs(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let runs: Vec<ApiRun> = store
        .list_runs_by_project(&id)?
        .iter()
        .map(ApiRun::from)
        .collect();
    Ok(Json(json!({ "runs": runs })))
}

/// Newest-first cap for the cross-project instances list. Generous: the store
/// is a local single-user SQLite db, so this only bounds pathological history.
const INSTANCES_LIMIT: usize = 500;

/// Every run across all projects (running first on the client), each tagged
/// with its owning project's name — the "instances" view of compute the agents
/// have spun up (Modal / HF / SSH / K8s), regardless of which project launched
/// it. Includes finished runs as history; the client surfaces live ones first.
async fn list_instances() -> ApiResult {
    let store = Store::open()?;
    let names: HashMap<String, String> = store
        .list_local_projects()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut instances: Vec<Value> = Vec::new();
    for run in store.list_runs(INSTANCES_LIMIT)? {
        // ApiRun is a plain serializable struct, so this can't realistically
        // fail; propagate rather than emit a malformed row if it ever does.
        let mut value = serde_json::to_value(ApiRun::from(&run))
            .map_err(|e| anyhow!("serialize run {}: {e}", run.id))?;
        if let (Some(obj), Some(name)) = (value.as_object_mut(), names.get(&run.project_id)) {
            obj.insert("projectName".into(), json!(name));
        }
        instances.push(value);
    }
    Ok(Json(json!({ "instances": instances })))
}

/// Re-attach watchers: for every non-terminal run whose supervisor heartbeat
/// is stale or absent, respawn `orx supervise <id>`. Supervision is
/// restart-idempotent — the fresh watcher re-inspects the backend and settles
/// the row (a vanished local process becomes `failed`, a finished HF job
/// `done`, a still-running job resumes live tailing). Runs whose watcher is
/// alive are left alone, so the button is safe to mash.
async fn reconcile_instances() -> ApiResult {
    let store = Store::open()?;
    let mut reattached: Vec<String> = Vec::new();
    for run in store.list_runs(INSTANCES_LIMIT)? {
        // Play sessions have no supervisor — the stale-session reaper is
        // their reconciliation.
        if is_terminal(&run.status) || watcher_alive(&run) || run.kind == "play-session" {
            continue;
        }
        crate::commands::exp::spawn_detached_supervise(&run.id)?;
        reattached.push(run.id);
    }
    Ok(Json(json!({ "reattached": reattached })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateExperimentReq {
    parent_experiment_id: Option<String>,
    /// Force a new baseline root even when the project already has one.
    #[serde(default)]
    baseline: bool,
    slug: Option<String>,
    title: Option<String>,
    description: Option<String>,
    run_command: Option<String>,
    /// Second parent: merge this experiment's branch into the new node.
    merge_parent_experiment_id: Option<String>,
}

async fn create_experiment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateExperimentReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    // Branch create + push shells out to git (network); off the async workers.
    let experiment = tokio::task::spawn_blocking(move || {
        let _admission = admission;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let parent = match &req.parent_experiment_id {
            Some(pid) => Some(
                store
                    .get_local_experiment(pid)?
                    .ok_or_else(|| not_found("parent experiment"))?,
            ),
            // `baseline` forces a new root; otherwise no parent -> the oldest
            // project root when one exists (empty project: a new baseline).
            None if req.baseline => None,
            None => local::experiments::project_root(&store, &project.id)?,
        };
        let merge_parent = match &req.merge_parent_experiment_id {
            Some(mid) => Some(
                store
                    .get_local_experiment(mid)?
                    .ok_or_else(|| not_found("merge experiment"))?,
            ),
            None => None,
        };
        let (experiment, merge_warning) = local::experiments::create_experiment(
            &store,
            &project,
            parent.as_ref(),
            req.slug.as_deref(),
            req.title,
            req.description,
            req.run_command,
            merge_parent.as_ref(),
        )
        .map_err(bad_request)?;
        // Playable from the moment the card exists (game projects): build
        // the fork-point code now; the agent's edits rebuild via Play.
        local::play::auto_build(&store, &project, &experiment);
        Ok::<_, ApiError>((experiment, merge_warning))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("branch task failed: {e}")))??;
    let (experiment, merge_warning) = experiment;
    Ok(Json(
        json!({ "experiment": experiment, "mergeWarning": merge_warning }),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RunReq {
    /// job (default) | sim — see ExpRunArgs::kind.
    kind: Option<String>,
    backend: Option<String>,
    flavor: Option<String>,
    /// Repo-relative manifest path (k8s only; default .orx/k8s.yaml).
    manifest: Option<String>,
    timeout: Option<String>,
    /// ssh config host alias of the Slurm login node (slurm only; defaults to
    /// the slurm settings' host).
    host: Option<String>,
    /// Org to bill the box to (openresearch only; omit = the sole org).
    org: Option<String>,
}

async fn run_experiment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let project_id = store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?
        .project_id;
    let _admission = state
        .project_lifecycle
        .admit(&project_id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    store
        .get_local_project(&project_id)?
        .ok_or_else(|| not_found("project"))?;
    // Tolerate an empty body — every field is optional in the schema.
    let req: RunReq = if body.is_empty() {
        RunReq::default()
    } else {
        serde_json::from_slice(&body).map_err(bad_request)?
    };
    // Resolve the persisted default target (Settings → Compute) when the
    // request doesn't name a backend; `hf` stays the last-resort fallback so
    // existing clients keep their historical behavior when no default is set.
    // Empty strings mean "unset", matching the /compute/default endpoint.
    let mut backend_opt = req.backend.filter(|b| !b.trim().is_empty());
    let mut flavor = req.flavor.filter(|f| !f.trim().is_empty());
    local::apply_compute_default(&mut backend_opt, &mut flavor);
    let backend = backend_opt.unwrap_or_else(|| "hf".to_string());
    // Typed runs are local-only in Phase 1 — the metrics/artifacts contract
    // (env vars + supervisor ingestion) only exists on the local backend.
    if matches!(req.kind.as_deref(), Some(k) if k != "job") && backend != "local" {
        return Err(bad_request(format!(
            "kind '{}' runs require the local backend (got '{}')",
            req.kind.as_deref().unwrap_or_default(),
            backend
        )));
    }
    let args = crate::ExpRunArgs {
        exp_id: id,
        kind: req.kind,
        gpu: None,
        count: None,
        disk: None,
        provider: None,
        cpu: None,
        vcpus: None,
        sandbox: None,
        backend: Some(backend.clone()),
        flavor,
        org: req.org,
        host: req.host,
        manifest: req.manifest,
        image: None,
        timeout: req.timeout,
        force: false,
    };
    // Same code paths as CLI `orx exp run --backend <b>` on a local experiment.
    let run = tokio::spawn(async move {
        let _admission = _admission;
        match backend.as_str() {
            "hf" => local::hf::submit_local_hf(&args).await,
            "modal" => local::modal::submit_local_modal(&args).await,
            "k8s" => local::k8s::submit_local_k8s(&args).await,
            "ssh" => local::ssh::submit_local_ssh(&args).await,
            "slurm" => local::slurm::submit_local_slurm(&args).await,
            "ray" => local::ray::submit_local_ray(&args).await,
            "openresearch" => local::openresearch::submit_local_openresearch(&args).await,
            "local" => local::localrun::submit_local_run(&args).await,
            other => Err(anyhow!(
                "Unknown backend '{other}'. Supported: local, hf, modal, k8s, ssh, slurm, ray, openresearch."
            )),
        }
    })
    .await
    .map_err(|error| bad_request(format!("run launch task failed: {error}")))?
    .map_err(bad_request)?;
    Ok(Json(json!({ "run": ApiRun::from(&run) })))
}

async fn cancel_run(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    let run = local::local_run(&store, &id)?.ok_or_else(|| not_found("run"))?;
    // A terminal run must not gain a stale cancel_requested flag.
    if is_terminal(&run.status) {
        return Err(bad_request(format!("run already {}", run.status)));
    }
    // No supervisor polls a play session — cancel means "end it now".
    if run.kind == "play-session" {
        store.update_status(&run.id, "cancelled", Some(now_ms()), None)?;
        return Ok(Json(json!({ "ok": true })));
    }
    store.set_cancel_requested(&run.id, true)?;
    Ok(Json(json!({ "ok": true })))
}

// --- verdicts & metrics ----------------------------------------------------

#[derive(Deserialize)]
struct VerdictReq {
    /// keep | kill | iterate; null/absent clears the verdict.
    verdict: Option<String>,
    notes: Option<String>,
}

/// Validate a verdict string against the fixed vocabulary.
fn parse_verdict(v: &Option<String>) -> std::result::Result<Option<&str>, ApiError> {
    match v.as_deref() {
        None | Some("") => Ok(None),
        Some(v @ ("keep" | "kill" | "iterate")) => Ok(Some(v)),
        Some(other) => Err(bad_request(format!(
            "invalid verdict '{other}': expected keep, kill, or iterate"
        ))),
    }
}

async fn set_run_verdict(Path(id): Path<String>, Json(req): Json<VerdictReq>) -> ApiResult {
    let verdict = parse_verdict(&req.verdict)?;
    let notes = req
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let store = Store::open()?;
    let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    store.set_run_verdict(&run.id, verdict, notes)?;
    let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    Ok(Json(json!({ "run": ApiRun::from(&run) })))
}

async fn set_exp_verdict(Path(id): Path<String>, Json(req): Json<VerdictReq>) -> ApiResult {
    let verdict = parse_verdict(&req.verdict)?;
    let notes = req
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let store = Store::open()?;
    store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    store.set_experiment_verdict(&id, verdict, notes)?;
    let exp = store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    Ok(Json(json!({ "experiment": exp })))
}

/// The run's full ingested metrics document (list payloads carry only the
/// small `aggregate` slice).
async fn run_metrics(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    let metrics: Option<Value> = run
        .metrics_json
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok());
    Ok(Json(json!({ "metrics": metrics })))
}

// --- play builds -----------------------------------------------------------

/// Build (or reuse) the experiment's playable — thin wrapper over
/// `local::play::start_play_build` (shared with the auto-build on experiment
/// creation). Idempotent: an in-flight build is returned as-is, and a
/// finished build at the current head short-circuits to "ready".
async fn play_build(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    // Shells out to git — off the async workers.
    let result =
        tokio::task::spawn_blocking(move || -> Result<(String, local::play::PlayBuild)> {
            let store = Store::open()?;
            let exp = store
                .get_local_experiment(&id)?
                .ok_or_else(|| anyhow!("experiment not found"))?;
            let project = store
                .get_local_project(&exp.project_id)?
                .ok_or_else(|| anyhow!("project not found"))?;
            let url = format!("/play/{}/", exp.id);
            let build = local::play::start_play_build(&store, &project, &exp).map_err(|e| {
                if e.to_string().contains("no commits") {
                    e
                } else {
                    anyhow!("play build failed to start: {e}")
                }
            })?;
            Ok((url, build))
        })
        .await
        .map_err(|e| ApiError::from(anyhow!("build task failed: {e}")))?
        .map_err(bad_request)?;
    let (url, build) = result;
    Ok(Json(match build {
        local::play::PlayBuild::Building(run_id) => {
            json!({ "state": "building", "runId": run_id, "url": url })
        }
        local::play::PlayBuild::Ready => json!({ "state": "ready", "url": url }),
        local::play::PlayBuild::Started(run_id) => {
            json!({ "state": "building", "runId": run_id, "url": url })
        }
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateExperimentReq {
    /// Present-vs-absent: absent = leave, null/empty = clear.
    #[serde(default, deserialize_with = "double_option")]
    play_entry: Option<Option<String>>,
}

/// A play entry is a build-relative path plus optional query
/// (`gambit-slots.html?x=1`) — never absolute, never traversing.
fn validate_play_entry(entry: &str) -> std::result::Result<(), ApiError> {
    let path = entry.split(['?', '#']).next().unwrap_or("");
    let p = std::path::Path::new(path);
    if entry.len() > 512
        || path.is_empty()
        || p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(bad_request(
            "playEntry must be a relative path like game.html or game.html?screen=waves",
        ));
    }
    Ok(())
}

async fn update_experiment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateExperimentReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let mut exp = store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    if let Some(entry) = req.play_entry {
        let entry = entry
            .map(|e| e.trim().trim_start_matches('/').to_string())
            .filter(|e| !e.is_empty());
        if let Some(e) = &entry {
            validate_play_entry(e)?;
        }
        exp.play_entry = entry;
        store.update_local_experiment(&exp)?;
    }
    let exp = store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    Ok(Json(json!({ "experiment": exp })))
}

/// A session with no heartbeat for this long is over — generous enough that
/// a detour to the Logs tab (which pauses beats) doesn't clip a live session.
const PLAY_SESSION_STALE_MS: i64 = 180_000;

/// Start-or-keep-alive for the experiment's play session. The play tab calls
/// this every 10s while mounted: an open session gets its heartbeat touched;
/// none open starts one, stamped with the served build's commit and entry
/// page. One open session per experiment.
async fn play_session_beat(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let exp = store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    let runs = store.list_runs_by_experiment(&exp.id)?;
    if let Some(open) = runs
        .iter()
        .find(|r| r.kind == "play-session" && !is_terminal(&r.status))
    {
        store.touch_supervisor(&open.id)?;
        let run = store.get_run(&open.id)?.ok_or_else(|| not_found("run"))?;
        return Ok(Json(json!({ "run": ApiRun::from(&run) })));
    }
    // Sessions attach to a served build — its commit is the provenance.
    let sha = runs
        .iter()
        .find(|r| r.kind == "play" && r.status == "done")
        .and_then(|r| r.commit_sha.clone())
        .ok_or_else(|| bad_request("no playable build to attach a session to"))?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let run = StoredRun {
        id: run_id.clone(),
        experiment_id: exp.id.clone(),
        project_id: exp.project_id.clone(),
        status: "running".to_string(),
        backend_json: json!({ "kind": "play_session", "entry": exp.play_entry }).to_string(),
        command: String::new(),
        created_at: now_ms(),
        updated_at: now_ms(),
        ended_at: None,
        exit_code: None,
        commit_sha: Some(sha),
        result_markdown: None,
        cancel_requested: false,
        supervisor_heartbeat_ms: None,
        kind: "play-session".to_string(),
        metrics_json: None,
        verdict: None,
        verdict_notes: None,
        verdict_at: None,
        // Dashboard-initiated, so no launching chat session to notify.
        chat_session_id: None,
    };
    store.upsert_run(&run)?;
    store.touch_supervisor(&run_id)?;
    let run = store.get_run(&run_id)?.ok_or_else(|| not_found("run"))?;
    Ok(Json(json!({ "run": ApiRun::from(&run) })))
}

/// End the experiment's open play session (tab closed, or a verdict given —
/// the verdict lands on the session run). No open session is a no-op, so the
/// tab-close path can fire blind.
async fn play_session_end(Path(id): Path<String>, Json(req): Json<VerdictReq>) -> ApiResult {
    let verdict = parse_verdict(&req.verdict)?;
    let notes = req
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let store = Store::open()?;
    store
        .get_local_experiment(&id)?
        .ok_or_else(|| not_found("experiment"))?;
    let open = store
        .list_runs_by_experiment(&id)?
        .into_iter()
        .find(|r| r.kind == "play-session" && !is_terminal(&r.status));
    let Some(open) = open else {
        return Ok(Json(json!({ "run": Value::Null })));
    };
    store.update_status(&open.id, "done", Some(now_ms()), None)?;
    if verdict.is_some() || notes.is_some() {
        store.set_run_verdict(&open.id, verdict, notes)?;
    }
    let run = store.get_run(&open.id)?.ok_or_else(|| not_found("run"))?;
    Ok(Json(json!({ "run": ApiRun::from(&run) })))
}

/// Cap on a session's events.jsonl — beyond it, events drop (counted in the
/// response) rather than growing unbounded.
const SESSION_EVENTS_MAX_BYTES: u64 = 5_000_000;

#[derive(Deserialize)]
struct SessionEventsReq {
    events: Vec<Value>,
}

/// Telemetry from the playable into the open play session: events append to
/// `events.jsonl` in the session run's artifacts dir (so they list and serve
/// through the normal artifact machinery). Fire-and-forget contract: no open
/// session (or a full file) drops events without erroring — the game must
/// never break because the ledger isn't listening.
async fn play_session_events(
    Path(id): Path<String>,
    Json(req): Json<SessionEventsReq>,
) -> ApiResult {
    if req.events.is_empty() || req.events.len() > 500 {
        return Err(bad_request("expected 1..=500 events"));
    }
    blocking_api(move || {
        let store = Store::open()?;
        store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let open = store
            .list_runs_by_experiment(&id)?
            .into_iter()
            .find(|r| r.kind == "play-session" && !is_terminal(&r.status));
        let Some(open) = open else {
            return Ok(Json(
                json!({ "ok": true, "run": Value::Null, "dropped": true }),
            ));
        };
        let dir = crate::store::run_artifacts_dir(&open.id);
        std::fs::create_dir_all(&dir).map_err(|e| anyhow!("create {}: {e}", dir.display()))?;
        let path = dir.join("events.jsonl");
        let existing = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if existing >= SESSION_EVENTS_MAX_BYTES {
            return Ok(Json(json!({ "ok": true, "run": open.id, "dropped": true })));
        }
        let now = now_ms();
        let mut out = String::new();
        for ev in &req.events {
            // Stamp receive time; the event's own payload rides untouched.
            out.push_str(&json!({ "at": now, "event": ev }).to_string());
            out.push('\n');
        }
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(out.as_bytes()))
            .map_err(|e| anyhow!("append events: {e}"))?;
        Ok(Json(json!({ "ok": true, "run": open.id })))
    })
    .await
}

async fn play_root(Path(id): Path<String>) -> Response {
    axum::response::Redirect::permanent(&format!("/play/{id}/")).into_response()
}

async fn play_index(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    serve_play_path(state, id, String::new()).await
}

async fn play_file(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
) -> Response {
    serve_play_path(state, id, path).await
}

/// A minimal holding page while a build is in flight (auto-refreshes) or
/// after a failure (points at the run's logs in the dashboard).
fn play_holding_page(exp_name: &str, state: &str) -> Response {
    let (title, body, refresh) = match state {
        "building" => (
            "Building…",
            "The playable build is running. This page refreshes until it's ready.",
            true,
        ),
        "failed" => (
            "Build failed",
            "The last build failed — check the run's logs in the dashboard, then press Play again.",
            false,
        ),
        _ => (
            "No build yet",
            "No playable build exists for this experiment — press Play in the dashboard to start one.",
            false,
        ),
    };
    let meta = if refresh {
        r#"<meta http-equiv="refresh" content="3">"#
    } else {
        ""
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>{title}</title>\
         <style>body{{font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0;color:#333}}\
         main{{text-align:center}}</style></head>\
         <body><main><h1>{title}</h1><p>{exp_name}</p><p>{body}</p></main></body></html>"
    );
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-cache")],
        Html(html),
    )
        .into_response()
}

async fn serve_play_path(_state: AppState, exp_id: String, rel: String) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<Response> {
        let store = Store::open()?;
        let Some(exp) = store.get_local_experiment(&exp_id)? else {
            return Ok((StatusCode::NOT_FOUND, "experiment not found").into_response());
        };
        let Some(project) = store.get_local_project(&exp.project_id)? else {
            return Ok((StatusCode::NOT_FOUND, "project not found").into_response());
        };
        let dir = local::play::play_serve_dir(&project, &exp.id);

        // No completed build (or its dir is gone): explain instead of 404ing.
        let play_runs: Vec<_> = store
            .list_runs_by_experiment(&exp.id)?
            .into_iter()
            .filter(|r| r.kind == "play")
            .collect();
        let has_build = play_runs.iter().any(|r| r.status == "done") && dir.is_dir();
        if !has_build {
            let state = if play_runs.iter().any(|r| !is_terminal(&r.status)) {
                "building"
            } else if play_runs.iter().any(|r| r.status == "failed") {
                "failed"
            } else {
                "none"
            };
            return Ok(play_holding_page(exp.display_name(), state));
        }

        // The stable root URL lands on the experiment's chosen entry page
        // (repos like gambit-arena have no index.html — game.html and harness
        // pages are the real entries).
        if rel.is_empty() {
            if let Some(entry) = exp
                .play_entry
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty())
            {
                return Ok(axum::response::Redirect::temporary(&format!(
                    "/play/{}/{}",
                    exp.id, entry
                ))
                .into_response());
            }
        }

        let root = std::fs::canonicalize(&dir).map_err(|e| anyhow!("play dir unavailable: {e}"))?;
        let rel = rel.trim_start_matches('/');
        let candidate = if rel.is_empty() {
            root.join("index.html")
        } else {
            root.join(rel)
        };
        // SPA fallback: unknown extensionless routes serve index.html.
        let resolved = match std::fs::canonicalize(&candidate) {
            Ok(p) if p.is_dir() => p.join("index.html"),
            Ok(p) => p,
            Err(_) if !rel.contains('.') => root.join("index.html"),
            Err(_) => return Ok((StatusCode::NOT_FOUND, "not found").into_response()),
        };
        let resolved = std::fs::canonicalize(&resolved)
            .map_err(|_| anyhow!("not found"))
            .unwrap_or(resolved);
        if !resolved.starts_with(&root) {
            return Ok((StatusCode::BAD_REQUEST, "path escapes build dir").into_response());
        }
        let bytes = match std::fs::read(&resolved) {
            Ok(b) => b,
            Err(_) => return Ok((StatusCode::NOT_FOUND, "not found").into_response()),
        };
        let name = resolved.to_string_lossy();
        let content_type = local::files::content_type_for_path(&name);
        // HTML swaps in place on rebuild — never cache it; hashed assets can
        // be cached briefly.
        let cache = if content_type.starts_with("text/html") {
            "no-cache"
        } else {
            "public, max-age=3600"
        };
        Ok((
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            bytes,
        )
            .into_response())
    })
    .await;
    match result {
        Ok(Ok(resp)) => resp,
        Ok(Err(err)) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

// --- run artifacts ---------------------------------------------------------

/// Cap on artifact listing entries (a run dumping thousands of files still
/// lists, truncated).
const ARTIFACTS_LIMIT: usize = 2_000;

async fn list_run_artifacts(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
        let root = crate::store::run_artifacts_dir(&id);
        let mut entries: Vec<(String, u64)> = Vec::new();
        collect_files(&root, &root, &mut entries);
        entries.sort();
        let truncated = entries.len() > ARTIFACTS_LIMIT;
        entries.truncate(ARTIFACTS_LIMIT);
        let artifacts: Vec<Value> = entries
            .iter()
            .map(|(path, size)| {
                json!({
                    "path": path,
                    "size": size,
                    "contentType": local::files::content_type_for_path(path),
                })
            })
            .collect();
        Ok(Json(
            json!({ "artifacts": artifacts, "truncated": truncated }),
        ))
    })
    .await
}

/// Recursive file walk, paths relative to `root`. Missing dir = empty list
/// (the run may simply never have written artifacts).
fn collect_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let (Ok(rel), Ok(meta)) = (path.strip_prefix(root), entry.metadata()) {
            out.push((rel.to_string_lossy().into_owned(), meta.len()));
        }
    }
}

async fn serve_run_artifact(
    Path(id): Path<String>,
    Query(q): Query<FilePathQuery>,
) -> std::result::Result<Response, ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
        let root = std::fs::canonicalize(crate::store::run_artifacts_dir(&id))
            .map_err(|_| not_found("artifact"))?;
        let full = std::fs::canonicalize(root.join(q.path.trim_start_matches('/')))
            .map_err(|_| not_found("artifact"))?;
        if !full.starts_with(&root) || full.is_dir() {
            return Err(bad_request("invalid artifact path"));
        }
        let bytes = std::fs::read(&full).map_err(|_| not_found("artifact"))?;
        let content_type = local::files::content_type_for_path(&full.to_string_lossy());
        Ok((
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response())
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("artifact task failed: {e}")))?
}

#[derive(Deserialize)]
struct LogQuery {
    offset: Option<u64>,
}

async fn run_log(Path(id): Path<String>, Query(q): Query<LogQuery>) -> ApiResult {
    let store = Store::open()?;
    store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    let offset = q.offset.unwrap_or(0);
    let chunk = read_log_from(&id, offset, 4_000_000);
    let next_offset = offset + chunk.len() as u64;
    Ok(Json(json!({
        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&chunk),
        "nextOffset": next_offset,
        "eof": next_offset >= log_size(&id),
    })))
}

// --- diffs ------------------------------------------------------------------
//
// Same payload shape as the OpenResearch api diff endpoints:
// `{diff, truncated, bytesRead, byteLimit}` with the raw unified-diff text.
// All of these shell out to git against the project's local clone, so they
// run on the blocking pool.

fn diff_json(d: local::git::DiffPayload) -> Value {
    json!({
        "diff": d.diff,
        "truncated": d.truncated,
        "bytesRead": d.bytes_read,
        "byteLimit": local::git::MAX_DIFF_BYTES,
    })
}

/// Off-worker helper for git-backed handlers.
async fn blocking_api<F>(f: F) -> ApiResult
where
    F: FnOnce() -> std::result::Result<Json<Value>, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

/// Cumulative diff of a run's commit vs its experiment's parent branch —
/// "everything this experiment changed as of this run".
async fn run_diff(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
        let sha = run
            .commit_sha
            .clone()
            .ok_or_else(|| bad_request("run has no commit to diff"))?;
        let exp = store
            .get_local_experiment(&run.experiment_id)?
            .ok_or_else(|| not_found("experiment"))?;
        let parent_id = exp
            .parent_experiment_id
            .ok_or_else(|| bad_request("baseline runs have no parent branch to diff against"))?;
        let parent = store
            .get_local_experiment(&parent_id)?
            .ok_or_else(|| not_found("parent experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let payload = local::git::diff_range(repo, &parent.branch_name, &sha)?;
        Ok(Json(diff_json(payload)))
    })
    .await
}

/// Commits on the experiment branch: child experiments list `parent..branch`,
/// the baseline lists the branch's recent history.
async fn experiment_commits(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let exp = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let commits = match &exp.parent_experiment_id {
            Some(pid) => {
                let parent = store
                    .get_local_experiment(pid)?
                    .ok_or_else(|| not_found("parent experiment"))?;
                local::git::list_commits_between(repo, &parent.branch_name, &exp.branch_name, 100)?
            }
            None => local::git::list_commits(repo, &exp.branch_name, 25)?,
        };
        let commits: Vec<Value> = commits
            .iter()
            .map(|c| json!({ "sha": c.sha, "subject": c.subject, "committedAt": c.committed_at }))
            .collect();
        Ok(Json(json!({ "commits": commits })))
    })
    .await
}

async fn experiment_commit_diff(Path((id, sha)): Path<(String, String)>) -> ApiResult {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() < 7 || sha.len() > 64 {
        return Err(bad_request("invalid commit sha"));
    }
    blocking_api(move || {
        let store = Store::open()?;
        let exp = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let payload = local::git::commit_diff(repo, &sha)?;
        Ok(Json(diff_json(payload)))
    })
    .await
}

/// Live uncommitted changes in the project's clone (the agent's working
/// tree), mapped back to the experiment whose branch is checked out.
async fn project_working_tree(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let (branch, payload) = local::git::working_tree_diff(repo)?;
        let experiment_id = match &branch {
            Some(b) => store
                .list_experiments_by_project(&project.id)?
                .into_iter()
                .find(|e| &e.branch_name == b)
                .map(|e| e.id),
            None => None,
        };
        Ok(Json(json!({
            "branch": branch,
            "experimentId": experiment_id,
            "diff": payload.diff,
            "truncated": payload.truncated,
        })))
    })
    .await
}

/// Live view of one chat session's private worktree — what the agent has
/// changed, before any run exists. Unlike `project_working_tree` (clone-scoped,
/// diffed against HEAD), the session worktree starts detached on the baseline
/// tip and the agent commits to experiment branches, so "what it changed" is
/// the working tree diffed against the merge-base of the baseline and HEAD; a
/// bare HEAD diff would hide every committed edit. Read-only throughout: no
/// index-touching (`git add -N`) that would mutate the agent's checkout.
///
/// A never-started session (worktree is lazy) or a pruned worktree degrades to
/// `resolve_checkout_root`'s clone fallback; we report `{ exists: false }`
/// rather than pass off the clone's contents as the session's work.
async fn session_worktree(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let session = store
            .get_chat_session(&id)?
            .ok_or_else(|| not_found("chat session"))?;
        let project = store
            .get_local_project(&session.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let (root, root_kind) = resolve_checkout_root(&store, &project, Some(&id))?;
        if root_kind != "worktree" {
            return Ok(Json(json!({ "exists": false })));
        }
        let branch = local::git::current_branch(&root);
        // Diff against the merge-base of the baseline tip and HEAD — the fork
        // point of the agent's work. Every step that can't resolve (missing
        // origin ref, unrelated histories, unborn HEAD) falls back to HEAD, so
        // the diff degrades to "uncommitted only" rather than erroring.
        let baseline = &project.baseline_branch;
        let remote_baseline = format!("origin/{baseline}");
        let base = local::git::merge_base(&root, &remote_baseline, "HEAD")?
            .unwrap_or_else(|| "HEAD".to_string());
        let files = local::git::changed_files(&root, &base)?;
        let payload = local::git::working_tree_diff_against(&root, Some(&base))?;
        Ok(Json(json!({
            "exists": true,
            "branch": branch,
            "baselineBranch": baseline,
            "baseSha": base,
            "files": files,
            "diff": diff_json(payload),
        })))
    })
    .await
}

/// Cap on file bytes served to the viewer (mirrors openresearch.sh).
const FILE_READ_LIMIT: u64 = 512_000;

/// Resolve which on-disk checkout answers a file/code request for a project.
///
/// The chat session's worktree is where the agent actually works, so it can
/// hold files the hub clone's checkout never sees. When `session_id` is given
/// it must be this project's session (the authorization boundary, which also
/// pins the worktree dir to a store-issued id); a missing worktree (pruned, or
/// never created) degrades to the clone rather than erroring, but any other
/// worktree failure is reported, not papered over. Returns the canonicalized
/// root and whether it is the `"worktree"` or the `"clone"`.
fn resolve_checkout_root(
    store: &Store,
    project: &local::model::LocalProject,
    session_id: Option<&str>,
) -> std::result::Result<(std::path::PathBuf, &'static str), ApiError> {
    let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
    let worktree = match session_id {
        Some(s) => {
            let session = store
                .get_chat_session(s)?
                .filter(|sess| sess.project_id == project.id)
                .ok_or_else(|| not_found("chat session"))?;
            let dir = local::git::session_worktree_path(
                &project.github_owner,
                &project.github_repo,
                &session.id,
            );
            match std::fs::canonicalize(&dir) {
                Ok(p) => Some(p),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(ApiError::from(anyhow!("session worktree unavailable: {e}"))),
            }
        }
        None => None,
    };
    match worktree {
        Some(r) => Ok((r, "worktree")),
        None => Ok((
            std::fs::canonicalize(&project.repo_path)
                .map_err(|e| ApiError::from(anyhow!("repo clone unavailable: {e}")))?,
            "clone",
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeTreeQuery {
    /// Branch to list the committed tree of; absent lists a live checkout.
    r#ref: Option<String>,
    /// Chat session whose worktree to list (the live view the Worktree tab's
    /// Files pane wants). Absent falls back to the hub clone's checkout.
    /// Mutually exclusive with `ref` — a committed tree has no live worktree.
    session_id: Option<String>,
}

/// Cap on entries returned by the code-tree listing.
const CODE_TREE_LIMIT: usize = 20_000;

/// Flat file listing for the UI code browser. With `ref`: the committed tree
/// of that branch (local ref first, then origin's), independent of any
/// checkout. Without: the hub clone's checkout via `git ls-files`, so
/// gitignored trees are excluded and untracked-but-new files are included.
/// Paths are repo-relative; the client builds the nested tree.
async fn project_code_tree(Path(id): Path<String>, Query(q): Query<CodeTreeQuery>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let ref_name = q.r#ref.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let session_id = q
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if ref_name.is_some() && session_id.is_some() {
            return Err(bad_request("ref and sessionId are mutually exclusive"));
        }
        // Branch refs live in the shared object DB — any checkout resolves them;
        // for a live listing the session's worktree is the live view when given
        // (its untracked files the clone never sees), else the hub clone.
        let (root, root_kind) = resolve_checkout_root(&store, &project, session_id)?;
        let (root_kind, branch, mut entries) = match ref_name {
            Some(name) => {
                let sha = local::git::resolve_branch_commit(&root, name)?
                    .ok_or_else(|| not_found("branch"))?;
                let entries = local::git::list_tree_files(&root, &sha)?;
                ("branch", Some(name.to_string()), entries)
            }
            None => {
                let branch = local::git::current_branch(&root);
                let entries = local::git::list_worktree_files(&root)?;
                (root_kind, branch, entries)
            }
        };
        entries.sort();
        // During a merge conflict `ls-files --cached` emits an unmerged path
        // once per stage — collapse to one entry (they'd be duplicate keys).
        entries.dedup();
        let truncated = entries.len() > CODE_TREE_LIMIT;
        entries.truncate(CODE_TREE_LIMIT);
        Ok(Json(json!({
            "root": root_kind,
            "branch": branch,
            "entries": entries,
            "truncated": truncated,
        })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFileQuery {
    path: String,
    /// Chat session whose worktree holds the file. Absent (or the worktree
    /// already pruned) falls back to the hub clone. Ignored when `ref` is given.
    session_id: Option<String>,
    /// Branch to read the committed file from, instead of a live checkout.
    r#ref: Option<String>,
}

/// One file for the UI file viewer. With `ref`: the committed content on that
/// branch (a streamed, capped `git cat-file` read), independent of any
/// checkout. Without: the project's checkout — the chat session's worktree
/// when `sessionId` is given, else the hub clone. Path is repo-relative;
/// traversal outside the checkout is rejected. The response's `root` says
/// which source actually answered, so the UI can flag fallback.
async fn project_file(Path(id): Path<String>, Query(q): Query<ProjectFileQuery>) -> ApiResult {
    blocking_api(move || {
        use std::io::Read as _;
        let rel = q.path.trim().trim_start_matches("./").to_string();
        if rel.is_empty() || rel.len() > 1024 {
            return Err(bad_request("invalid path"));
        }
        let rel_path = std::path::Path::new(&rel);
        let traversal = rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)));
        if traversal {
            return Err(bad_request("path must be repo-relative"));
        }

        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let ref_name = q.r#ref.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(name) = ref_name {
            let (root, _) = resolve_checkout_root(&store, &project, None)?;
            let sha = local::git::resolve_branch_commit(&root, name)?
                .ok_or_else(|| not_found("branch"))?;
            // Streamed + capped: a committed multi-GB blob must not become a
            // multi-GB allocation. Missing path is an exit-code check inside
            // the helper (`cat-file -e`) — no error-message parsing.
            return match local::git::file_at_capped(&root, &sha, &rel, FILE_READ_LIMIT)? {
                Some((content, truncated)) => Ok(Json(json!({
                    "path": rel, "content": content, "truncated": truncated,
                    "notFound": false, "root": "branch",
                }))),
                None => Ok(Json(json!({
                    "path": rel, "content": "", "truncated": false,
                    "notFound": true, "root": "branch",
                }))),
            };
        }
        let (root, root_kind) = resolve_checkout_root(&store, &project, q.session_id.as_deref())?;
        let not_found_json = json!({
            "path": rel, "content": "", "truncated": false, "notFound": true, "root": root_kind,
        });
        // Canonicalize so symlinks can't escape the checkout.
        let full = match std::fs::canonicalize(root.join(rel_path)) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Json(not_found_json)),
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        if !full.starts_with(&root) {
            return Err(bad_request("path escapes repository"));
        }
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        let file = match std::fs::File::open(&full) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Json(not_found_json)),
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        let mut buf = Vec::new();
        std::io::Read::take(file, FILE_READ_LIMIT + 1)
            .read_to_end(&mut buf)
            .map_err(|e| ApiError::from(anyhow!("read failed: {e}")))?;
        let truncated = buf.len() as u64 > FILE_READ_LIMIT;
        buf.truncate(FILE_READ_LIMIT as usize);
        Ok(Json(json!({
            "path": rel,
            "content": String::from_utf8_lossy(&buf).into_owned(),
            "truncated": truncated,
            "notFound": false,
            "root": root_kind,
        })))
    })
    .await
}

// --- files ----------------------------------------------------------------

/// Listing of the project's files dir — the filesystem is the source of
/// truth; this scans it fresh on every call (and creates it if missing).
/// Top-level folders named for an experiment slug carry that experiment
/// (title, branch, latest run status) so the tab can group by experiment.
async fn list_files(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let experiments = store.list_experiments_by_project(&id)?;
        // Newest-first run list → first status seen per experiment is latest.
        let mut latest: HashMap<String, String> = HashMap::new();
        for run in store.list_runs_by_project(&id)? {
            latest.entry(run.experiment_id).or_insert(run.status);
        }
        let listing = local::files::list(&project, &experiments, &latest)?;
        Ok(Json(json!(listing)))
    })
    .await
}

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

/// A report folder's markdown body (`<name>/report.md`).
async fn file_report(Path(id): Path<String>, Query(q): Query<FilePathQuery>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let markdown = local::files::read_report_markdown(&project, &q.path)
            .map_err(|_| not_found("report"))?;
        Ok(Json(json!({ "markdown": markdown })))
    })
    .await
}

/// Delete a file or report folder in the files dir, by relative path.
async fn delete_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FilePathQuery>,
) -> ApiResult {
    reject_if_moving(&state)?;
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        local::files::delete_entry(&project, &q.path)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Raw file bytes, by files-dir-relative path. `no-cache`: the same path can
/// be rewritten in place on disk.
async fn serve_file(
    Path(id): Path<String>,
    Query(q): Query<FilePathQuery>,
) -> std::result::Result<Response, ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let bytes = local::files::read_file(&project, &q.path).map_err(|_| not_found("file"))?;
        let content_type = local::files::content_type_for_path(&q.path);
        Ok((
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response())
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))?
}

// --- HF token settings ------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HfSettings {
    configured: bool,
    source: Option<&'static str>,
    masked_token: Option<String>,
    valid: bool,
    username: Option<String>,
    jobs_write: Option<bool>,
}

/// Never the full token: first 3 chars + ellipsis + last 4.
fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 8 {
        return "…".to_string();
    }
    format!(
        "{}…{}",
        chars[..3].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

/// Re-resolve the token and check it against whoami-v2. Uncached — cheap, and
/// the UI calls it rarely.
async fn hf_token_status() -> HfSettings {
    use crate::jobs::huggingface::{self, TokenSource};
    let Ok((token, source)) = huggingface::resolve_token_with_source() else {
        return HfSettings {
            configured: false,
            source: None,
            masked_token: None,
            valid: false,
            username: None,
            jobs_write: None,
        };
    };
    let source = match source {
        TokenSource::Env => "env",
        TokenSource::OpenresearchEnv => "openresearchEnv",
        TokenSource::HfCache => "hfCache",
    };
    let details = huggingface::whoami_details(&token).await.ok();
    HfSettings {
        configured: true,
        source: Some(source),
        masked_token: Some(mask_token(&token)),
        valid: details.is_some(),
        username: details.as_ref().map(|d| d.name.clone()),
        jobs_write: details.and_then(|d| d.jobs_write),
    }
}

async fn hf_settings() -> Json<Value> {
    Json(json!(hf_token_status().await))
}

#[derive(Deserialize)]
struct SetHfTokenReq {
    token: String,
}

async fn set_hf_token(Json(req): Json<SetHfTokenReq>) -> ApiResult {
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(bad_request("token is required"));
    }
    crate::jobs::huggingface::whoami_details(&token)
        .await
        .map_err(bad_request)?;
    tokio::task::spawn_blocking(move || crate::config::write_synced_env_var("HF_TOKEN", &token))
        .await
        .map_err(|e| anyhow!("env write task failed: {e}"))??;
    // Freshly re-resolved: if HF_TOKEN is set in this process env, env still
    // wins over the file — source says "env" and the UI explains it.
    Ok(Json(json!(hf_token_status().await)))
}

/// Startup warning when HF Jobs can't launch as-is. Never blocks startup.
fn spawn_hf_preflight() {
    tokio::spawn(async {
        let s = hf_token_status().await;
        let fix = "run `hf auth login` with a Jobs-write token, or set one in the dashboard Settings page";
        if !s.configured {
            eprintln!("orx up: warning: no Hugging Face token found — runs can't launch; {fix}.");
        } else if !s.valid {
            eprintln!(
                "orx up: warning: the Hugging Face token ({}) was rejected by huggingface.co; {fix}.",
                s.source.unwrap_or("unknown source")
            );
        } else if s.jobs_write == Some(false) {
            eprintln!(
                "orx up: warning: the Hugging Face token ({}) lacks Jobs write access — launches will fail; {fix}.",
                s.source.unwrap_or("unknown source")
            );
        }
    });
}

/// Startup summary of detected coding agents + git/GitHub credentials, so a
/// first-time CLI user learns autodetection happened at all. Never blocks.
fn spawn_agent_git_preflight() {
    tokio::spawn(async {
        let harnesses = local::harness::detect_harnesses().await;
        let line: Vec<String> = harnesses
            .iter()
            .map(|h| {
                if h.agent_ready {
                    match &h.account {
                        Some(acct) => format!("{} ✓ ({acct})", h.name),
                        None => format!("{} ✓", h.name),
                    }
                } else if h.installed {
                    format!("{} — not signed in", h.name)
                } else {
                    format!("{} — not installed", h.name)
                }
            })
            .collect();
        eprintln!("orx up: agents: {}", line.join(" · "));
        if !harnesses.iter().any(|h| h.agent_ready) {
            eprintln!(
                "orx up: warning: no coding agent detected — install Claude Code, Codex or opencode and sign in to chat in the dashboard."
            );
        }
        // git checks shell out; keep them off the async workers.
        let _ = tokio::task::spawn_blocking(|| {
            let git = git_settings_json();
            if git.get("gitVersion").and_then(Value::as_str).is_none() {
                eprintln!("orx up: warning: git not found on PATH — cloning projects will fail.");
            } else if git.get("githubTokenSource").and_then(Value::as_str).is_none() {
                eprintln!(
                    "orx up: note: no GitHub credentials found (`gh auth login` or GITHUB_TOKEN) — private-repo clones and experiment-branch pushes need them unless SSH keys are set up."
                );
            }
        })
        .await;
    });
}

/// A signed-out harness is the only state that needs polling. Normal turns and
/// healthy idle sessions do no auth work; this loop merely notices a login the
/// user completed separately and wakes the UI immediately.
fn spawn_claude_auth_monitor(
    chat: Arc<ChatHost>,
    claude: Arc<local::claude::ClaudeHost>,
    harnesses: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>>,
) {
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(5);
        let mut observed_generation = claude.auth_snapshot().generation;
        loop {
            tokio::time::sleep(delay).await;
            let before = claude.auth_snapshot();
            if before.generation != observed_generation {
                observed_generation = before.generation;
                *harnesses.lock().await = None;
                if claude.claim_auth_announcement(before.generation) {
                    chat.emit_event(
                        "harness.auth",
                        json!({ "harness": "claude-code", "authState": before.state }),
                    );
                }
            }
            if before.runtime_rejected
                || matches!(
                    before.state,
                    local::harness::HarnessAuthState::Ready
                        | local::harness::HarnessAuthState::Unsupported
                )
            {
                delay = Duration::from_secs(5);
                continue;
            }
            let state = local::harness::claude::current_auth_state().await;
            claude.observe_auth_state(state, before.generation);
            let after = claude.auth_snapshot();
            if after.generation != observed_generation {
                observed_generation = after.generation;
                *harnesses.lock().await = None;
                if claude.claim_auth_announcement(after.generation) {
                    chat.emit_event(
                        "harness.auth",
                        json!({ "harness": "claude-code", "authState": after.state }),
                    );
                }
            }
            delay = if state == local::harness::HarnessAuthState::Unknown {
                (delay * 2).min(Duration::from_secs(60))
            } else {
                Duration::from_secs(5)
            };
        }
    });
}

// --- modal settings -----------------------------------------------------------

use crate::jobs::modal;

fn modal_settings_json(s: &modal::ModalStatus) -> Value {
    json!({
        "envProvisioned": s.env_provisioned,
        "modalImportable": s.modal_importable,
        "tokenConfigured": s.token_configured,
        "tokenSource": s.token_source,
        "ready": s.modal_importable && s.token_configured,
        "error": s.error,
    })
}

async fn modal_settings() -> Json<Value> {
    Json(modal_settings_json(&modal::detect().await))
}

/// Build the orx-managed Modal env (first run downloads the SDK, ~30–60s), then
/// report status. Idempotent — a no-op once the env exists.
async fn provision_modal() -> ApiResult {
    modal::ensure_env().await.map_err(bad_request)?;
    Ok(Json(modal_settings_json(&modal::detect().await)))
}

// --- scenario (asset generation over MCP) --------------------------------------

use crate::local::scenario;

/// Connection state for the Settings card.
///
/// Verifies the login rather than just reporting the file's existence: a stored
/// token whose refresh has been redeemed elsewhere looks present but fails on
/// first use, and a card claiming "Connected" on the strength of a file on disk
/// would be wrong exactly when it matters. One MCP round trip, on a page that
/// isn't hot.
async fn scenario_settings_json() -> Value {
    let mut out = json!({
        "authPath": scenario::auth::auth_path().to_string_lossy(),
        // A login started from this dashboard that hasn't come back yet. Lets a
        // reloaded page rejoin a login in flight instead of offering Connect
        // again and being refused.
        "connecting": scenario::auth::login_in_progress(),
    });

    if !scenario::is_connected() {
        // "Not connected" and "cannot connect" are indistinguishable from here,
        // and only the second is a problem the user can't fix by clicking
        // Connect. Discovery needs no credentials, so answer it while we're here.
        let reachable = scenario::auth::discover().await;
        out["state"] = json!("disconnected");
        out["reachable"] = json!(reachable.is_ok());
        if let Err(e) = reachable {
            out["error"] = json!(e.to_string());
        }
        return out;
    }

    match scenario::list_tools().await {
        Ok(tools) => {
            out["state"] = json!("connected");
            out["toolCount"] = json!(tools.len());
        }
        // The one distinction the whole error split exists for: this is
        // "reconnect", not "something broke".
        Err(scenario::ScenarioError::NotConnected) => out["state"] = json!("expired"),
        Err(e) => {
            out["state"] = json!("error");
            out["error"] = json!(e.to_string());
        }
    }
    out
}

async fn scenario_settings() -> ApiResult {
    Ok(Json(scenario_settings_json().await))
}

/// Start a Scenario login and return immediately with the authorize URL.
///
/// The login itself finishes on a background task: a human takes up to five
/// minutes, and holding the response open for that would look like a hung request
/// and die to any proxy read timeout. Progress arrives on `/api/events` as
/// `scenario.connect.done` / `scenario.connect.error`.
async fn connect_scenario(State(state): State<AppState>) -> ApiResult {
    let port = state.chat.up_port().ok_or_else(|| {
        bad_request("this server's port is unknown, so the login redirect can't be built")
    })?;

    let login = scenario::auth::begin(scenario::auth::Redirect::Dashboard { port })
        .await
        .map_err(bad_request)?;
    let url = login.url.clone();

    // Over SSH the browser is on the user's machine, not this one, so opening one
    // here shows nobody anything — `up` skips it at startup for the same reason.
    // Report that we didn't, and the card shows the URL to open by hand.
    let opened = crate::remote::detect_ssh_session().is_none();
    if opened {
        browser::open_browser(&url);
    }

    let chat = state.chat.clone();
    tokio::spawn(async move {
        match login.finish().await {
            Ok(scopes) => chat.emit_event("scenario.connect.done", json!({ "scopes": scopes })),
            Err(e) => chat.emit_event("scenario.connect.error", json!({ "error": e.to_string() })),
        }
    });

    Ok(Json(json!({
        "started": true,
        "authorizeUrl": url,
        "browserOpened": opened,
    })))
}

async fn disconnect_scenario() -> ApiResult {
    scenario::auth::forget().map_err(bad_request)?;
    Ok(Json(scenario_settings_json().await))
}

// --- generative-ai providers ---------------------------------------------------

/// How long a Blender probe is reused.
///
/// Much shorter than `HARNESS_CACHE_TTL`: opening and closing Blender is something
/// the user does *while looking at this card*, so a minute-long cache would show
/// them a stale answer about a window they can see. Short enough to notice, long
/// enough that the `blender-mcp --help` subprocess isn't paid per poll.
const BLENDER_CACHE_TTL: Duration = Duration::from_secs(10);

/// Blender's card payload: the three-layer probe, plus what the managed server
/// advertises.
async fn blender_settings_json(state: &AppState) -> Value {
    let status = local::blender::detect().await;
    let mut out = serde_json::to_value(&status).unwrap_or_else(|_| json!({}));
    out["kind"] = json!("blender");
    out["id"] = json!("blender");
    out["name"] = json!("Blender");
    out["ready"] = json!(status.ready());

    // Whether *we* have a server running is a separate fact from whether one
    // could run: a repaired install still needs an `orx up` restart before the
    // child exists, and the card has to be able to say so.
    let server = state.blender.lock().await.clone();
    match server {
        Some(server) => {
            out["serverUrl"] = json!(server.url());
            out["serverRunning"] = json!(true);
            // Works with Blender closed — `tools/list` never touches the socket —
            // so this reports the server, not the session.
            match server.list_tools().await {
                Ok(tools) => out["toolCount"] = json!(tools.len()),
                Err(e) => out["serverError"] = json!(e.to_string()),
            }
        }
        None => out["serverRunning"] = json!(false),
    }
    out
}

/// Cached Blender payload, refreshed on `?refresh=1`.
async fn blender_settings_cached(state: &AppState, refresh: bool) -> Value {
    let mut slot = state.blender_status.lock().await;
    if !refresh {
        if let Some((at, cached)) = slot.as_ref() {
            if at.elapsed() < BLENDER_CACHE_TTL {
                return cached.clone();
            }
        }
    }
    let fresh = blender_settings_json(state).await;
    *slot = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

#[derive(Deserialize)]
struct RefreshQuery {
    refresh: Option<u8>,
}

/// Every generative-AI provider, one entry per Settings sub-tab.
///
/// A discriminated list rather than a `Harness`-style trait: Scenario is a remote
/// OAuth service verified by a live tool call, Blender is a local process plus a
/// socket, and they share almost no fields. A common trait would force a shape
/// neither one really has; `kind` lets the UI render each honestly.
async fn genai_settings(State(state): State<AppState>, Query(q): Query<RefreshQuery>) -> ApiResult {
    let refresh = q.refresh == Some(1);
    let mut scenario = scenario_settings_json().await;
    scenario["kind"] = json!("scenario");
    scenario["id"] = json!("scenario");
    scenario["name"] = json!("Scenario");
    scenario["ready"] = json!(scenario.get("state").and_then(Value::as_str) == Some("connected"));

    Ok(Json(json!({
        "providers": [
            scenario,
            blender_settings_cached(&state, refresh).await,
            comfyui_settings_cached(&state, refresh).await,
        ],
    })))
}

/// ComfyUI's card payload: the layered probe, plus the polled tool surface.
async fn comfyui_settings_json(state: &AppState) -> Value {
    let status = local::comfyui::detect().await;
    let mut out = serde_json::to_value(&status).unwrap_or_else(|_| json!({}));
    out["kind"] = json!("comfyui");
    out["id"] = json!("comfyui");
    out["name"] = json!("ComfyUI");
    out["ready"] = json!(status.ready());

    let server = state.comfyui.lock().await.clone();
    match server {
        Some(server) => {
            out["serverUrl"] = json!(server.url());
            out["serverRunning"] = json!(true);
            // Polled, never hard-coded: `comfyui-mcp` is a third-party package and
            // its surface moves. This also needs ComfyUI up — several of its tools
            // introspect the live install — so a failure here is reported without
            // contradicting `comfyReachable`.
            match server.list_tools().await {
                Ok(tools) => out["toolCount"] = json!(tools.len()),
                Err(e) => out["mcpError"] = json!(e.to_string()),
            }
        }
        None => out["serverRunning"] = json!(false),
    }
    out
}

async fn comfyui_settings_cached(state: &AppState, refresh: bool) -> Value {
    let mut slot = state.comfyui_status.lock().await;
    if !refresh {
        if let Some((at, cached)) = slot.as_ref() {
            if at.elapsed() < BLENDER_CACHE_TTL {
                return cached.clone();
            }
        }
    }
    let fresh = comfyui_settings_json(state).await;
    *slot = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// Start ComfyUI, or report that something already answers on its port.
///
/// Adopt-then-start, and slow by nature: ComfyUI imports torch and scans models.
/// Unlike the Scenario login this *does* hold the request open — there is no
/// browser step to hand back, the card shows a spinner, and the honest answer is
/// "it's up" or the reason it isn't.
async fn start_comfyui(State(state): State<AppState>) -> ApiResult {
    let started = local::comfyui::server::start_comfy()
        .await
        .map_err(bad_request)?;
    // The cached status is stale the moment this returns.
    *state.comfyui_status.lock().await = None;
    Ok(Json(json!({
        "started": started,
        "adopted": !started,
        "dashboardUrl": local::comfyui::dashboard_url(),
    })))
}

#[derive(Deserialize)]
struct ScenarioCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Where Scenario's OAuth server sends the user's browser after they sign in.
///
/// Mounted here rather than on a private loopback port so the remote modes work:
/// `up --remote` already forwards this port to the user's laptop, so the redirect
/// rides the tunnel that's there. The CLI keeps its own listener for the
/// no-server-running case.
async fn scenario_callback(Query(q): Query<ScenarioCallbackQuery>) -> Response {
    // Clerk sends the human-readable reason in `error_description`; `error` is the
    // machine code and the fallback.
    let reason = q.error_description.as_deref().or(q.error.as_deref());
    match scenario::auth::deliver_callback(q.state.as_deref(), q.code.as_deref(), reason) {
        Ok(message) => (StatusCode::OK, Html(callback_page(message))).into_response(),
        Err(message) => (StatusCode::BAD_REQUEST, Html(callback_page(&message))).into_response(),
    }
}

/// The one page the user sees in the tab Scenario redirected. Deliberately
/// dependency-free and self-contained — it renders in a tab that has no access to
/// the SPA's assets yet, and its only job is to say what happened.
fn callback_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Crux — Scenario</title>\
         <style>body{{font:15px/1.5 system-ui,sans-serif;margin:0;display:grid;\
         place-items:center;min-height:100vh;background:#111;color:#eee}}\
         p{{max-width:34rem;padding:0 1.5rem;text-align:center}}</style></head>\
         <body><p>{}</p></body></html>",
        html_escape(message)
    )
}

/// Minimal HTML text escaping. The message can carry a server-supplied error
/// string, so it does not go into the page unescaped.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// --- kubernetes settings ------------------------------------------------------

use crate::jobs::kubernetes as k8s;

/// One payload powers the whole settings card: stored config plus live
/// cluster health. Contexts come from the local kubeconfig. Resource shapes
/// live in each experiment's committed manifest, not in settings.
async fn k8s_settings_json() -> Value {
    let settings = k8s::load_settings().ok().flatten();
    let configured = settings.is_some();
    let settings = settings.unwrap_or_default();
    let (contexts, current) = k8s::list_contexts().await.unwrap_or((Vec::new(), None));
    let preflight = k8s::preflight(settings.context.as_deref(), &settings.namespace).await;
    json!({
        "configured": configured,
        "contexts": contexts,
        "currentContext": current,
        "context": settings.context,
        "namespace": settings.namespace,
        "preflight": preflight,
    })
}

async fn k8s_settings() -> ApiResult {
    Ok(Json(k8s_settings_json().await))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetK8sSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (kubectl default).
    context: Option<String>,
    namespace: Option<String>,
}

async fn set_k8s_settings(Json(req): Json<SetK8sSettingsReq>) -> ApiResult {
    let mut settings = k8s::load_settings()?.unwrap_or_default();
    if let Some(ctx) = req.context {
        settings.context = Some(ctx.trim().to_string()).filter(|c| !c.is_empty());
    }
    if let Some(ns) = req.namespace {
        let ns = ns.trim().to_string();
        settings.namespace = if ns.is_empty() {
            "default".to_string()
        } else {
            ns
        };
    }
    k8s::save_settings(&settings)?;
    Ok(Json(k8s_settings_json().await))
}

/// Startup warning when a configured k8s backend can't launch as-is. Silent
/// when k8s was never configured — HF remains the default backend.
fn spawn_k8s_preflight() {
    tokio::spawn(async {
        let Ok(Some(settings)) = k8s::load_settings() else {
            return;
        };
        let p = k8s::preflight(settings.context.as_deref(), &settings.namespace).await;
        let fix = "check the cluster in the dashboard Settings → Compute page";
        if !p.kubectl_found {
            eprintln!("orx up: warning: kubectl not found on PATH — k8s runs can't launch.");
        } else if !p.reachable {
            eprintln!(
                "orx up: warning: Kubernetes cluster unreachable ({}) — k8s runs can't launch; {fix}.",
                p.error.unwrap_or_default()
            );
        } else if !p.can_create_jobs {
            eprintln!(
                "orx up: warning: no permission to create Jobs in namespace '{}' — k8s runs will fail; {fix}.",
                settings.namespace
            );
        }
    });
}

// --- env var settings -------------------------------------------------------

/// Everything in `~/.openresearch/env`, values masked. `inProcessEnv` flags
/// keys that are also set in orx up's own environment (which wins at runtime).
fn env_settings_json() -> Value {
    let vars: Vec<Value> = crate::config::list_synced_env()
        .iter()
        .map(|(key, value)| {
            json!({
                "key": key,
                "maskedValue": mask_token(value),
                "inProcessEnv": std::env::var_os(key).is_some(),
            })
        })
        .collect();
    json!({ "vars": vars })
}

async fn env_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(env_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetEnvVarReq {
    key: String,
    value: String,
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

async fn set_env_var(Json(req): Json<SetEnvVarReq>) -> ApiResult {
    let key = req.key.trim().to_string();
    let value = req.value.trim().to_string();
    if !valid_env_key(&key) {
        return Err(bad_request(
            "key must be letters, digits or _, not starting with a digit",
        ));
    }
    if value.is_empty() {
        return Err(bad_request("value is required"));
    }
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_var(&key, &value)?;
        Ok(Json(env_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

async fn delete_env_var(Path(key): Path<String>) -> ApiResult {
    if !valid_env_key(&key) {
        return Err(bad_request("invalid key"));
    }
    tokio::task::spawn_blocking(move || {
        crate::config::remove_synced_env_var(&key)?;
        Ok(Json(env_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

// --- data directory ---------------------------------------------------------

/// Current data-dir state for the Storage settings card: where it resolves,
/// whether that's the default, the path we'd fall back to, and *why* it resolves
/// where it does (so the UI can lock the field when `$ORX_DATA_DIR` forces it).
fn data_dir_json() -> Value {
    use crate::store::DataDirSource;
    let current = crate::store::data_dir();
    let default = crate::store::default_data_dir();
    let source = crate::store::data_dir_source();
    json!({
        "current": current.to_string_lossy(),
        "defaultPath": default.to_string_lossy(),
        // "On the fallback chain" = no explicit choice (env pin or saved config).
        // Env can happen to equal the default path but is still a forced override.
        "isDefault": matches!(source, DataDirSource::Xdg | DataDirSource::Default),
        // env | config | xdg | default — env means a forced override (read-only).
        "source": source,
    })
}

/// The signed-in GitHub login, so "creates github.com/you/x" can name the real
/// account. `null` when there's no usable token — the UI keeps saying "you".
async fn github_account() -> ApiResult {
    Ok(Json(
        json!({ "login": local::github::viewer_login().await }),
    ))
}

#[derive(Deserialize)]
struct RepoAccessQuery {
    owner: String,
    repo: String,
}

/// Whether the stored credentials can push to `owner/repo`, so the New project
/// form can drop the fork choice when it isn't one. Mirrors the create path:
/// unknown (no token / API hiccup) counts as access, so the UI never nags about
/// a fork the server wouldn't make.
async fn github_repo_access(Query(q): Query<RepoAccessQuery>) -> ApiResult {
    let owner = q.owner.trim().to_string();
    let repo = q.repo.trim().to_string();
    if owner.is_empty() || repo.is_empty() {
        return Err(bad_request("owner and repo are required"));
    }
    let meta = local::github::repo_meta(&owner, &repo).await;
    Ok(Json(json!({
        "canPush": meta.map(|m| m.can_push).unwrap_or(true),
    })))
}

async fn data_dir_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(data_dir_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("data-dir task failed: {e}")))?
}

#[derive(Deserialize)]
struct DataDirReq {
    path: String,
}

/// Reject a mutation when `$ORX_DATA_DIR` is forcing the path — the config value
/// would be shadowed, so honoring the request would silently do nothing.
fn ensure_not_env_forced() -> std::result::Result<(), ApiError> {
    if crate::store::data_dir_source() == crate::store::DataDirSource::Env {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The data directory is pinned by the ORX_DATA_DIR environment \
             variable, which overrides this setting. Unset it to choose a path here."
                .into(),
        ));
    }
    Ok(())
}

/// Pre-flight a candidate path for a **move** without committing: absolute? empty
/// target? room? Returns `{ ok, error?, treeBytes, freeBytes?, sameFilesystem }`.
async fn validate_data_dir(Json(req): Json<DataDirReq>) -> ApiResult {
    use crate::local::datadir::TargetIntent;
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(bad_request("path is required"));
    }
    tokio::task::spawn_blocking(move || {
        match crate::local::datadir::validate_target(
            std::path::Path::new(&path),
            TargetIntent::Move,
        ) {
            Ok(report) => Ok(Json(json!({
                "ok": true,
                "treeBytes": report.tree_bytes,
                "freeBytes": report.free_bytes,
                "sameFilesystem": report.same_filesystem,
            }))),
            Err(e) => Ok(Json(json!({ "ok": false, "error": e.to_string() }))),
        }
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("validate task failed: {e}")))?
}

/// Set the data dir *without moving* — for onboarding on an empty install, or
/// reconnecting to an already-populated location (a second machine, after config
/// loss). The UI routes here only when the current dir has nothing to migrate;
/// otherwise it calls `/move`. Uses `TargetIntent::Set`, which (unlike `Move`)
/// permits a populated existing dir since nothing is copied.
async fn set_data_dir(State(state): State<AppState>, Json(req): Json<DataDirReq>) -> ApiResult {
    use crate::local::datadir::TargetIntent;
    reject_if_moving(&state)?;
    ensure_not_env_forced()?;
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(bad_request("path is required"));
    }
    // Validate before persisting so we never store a bad path.
    let validate_path = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::local::datadir::validate_target(
            std::path::Path::new(&validate_path),
            TargetIntent::Set,
        )
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("validate task failed: {e}")))?
    .map_err(bad_request)?;

    tokio::task::spawn_blocking(move || crate::config::set_settings_data_dir(Some(path)))
        .await
        .map_err(|e| ApiError::from(anyhow!("settings task failed: {e}")))??;
    Ok(Json(data_dir_json()))
}

/// Relocate the data dir to `path`, streaming `datadir.move.*` progress events
/// over `/api/events`. Returns 202 immediately; the UI watches the SSE stream.
///
/// Concurrency safety: sets `data_dir_move_in_progress` *first*, then refuses
/// (409) if a run or chat turn is already active. The substantive store-mutating
/// handlers (`send`/`launch`, project/experiment/chat CRUD, file delete,
/// `set_data_dir`) check the flag on entry and back off, so once the move is
/// underway nothing new writes the store. Even the residual races don't lose
/// data: a request that passed its own flag check in the tiny window before this
/// one set the flag — or an unguarded incidental write (an `open_project`
/// timestamp touch, an `ssh_preflight` test row) — lands in the *old* dir, but
/// the cross-filesystem path never deletes it (it's returned as `oldPathLeft`),
/// so the write is preserved there; only the atomic same-filesystem rename
/// consumes the old dir, and that path has no copy window.
async fn move_data_dir(State(state): State<AppState>, Json(req): Json<DataDirReq>) -> Response {
    use crate::local::datadir::TargetIntent;
    use std::sync::atomic::Ordering;

    if let Err(e) = ensure_not_env_forced() {
        return e.into_response();
    }
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return bad_request("path is required").into_response();
    }

    // Claim the move slot first (compare-exchange): only one move at a time, and
    // once claimed, new turns/launches see the flag and back off.
    if state
        .data_dir_move_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return ApiError(
            StatusCode::CONFLICT,
            "A data-directory move is already in progress.".into(),
        )
        .into_response();
    }

    // Helper to release the slot on any early return.
    let release = |state: &AppState| {
        state
            .data_dir_move_in_progress
            .store(false, Ordering::SeqCst);
    };

    // In-flight guard: block if a chat turn or a run is active right now. (The
    // flag we just set prevents *new* ones from starting past this point.)
    let busy = state.chat.busy_sessions().await;
    let active_runs = tokio::task::spawn_blocking(active_run_count)
        .await
        .unwrap_or(0);
    if !busy.is_empty() || active_runs > 0 {
        release(&state);
        return ApiError(
            StatusCode::CONFLICT,
            format!(
                "Can't move while work is in progress ({} active chat turn(s), \
                 {active_runs} active run(s)). Finish or stop them, then retry.",
                busy.len()
            ),
        )
        .into_response();
    }

    // Validate before kicking off the background move.
    let vpath = path.clone();
    let validated = tokio::task::spawn_blocking(move || {
        crate::local::datadir::validate_target(std::path::Path::new(&vpath), TargetIntent::Move)
    })
    .await;
    match validated {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            release(&state);
            return bad_request(e).into_response();
        }
        Err(e) => {
            release(&state);
            return ApiError::from(anyhow!("validate task failed: {e}")).into_response();
        }
    }

    // Spawn the move on a blocking task (it does synchronous FS work); forward
    // throttled progress onto the SSE broadcast, clear the flag when done.
    let chat = state.chat.clone();
    let flag = state.data_dir_move_in_progress.clone();
    let target = std::path::PathBuf::from(path);
    tokio::spawn(async move {
        use crate::local::datadir::MoveProgress;
        let chat_for_progress = chat.clone();
        // Throttle: forward at most one progress event per ~120ms of copy, but
        // always emit phase edges (copied==0 or ==total) so the first/last tick
        // of every phase gets through.
        let last = std::sync::Mutex::new(0i64);
        let on_progress = move |p: MoveProgress| {
            let now = crate::store::now_ms();
            let mut guard = last.lock().unwrap();
            let is_edge = p.copied_bytes == 0 || p.copied_bytes >= p.total_bytes;
            if is_edge || now - *guard >= 120 {
                *guard = now;
                chat_for_progress.emit_event("datadir.move.progress", json!(p));
            }
        };
        let target_for_move = target.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::local::datadir::move_data_dir(target_for_move, on_progress)
        })
        .await;

        match result {
            Ok(Ok(outcome)) => {
                // Restart harness children so any that pinned the old data dir
                // (Codex hard-pins $ORX_DATA_DIR at spawn) respawn on the new one.
                chat.shutdown_harnesses().await;
                chat.emit_event("datadir.move.done", json!(outcome));
            }
            Ok(Err(e)) => chat.emit_event("datadir.move.error", json!({ "error": e.to_string() })),
            Err(e) => chat.emit_event(
                "datadir.move.error",
                json!({ "error": format!("move task panicked: {e}") }),
            ),
        }
        flag.store(false, Ordering::SeqCst);
    });

    (StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response()
}

/// Count runs currently in an active state (`starting`/`running`), for the
/// data-dir move's in-flight guard. SQL-side and unbounded (see
/// `Store::count_active_runs`).
fn active_run_count() -> usize {
    Store::open()
        .and_then(|s| s.count_active_runs())
        .unwrap_or(0)
}

/// Refuse an operation that would write the store while a data-dir move is in
/// progress — the move relies on nothing new touching the old dir mid-flight.
fn reject_if_moving(state: &AppState) -> std::result::Result<(), ApiError> {
    if state
        .data_dir_move_in_progress
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A data-directory move is in progress. Try again once it finishes.".into(),
        ));
    }
    Ok(())
}

// --- git settings -----------------------------------------------------------

fn git_out(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn git_settings_json() -> Value {
    // Spawn failure means gh isn't installed — distinct from installed-but-
    // signed-out, so the UI can lead with the right fix.
    let gh = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output();
    let gh_installed = gh.is_ok();
    let github_source = if std::env::var("GITHUB_TOKEN").is_ok_and(|t| !t.trim().is_empty()) {
        Some("env")
    } else if crate::config::synced_env_var("GITHUB_TOKEN").is_some() {
        Some("stored")
    } else {
        matches!(gh, Ok(out) if out.status.success() && !out.stdout.is_empty()).then_some("gh")
    };
    json!({
        "gitVersion": git_out(&["--version"]),
        "userName": git_out(&["config", "--global", "user.name"]),
        "userEmail": git_out(&["config", "--global", "user.email"]),
        "ghInstalled": gh_installed,
        "githubTokenSource": github_source,
    })
}

#[derive(Deserialize)]
struct SetGitTokenReq {
    token: String,
}

/// Validate a pasted GitHub token against the API, then persist it to the
/// synced env file — the same store job launches already read, so local git
/// ops and remote compute both pick it up.
async fn set_git_token(Json(req): Json<SetGitTokenReq>) -> ApiResult {
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(bad_request("token is required"));
    }
    let resp = reqwest::Client::new()
        .get("https://api.github.com/user")
        .header("User-Agent", "orx")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| bad_request(format!("Could not reach api.github.com: {e}")))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(bad_request(
            "GitHub rejected the token — check it was copied fully.",
        ));
    }
    if !resp.status().is_success() {
        return Err(bad_request(format!(
            "GitHub returned {} validating the token.",
            resp.status()
        )));
    }
    // Classic PATs list scopes; fine-grained tokens send an empty header, so
    // only enforce when scopes are reported.
    let scopes = resp
        .headers()
        .get("x-oauth-scopes")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !scopes.trim().is_empty() && !scopes.split(',').any(|s| s.trim() == "repo") {
        return Err(bad_request(
            "Token is valid but lacks the `repo` scope — private clones and branch pushes would fail.",
        ));
    }
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_var("GITHUB_TOKEN", &token)?;
        Ok(Json(git_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

async fn delete_git_token() -> ApiResult {
    tokio::task::spawn_blocking(|| {
        crate::config::remove_synced_env_var("GITHUB_TOKEN")?;
        Ok(Json(git_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

async fn git_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(git_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetGitSettingsReq {
    user_name: Option<String>,
    user_email: Option<String>,
}

async fn set_git_settings(Json(req): Json<SetGitSettingsReq>) -> ApiResult {
    let name = req.user_name.map(|s| s.trim().to_string());
    let email = req.user_email.map(|s| s.trim().to_string());
    if name.as_deref().is_none_or(str::is_empty) && email.as_deref().is_none_or(str::is_empty) {
        return Err(bad_request(
            "nothing to update: pass userName and/or userEmail",
        ));
    }
    tokio::task::spawn_blocking(move || {
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                let ok = std::process::Command::new("git")
                    .args(["config", "--global", key, &v])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ok {
                    return Err(bad_request(format!("git config --global {key} failed")));
                }
            }
        }
        Ok(Json(git_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

// --- telemetry settings -----------------------------------------------------

/// `{ enabled, reason }` — whether anonymous usage analytics is on, and if off,
/// why (so the UI can explain a `--no-telemetry`-style override vs a persisted
/// opt-out). `reason` is null when enabled.
fn telemetry_settings_json() -> Value {
    match crate::telemetry::disabled_reason(false) {
        None => json!({ "enabled": true, "reason": null }),
        Some(r) => json!({ "enabled": false, "reason": r.as_str() }),
    }
}

async fn telemetry_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(telemetry_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("telemetry task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetTelemetryReq {
    enabled: bool,
}

async fn set_telemetry_settings(Json(req): Json<SetTelemetryReq>) -> ApiResult {
    let enabled = req.enabled;
    tokio::task::spawn_blocking(move || {
        crate::telemetry::set_persisted_disabled(!enabled)
            .map_err(|e| ApiError::from(anyhow!("could not save telemetry setting: {e}")))?;
        Ok(Json(telemetry_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("telemetry task failed: {e}")))?
}

/// Record the consent decision (agree/reject) for the analytics choice — fired
/// once when the user leaves the onboarding step, so every user who sees it is
/// counted, including those who accept the default. Unconditional by design (see
/// telemetry::record_consent): it lands even when the choice is "off".
async fn record_telemetry_consent(Json(req): Json<SetTelemetryReq>) -> ApiResult {
    crate::telemetry::record_consent(req.enabled).await;
    Ok(Json(json!({ "ok": true })))
}

// --- ssh hosts ----------------------------------------------------------------

/// Concrete Host entries from `~/.ssh/config` (wildcard patterns skipped) —
/// read-only groundwork for an SSH compute backend. No keys are read.
fn list_ssh_hosts() -> Vec<Value> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".ssh").join("config")) else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut hosts: Vec<Value> = Vec::new();
    // Indices into `hosts` for the Host block currently being filled.
    let mut current: Vec<usize> = Vec::new();
    for line in raw.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match line.split_once([' ', '\t', '=']) {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"')),
            None => continue,
        };
        if key == "host" {
            current = value
                .split_whitespace()
                .filter(|name| !name.contains(['*', '?', '!']))
                .map(|name| {
                    hosts.push(json!({ "host": name }));
                    hosts.len() - 1
                })
                .collect();
            continue;
        }
        let field = match key.as_str() {
            "hostname" => "hostname",
            "user" => "user",
            "port" => "port",
            "identityfile" => "identityFile",
            _ => continue,
        };
        for &i in &current {
            // First value wins, like ssh itself.
            if hosts[i].get(field).is_none() {
                hosts[i][field] = json!(value);
            }
        }
    }
    hosts
}

async fn ssh_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| {
        let mut hosts = list_ssh_hosts();
        // Best-effort, like the preflight write: a store hiccup shouldn't take
        // out the host listing — hosts just render as never tested.
        let tests: HashMap<String, SshHostTest> = Store::open()
            .and_then(|s| s.list_ssh_host_tests())
            .unwrap_or_else(|e| {
                eprintln!("orx up: could not load ssh test history: {e}");
                Vec::new()
            })
            .into_iter()
            .map(|t| (t.host.clone(), t))
            .collect();
        for h in &mut hosts {
            let Some(t) = h
                .get("host")
                .and_then(Value::as_str)
                .and_then(|a| tests.get(a))
            else {
                continue;
            };
            h["lastTest"] = json!(t);
        }
        Ok(Json(json!({ "hosts": hosts })))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ssh task failed: {e}")))?
}

#[derive(Deserialize)]
struct SshPreflightReq {
    host: String,
}

/// Live check for one host: can we reach it (BatchMode ssh), and is `git` there?
async fn ssh_preflight(Json(req): Json<SshPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    let p = crate::jobs::ssh::preflight(&crate::jobs::ssh::SshTarget::alias(&host)).await;
    let test = SshHostTest {
        host,
        reachable: p.reachable,
        git_found: p.git_found,
        error: p.error,
        tested_at: now_ms(),
    };
    // Best-effort persistence — the UI shows "last tested" across restarts,
    // but a store hiccup shouldn't hide a test result that already ran.
    let record = test.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || Store::open()?.upsert_ssh_host_test(&record))
            .await
            .map_err(|e| anyhow!("ssh task failed: {e}"))
            .and_then(|r| r)
    {
        eprintln!("orx up: could not record ssh test for {}: {e}", test.host);
    }
    Ok(Json(json!(test)))
}

// --- slurm --------------------------------------------------------------------

use crate::jobs::slurm;

/// One payload powers the whole settings card: stored cluster defaults plus
/// the ssh hosts to pick a login node from (same `~/.ssh/config` source as
/// the ssh backend — a Slurm login node is just an ssh host).
fn slurm_settings_json() -> Value {
    let settings = slurm::load_settings().ok().flatten().unwrap_or_default();
    json!({
        "host": settings.host,
        "partition": settings.partition,
        "account": settings.account,
        "timeLimit": settings.time_limit,
        "hosts": list_ssh_hosts(),
    })
}

async fn slurm_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(slurm_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSlurmSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (cluster default).
    host: Option<String>,
    partition: Option<String>,
    account: Option<String>,
    time_limit: Option<String>,
}

async fn set_slurm_settings(Json(req): Json<SetSlurmSettingsReq>) -> ApiResult {
    // One spawn_blocking around the whole load→mutate→save→respond body
    // (settings + ~/.ssh/config are sync fs I/O), like the git handlers.
    tokio::task::spawn_blocking(move || {
        let mut settings = slurm::load_settings()?.unwrap_or_default();
        let norm = |v: String| Some(v.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(h) = req.host {
            settings.host = norm(h);
        }
        if let Some(p) = req.partition {
            settings.partition = norm(p);
        }
        if let Some(a) = req.account {
            settings.account = norm(a);
        }
        if let Some(t) = req.time_limit {
            // Reject a default that would fail every later launch.
            let t = norm(t);
            if let Some(t) = &t {
                crate::jobs::huggingface::parse_timeout(t).map_err(bad_request)?;
            }
            settings.time_limit = t;
        }
        slurm::save_settings(&settings)?;
        Ok(Json(slurm_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
struct SlurmPreflightReq {
    host: String,
}

/// Live check for one login node: reachable, Slurm CLI + git present, and
/// which partitions exist (feeds the partition picker).
async fn slurm_preflight(Json(req): Json<SlurmPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    let p = slurm::preflight(&host).await;
    Ok(Json(json!({
        "reachable": p.reachable,
        "slurmFound": p.slurm_found,
        "gitFound": p.git_found,
        "partitions": p.partitions,
        "error": p.error,
    })))
}

// --- ray --------------------------------------------------------------------

use crate::jobs::ray;

fn ray_settings_json() -> Value {
    let settings = ray::load_settings().ok().flatten().unwrap_or_default();
    let (resolved, source) = ray::resolve_address_with_source();
    let source_label = match source {
        ray::AddressSource::Settings => "settings",
        ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        ray::AddressSource::Default => "default",
    };
    json!({
        "address": settings.address,
        "resolvedAddress": resolved,
        "source": source_label,
    })
}

async fn ray_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(ray_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRaySettingsReq {
    /// `None` leaves alone; `Some("")` clears (fall back to env / default).
    address: Option<String>,
}

async fn set_ray_settings(Json(req): Json<SetRaySettingsReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let mut settings = ray::load_settings()?.unwrap_or_default();
        if let Some(a) = req.address {
            let a = Some(a.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(a) = &a {
                // Reject a default that would fail every later launch.
                let url = reqwest::Url::parse(a)
                    .map_err(|e| bad_request(anyhow!("Invalid Jobs URL {a:?}: {e}")))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(bad_request(anyhow!(
                        "The Jobs URL must be http(s), e.g. http://127.0.0.1:8265 (got {a:?})."
                    )));
                }
            }
            settings.address = a;
        }
        ray::save_settings(&settings)?;
        Ok(Json(ray_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
struct RayPreflightReq {
    address: Option<String>,
}

/// Live check for a Ray Jobs / Dashboard endpoint.
async fn ray_preflight(Json(req): Json<RayPreflightReq>) -> ApiResult {
    let address = ray::resolve_address(req.address.as_deref());
    match ray::preflight(&address).await {
        Ok(ray_version) => Ok(Json(json!({
            "reachable": true,
            "address": address,
            "rayVersion": ray_version,
            "error": null,
        }))),
        Err(e) => Ok(Json(json!({
            "reachable": false,
            "address": address,
            "rayVersion": null,
            "error": e.to_string(),
        }))),
    }
}

// --- compute targets (unified settings list + default) --------------------------

/// The whole payload for the Compute tab's collapsed list, in one round trip.
/// CHEAP probes only — env vars and file reads, never a network call, kubectl,
/// or the modal python import. `configured` means "worth trying", not
/// "healthy"; deep health stays in each backend's own settings endpoint,
/// fetched when a row is expanded.
/// Whether a box would accept this machine. Three-valued on purpose: the check
/// needs the api, so "we couldn't ask" is a different answer from "no" and the
/// badge shouldn't have to pick one of the two. Each arm carries what the row
/// needs to say — guessing a key path would send the user to a file that may
/// not exist.
#[derive(Clone, PartialEq)]
enum SshReadiness {
    Ready,
    /// The `.pub` on this machine worth registering, if there is one.
    NoUsableKey {
        pub_path: Option<String>,
    },
    Unverified {
        reason: String,
    },
}

async fn openresearch_ssh_readiness() -> SshReadiness {
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    let Ok(Some(creds)) = crate::config::load_credentials().await else {
        // Signed-out is reported by the row's own `or_logged_in`.
        return SshReadiness::NoUsableKey { pub_path: None };
    };
    let named = |local: &[crate::local::ssh_identity::LocalKey]| SshReadiness::NoUsableKey {
        pub_path: preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde),
    };
    match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => SshReadiness::Ready,
        KeyStatus::NoLocalMatch { local, .. } | KeyStatus::NoneRegistered { local } => {
            named(&local)
        }
        KeyStatus::Unknown { reason } => SshReadiness::Unverified { reason },
    }
}

/// The openresearch row's one-line status. Every branch that tells the user to
/// run something names a path we actually found — never a guessed one.
fn openresearch_summary(logged_in: bool, ssh: &SshReadiness) -> String {
    if !logged_in {
        return "Not signed in — run orx login".to_string();
    }
    match ssh {
        SshReadiness::Ready => "Signed in — ephemeral boxes billed to your org".to_string(),
        SshReadiness::NoUsableKey {
            pub_path: Some(path),
        } => format!("No usable SSH key — run orx ssh-key add {path}"),
        SshReadiness::NoUsableKey { pub_path: None } => {
            "No SSH key on this computer — run ssh-keygen -t ed25519, then orx ssh-key add"
                .to_string()
        }
        SshReadiness::Unverified { reason } => {
            format!("Signed in — couldn't check your SSH key ({reason})")
        }
    }
}

fn compute_settings_json(ssh: SshReadiness) -> Value {
    let default = crate::config::compute_default();
    let (default_backend, default_flavor) = match &default {
        Some((b, f)) => (Some(b.as_str()), f.as_deref()),
        None => (None, None),
    };

    let hf = crate::jobs::huggingface::resolve_token_with_source().ok();
    let modal_source = crate::jobs::modal::token_source();
    let k8s_settings = k8s::load_settings().ok().flatten();
    let ssh_hosts = list_ssh_hosts().len();
    let slurm_settings = crate::jobs::slurm::load_settings().ok().flatten();
    let slurm_host = slurm_settings.as_ref().and_then(|s| s.host.clone());
    let (ray_resolved, ray_source) = crate::jobs::ray::resolve_address_with_source();
    let ray_configured = !matches!(ray_source, crate::jobs::ray::AddressSource::Default);
    let ray_source_label = match ray_source {
        crate::jobs::ray::AddressSource::Settings => "Saved address",
        crate::jobs::ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        crate::jobs::ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        crate::jobs::ray::AddressSource::Default => "Default localhost:8265",
    };
    // Presence of the credentials file only — whether the token still works is
    // the expanded row's (network) question.
    let or_logged_in = crate::config::credentials_present();

    // Same spellings as the expanded rows' SOURCE_LABELS/MODAL_TOKEN_LABELS
    // in the UI — the collapsed head stays visible above the open row, so the
    // same fact must not read two different ways.
    let source_label = |s: &crate::jobs::huggingface::TokenSource| match s {
        crate::jobs::huggingface::TokenSource::Env => "HF_TOKEN env var",
        crate::jobs::huggingface::TokenSource::OpenresearchEnv => "Token from ~/.openresearch/env",
        crate::jobs::huggingface::TokenSource::HfCache => "Token from ~/.cache/huggingface/token",
    };
    let targets = json!([
        {
            "id": "local",
            "configured": true,
            "summary": "Runs as a detached process on this machine",
        },
        {
            "id": "hf",
            "configured": hf.is_some(),
            "summary": hf.as_ref().map_or_else(
                || "No token".to_string(),
                |(_, s)| source_label(s).to_string(),
            ),
        },
        {
            "id": "modal",
            "configured": modal_source.is_some(),
            "summary": match modal_source {
                Some("env") => "MODAL_TOKEN_ID env var",
                Some("syncedEnv") => "Token from ~/.openresearch/env",
                Some("modalToml") => "Token from ~/.modal.toml",
                _ => "No token",
            },
        },
        {
            "id": "k8s",
            "configured": k8s_settings.is_some(),
            "summary": k8s_settings.as_ref().map_or_else(
                || "No context selected".to_string(),
                |s| format!(
                    "Context {} / namespace {}",
                    s.context.as_deref().unwrap_or("(kubectl default)"),
                    s.namespace,
                ),
            ),
        },
        {
            "id": "ssh",
            "configured": ssh_hosts > 0,
            "summary": match ssh_hosts {
                0 => "No hosts in ~/.ssh/config".to_string(),
                1 => "1 host in ~/.ssh/config".to_string(),
                n => format!("{n} hosts in ~/.ssh/config"),
            },
        },
        {
            "id": "slurm",
            "configured": slurm_host.is_some(),
            "summary": slurm_host.as_ref().map_or_else(
                || "No login node configured".to_string(),
                |h| format!("Login node {h}"),
            ),
        },
        {
            "id": "ray",
            "configured": ray_configured,
            "summary": if ray_configured {
                format!("{ray_source_label} ({ray_resolved})")
            } else {
                ray_source_label.to_string()
            },
        },
        {
            "id": "openresearch",
            // Signed in alone would be a green light on a backend that can't
            // connect — the box authorizes your *registered* keys, so one of
            // them has to be on this machine too.
            "configured": or_logged_in && ssh == SshReadiness::Ready,
            "unverified": or_logged_in && matches!(ssh, SshReadiness::Unverified { .. }),
            "summary": openresearch_summary(or_logged_in, &ssh),
        },
    ]);
    json!({
        "defaultBackend": default_backend,
        "defaultFlavor": default_flavor,
        "targets": targets,
    })
}

async fn compute_settings() -> ApiResult {
    let ssh = openresearch_ssh_readiness().await;
    // fs/env probes only, but keep them off the async runtime anyway.
    let payload = tokio::task::spawn_blocking(move || compute_settings_json(ssh))
        .await
        .map_err(|e| ApiError::from(anyhow!("compute settings task failed: {e}")))?;
    Ok(Json(payload))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetComputeDefaultReq {
    /// `None`/absent clears the default (and its flavor with it).
    backend: Option<String>,
    flavor: Option<String>,
}

/// Persist the default compute target. An *unconfigured* backend is allowed
/// (config state fluctuates outside orx; the UI warns instead) — only unknown
/// backends and meaningless flavors are rejected.
async fn set_compute_default(Json(req): Json<SetComputeDefaultReq>) -> ApiResult {
    let backend = req
        .backend
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    let flavor = req
        .flavor
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty());
    if let Some(b) = &backend {
        local::validate_compute_default(b, flavor.as_deref()).map_err(bad_request)?;
    }
    // Picking openresearch as the default is the moment to answer "will this
    // actually work?", so the row that comes back is honest about the SSH key.
    let ssh = openresearch_ssh_readiness().await;
    // Validation already ran above, so a failure in here is a server-side
    // fault (io error, corrupt settings.json refusal) — surface it as 500 via
    // the plain ApiError conversion, not as a 400 blaming the request.
    let payload = tokio::task::spawn_blocking(move || -> Result<Value> {
        crate::config::set_compute_default(backend, flavor)?;
        Ok(compute_settings_json(ssh))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("compute default task failed: {e}")))??;
    Ok(Json(payload))
}

/// The "This machine" row's expanded detail: detected hardware. Subprocess
/// probes (hostname, sysctl, nvidia-smi) — blocking, so spawned.
async fn local_machine_settings() -> ApiResult {
    let hw = tokio::task::spawn_blocking(crate::jobs::localbox::hardware_info)
        .await
        .map_err(|e| ApiError::from(anyhow!("hardware probe task failed: {e}")))?;
    Ok(Json(json!(hw)))
}

/// The OpenResearch row's expanded detail. Network calls are fine here (the
/// row is open) but each is individually best-effort — an offline machine
/// still renders "signed in, status unknown" instead of an error page.
async fn openresearch_settings() -> ApiResult {
    let Some(creds) = crate::config::load_credentials().await? else {
        return Ok(Json(json!({
            "loggedIn": false,
            "apiUrl": null,
            "orgs": [],
            "sshKeyStatus": "unknown",
            "error": null,
        })));
    };
    let mut error: Option<String> = None;
    let orgs = match crate::client::list_orgs(&creds).await {
        Ok(o) => o.orgs.into_iter().map(|o| o.name).collect::<Vec<_>>(),
        Err(e) => {
            error = Some(e.to_string());
            Vec::new()
        }
    };
    // "Registered" alone is a misleading green: a key registered from another
    // laptop leaves this machine unable to reach any box. Report whether the
    // private half is actually here.
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    // Hand back the .pub we actually found so the note can name a real file
    // rather than guessing at ~/.ssh/id_ed25519.pub.
    let mut ssh_key_path: Option<String> = None;
    let mut note_key = |local: &[crate::local::ssh_identity::LocalKey]| {
        ssh_key_path = preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde);
    };
    let ssh_key_status = match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => "matched",
        KeyStatus::NoLocalMatch { local, .. } => {
            note_key(&local);
            "no_local_match"
        }
        KeyStatus::NoneRegistered { local } => {
            note_key(&local);
            "none_registered"
        }
        KeyStatus::Unknown { reason } => {
            error.get_or_insert(reason);
            "unknown"
        }
    };
    Ok(Json(json!({
        "loggedIn": true,
        "apiUrl": creds.api_url,
        "orgs": orgs,
        "sshKeyStatus": ssh_key_status,
        "sshKeyPath": ssh_key_path,
        "error": error,
    })))
}

// --- harnesses ---------------------------------------------------------------

const HARNESS_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
struct HarnessQuery {
    refresh: Option<u8>,
    retry: Option<u8>,
}

fn overlay_claude_auth(payload: &mut Value, snapshot: local::claude::AuthSnapshot) {
    let Some(harnesses) = payload.get_mut("harnesses").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(claude) = harnesses
        .iter_mut()
        .find(|h| h.get("id").and_then(Value::as_str) == Some("claude-code"))
    else {
        return;
    };
    claude["authState"] = json!(snapshot.state);
    if snapshot.state == local::harness::HarnessAuthState::Ready {
        return;
    }
    claude["authenticated"] = json!(false);
    claude["agentReady"] = json!(false);
    claude["models"] = json!([]);
    if let Some(object) = claude.as_object_mut() {
        object.remove("authMethod");
        object.remove("account");
        object.remove("org");
        object.remove("plan");
    }
    claude["agentNote"] = json!(if snapshot.runtime_rejected {
        local::harness::claude::auth_recovery_note()
    } else {
        match snapshot.state {
            local::harness::HarnessAuthState::NeedsLogin => {
                "Sign in with `claude auth login`, then re-check this harness."
            }
            local::harness::HarnessAuthState::Unknown => {
                "Open a terminal and run `claude auth status`, then re-check this harness."
            }
            local::harness::HarnessAuthState::Unsupported => {
                "Update Claude Code to 2.1.211 or newer, then re-check this harness."
            }
            local::harness::HarnessAuthState::Ready => unreachable!(),
        }
    });
}

fn payload_has_ready_claude(payload: &Value) -> bool {
    payload
        .get("harnesses")
        .and_then(Value::as_array)
        .and_then(|harnesses| {
            harnesses
                .iter()
                .find(|h| h.get("id").and_then(Value::as_str) == Some("claude-code"))
        })
        .and_then(|claude| claude.get("agentReady"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn ready_claude_entry(payload: &Value) -> Option<Value> {
    payload
        .get("harnesses")?
        .as_array()?
        .iter()
        .find(|h| {
            h.get("id").and_then(Value::as_str) == Some("claude-code")
                && h.get("agentReady").and_then(Value::as_bool) == Some(true)
        })
        .cloned()
}

fn replace_claude_entry(payload: &mut Value, replacement: Value) {
    let Some(harnesses) = payload.get_mut("harnesses").and_then(Value::as_array_mut) else {
        return;
    };
    if let Some(claude) = harnesses
        .iter_mut()
        .find(|h| h.get("id").and_then(Value::as_str) == Some("claude-code"))
    {
        *claude = replacement;
    }
}

async fn list_harnesses(
    State(state): State<AppState>,
    Query(q): Query<HarnessQuery>,
) -> Json<Value> {
    let mut cache = state.harnesses.lock().await;
    if q.retry == Some(1) && state.claude.clear_runtime_rejection() {
        *cache = None;
    }
    let prior_ready_claude = cache
        .as_ref()
        .map(|(_, payload)| payload)
        .and_then(ready_claude_entry);
    if q.refresh != Some(1) {
        if let Some((at, payload)) = cache.as_ref() {
            if at.elapsed() < HARNESS_CACHE_TTL {
                let snapshot = state.claude.auth_snapshot();
                if snapshot.state != local::harness::HarnessAuthState::Ready
                    || payload_has_ready_claude(payload)
                {
                    let mut payload = payload.clone();
                    overlay_claude_auth(&mut payload, snapshot);
                    return Json(payload);
                }
            }
        }
    }
    let probe_generation = state.claude.auth_snapshot().generation;
    let harnesses = local::harness::detect_harnesses().await;
    if let Some(claude) = harnesses.iter().find(|h| h.id == "claude-code") {
        state
            .claude
            .observe_auth_state(claude.auth_state, probe_generation);
    }
    let mut payload = json!({ "harnesses": harnesses });
    let mut snapshot = state.claude.auth_snapshot();
    if snapshot.state == local::harness::HarnessAuthState::Ready
        && !payload_has_ready_claude(&payload)
    {
        if let Some(prior) = prior_ready_claude {
            replace_claude_entry(&mut payload, prior);
        } else if let Some(retry) = local::harness::detect_harness("claude-code").await {
            state
                .claude
                .observe_auth_state(retry.auth_state, snapshot.generation);
            if retry.agent_ready {
                replace_claude_entry(&mut payload, json!(retry));
            }
            snapshot = state.claude.auth_snapshot();
        }
        if snapshot.state == local::harness::HarnessAuthState::Ready
            && !payload_has_ready_claude(&payload)
        {
            state.claude.defer_auth_verification(snapshot.generation);
            snapshot = state.claude.auth_snapshot();
        }
    }
    if state.claude.claim_auth_announcement(snapshot.generation) {
        state.chat.emit_event(
            "harness.auth",
            json!({ "harness": "claude-code", "authState": snapshot.state }),
        );
    }
    overlay_claude_auth(&mut payload, snapshot);
    *cache = Some((std::time::Instant::now(), payload.clone()));
    Json(payload)
}

// --- chat --------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsQuery {
    project_id: String,
}

async fn list_chat_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> ApiResult {
    let sessions = Store::open()?.list_chat_sessions_by_project(&q.project_id)?;
    let busy = state.chat.busy_sessions().await;
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|s| local::chat::session_json(s, busy.contains(&s.id)))
        .collect();
    Ok(Json(json!({ "sessions": sessions })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChatSessionReq {
    project_id: String,
    harness: String,
    model: Option<String>,
    permission_mode: Option<String>,
    reasoning_level: Option<String>,
    /// Persona wire id for this session; None = inherit the project's persona.
    /// Lets a dispatched subagent run a different persona on the same project.
    persona: Option<String>,
}

async fn create_chat_session(
    State(state): State<AppState>,
    Json(req): Json<CreateChatSessionReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    if !local::harness::is_chat_harness(&req.harness) {
        return Err(bad_request(format!("unknown harness: {}", req.harness)));
    }
    let _admission = state
        .project_lifecycle
        .admit(&req.project_id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    let store = Store::open()?;
    store
        .get_local_project(&req.project_id)?
        .ok_or_else(|| not_found("project"))?;
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    // Validate the persona wire id up front so a typo is rejected, not stored.
    let persona = nonempty(req.persona);
    if let Some(p) = persona.as_deref() {
        local::agent_skills::Persona::parse(Some(p)).map_err(bad_request)?;
    }
    let session = StoredChatSession {
        id: format!("chat_{}", uuid::Uuid::new_v4()),
        project_id: req.project_id,
        harness: req.harness,
        native_session_id: None,
        title: None,
        title_source: None,
        model: nonempty(req.model),
        // Observed during the first turn, not at creation.
        effective_model: None,
        permission_mode: nonempty(req.permission_mode),
        reasoning_level: nonempty(req.reasoning_level),
        persona,
        parent_session_id: None,
        archived: false,
        context_usage_json: None,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    store.create_chat_session(&session)?;
    Ok(Json(
        json!({ "session": local::chat::session_json(&session, false) }),
    ))
}

// --- subagent dispatch proposals -----------------------------------------------

fn proposal_json(p: &crate::store::StoredAgentProposal) -> serde_json::Value {
    json!({
        "id": p.id,
        "projectId": p.project_id,
        "parentExperimentId": p.parent_experiment_id,
        "persona": p.persona,
        "harness": p.harness,
        "model": p.model,
        "task": p.task,
        "why": p.why,
        "status": p.status,
        "sessionId": p.session_id,
        "parentSessionId": p.parent_session_id,
        "createdAt": p.created_at,
        "updatedAt": p.updated_at,
    })
}

async fn list_proposals(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let props: Vec<serde_json::Value> = store
        .list_agent_proposals(&id)?
        .iter()
        .map(proposal_json)
        .collect();
    Ok(Json(json!({ "proposals": props })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveProposalReq {
    /// The human's final picks; each overrides the orchestrator's suggestion.
    persona: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    reasoning_level: Option<String>,
}

/// Which model a dispatch actually runs on. Three request states, and collapsing
/// the last two is a real bug: the approval card resolves a suggestion the chosen
/// provider doesn't offer (a stale `claude-opus-5` against Claude Code's live
/// catalog) down to "provider default" and *shows* that — so if an explicit empty
/// string fell back to the proposal, the dispatch would silently run the very id
/// the card told the human it had dropped.
///
///   absent         → no override; honour the orchestrator's suggestion
///   present, empty → the human chose the provider default (no model pinned)
///   present, set   → the human's pick
fn resolve_dispatch_model(requested: Option<&str>, suggested: Option<&str>) -> Option<String> {
    match requested.map(str::trim) {
        None => suggested.map(str::to_string),
        Some("") => None,
        Some(m) => Some(m.to_string()),
    }
}

/// Approve a proposal: spawn a chat session with the chosen persona/harness/
/// model and run the proposed task as its first turn.
async fn approve_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApproveProposalReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let prop = store
        .get_agent_proposal(&id)?
        .ok_or_else(|| not_found("proposal"))?;
    if prop.status != "pending" {
        return Err(bad_request(format!("proposal already {}", prop.status)));
    }
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    // The human's choice wins over the orchestrator's suggestion.
    let persona = nonempty(req.persona).or_else(|| prop.persona.clone());
    let harness = nonempty(req.harness)
        .or_else(|| prop.harness.clone())
        .ok_or_else(|| bad_request("no harness — suggest one or pass it on approve"))?;
    let model = resolve_dispatch_model(req.model.as_deref(), prop.model.as_deref());
    if !local::harness::is_chat_harness(&harness) {
        return Err(bad_request(format!("unknown harness: {harness}")));
    }
    if let Some(p) = persona.as_deref() {
        local::agent_skills::Persona::parse(Some(p)).map_err(bad_request)?;
    }
    store
        .get_local_project(&prop.project_id)?
        .ok_or_else(|| not_found("project"))?;

    let session = StoredChatSession {
        id: format!("chat_{}", uuid::Uuid::new_v4()),
        project_id: prop.project_id.clone(),
        harness,
        native_session_id: None,
        title: None,
        model,
        // Observed during the first turn, not at creation.
        effective_model: None,
        // An approved subagent runs autonomously in the background — there is no
        // human at its keyboard to answer per-tool permission prompts, so it would
        // hang on the first one. Default it to bypass (the human already approved
        // spawning it); an explicit mode on the approve request still wins.
        permission_mode: nonempty(req.permission_mode).or_else(|| Some("bypass".to_string())),
        reasoning_level: nonempty(req.reasoning_level),
        persona,
        // Nest the spawned subagent under the orchestrator that suggested it.
        parent_session_id: prop.parent_session_id.clone(),
        archived: false,
        // The task text titles it up front, so the auto-titler leaves it alone.
        title_source: None,
        context_usage_json: None,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    store.create_chat_session(&session)?;
    store.set_agent_proposal_status(&id, "approved", Some(&session.id))?;

    // Run the dispatched task as the session's first turn (background; streams
    // over /api/events like any other turn). Session values are already set, so
    // no composer overrides needed. Prepend the target node id when the proposal
    // names one, so the subagent works on the right experiment instead of
    // guessing (a wrong id routes to the server and fails with "Not logged in").
    let task = match &prop.parent_experiment_id {
        Some(exp) => format!("{}\n\n(Target experiment node: `{exp}`.)", prop.task),
        None => prop.task.clone(),
    };
    let overrides = local::chat::TurnOverrides {
        model: None,
        permission_mode: None,
        reasoning_level: None,
    };
    state
        .chat
        .send_message(&session.id, task, overrides, Vec::new())
        .await
        .map_err(bad_request)?;
    Ok(Json(
        json!({ "session": local::chat::session_json(&session, true) }),
    ))
}

async fn dismiss_proposal(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let prop = store
        .get_agent_proposal(&id)?
        .ok_or_else(|| not_found("proposal"))?;
    if prop.status != "pending" {
        return Err(bad_request(format!("proposal already {}", prop.status)));
    }
    store.set_agent_proposal_status(&id, "dismissed", None)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_chat_session(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    state.chat.delete_session(&id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateChatSessionReq {
    archived: Option<bool>,
    title: Option<String>,
}

async fn update_chat_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateChatSessionReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let session = if let Some(title) = req.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(bad_request("title cannot be empty"));
        }
        state
            .chat
            .set_title(&id, title)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(archived) = req.archived {
        state
            .chat
            .set_archived(&id, archived)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else {
        return Err(bad_request("nothing to update"));
    };
    let busy = state.chat.is_busy(&id).await;
    Ok(Json(
        json!({ "session": local::chat::session_json(&session, busy) }),
    ))
}

async fn chat_messages(Path(id): Path<String>) -> ApiResult {
    Store::open()?
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let messages = local::chat::list_messages(&id)?;
    Ok(Json(json!({ "messages": messages })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendChatReq {
    text: String,
    model: Option<String>,
    permission_mode: Option<String>,
    reasoning_level: Option<String>,
    #[serde(default)]
    images: Vec<local::chat::ImageAttachment>,
}

async fn send_chat_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendChatReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let text = req.text.trim().to_string();
    if text.is_empty() && req.images.is_empty() {
        return Err(bad_request("text is required"));
    }
    let overrides = local::chat::TurnOverrides {
        model: req.model,
        permission_mode: req.permission_mode,
        reasoning_level: req.reasoning_level,
    };
    // The turn runs in the background; progress streams over /api/events.
    state
        .chat
        .send_message(&id, text, overrides, req.images)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

/// Raw bytes of a pasted-image attachment, by bare file name.
async fn chat_attachment(Path(name): Path<String>) -> std::result::Result<Response, ApiError> {
    // Names are server-minted (img_<uuid>.<ext>); anything else is rejected.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        || name.contains("..")
    {
        return Err(bad_request("invalid attachment name"));
    }
    tokio::task::spawn_blocking(move || {
        let path = local::chat::attachments_dir()?.join(&name);
        let bytes = std::fs::read(&path).map_err(|_| not_found("attachment"))?;
        Ok((
            [
                (
                    header::CONTENT_TYPE,
                    local::chat::attachment_content_type(&name),
                ),
                (header::CACHE_CONTROL, "max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response())
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("attachment task failed: {e}")))?
}

async fn interrupt_chat(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    state.chat.interrupt_by_user(&id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondReq {
    prompt_id: String,
    #[serde(default = "default_true")]
    approve: bool,
    #[serde(default)]
    resume_mode: Option<String>,
    #[serde(default)]
    answers: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Answer an interactive prompt (plan / permission / question) on a session.
async fn respond_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RespondReq>,
) -> ApiResult {
    state
        .chat
        .respond(local::chat::PromptAnswer {
            session_id: id,
            prompt_id: req.prompt_id,
            approve: req.approve,
            resume_mode: req.resume_mode,
            answers: req.answers,
            note: req.note,
        })
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

/// The `orx mcp-gate` bridge relaying one blocked tool call from a plan-mode
/// claude turn. The response body is the permission decision verbatim
/// (`{"behavior":"allow",…}` / `{"behavior":"deny",…}`) — the bridge
/// stringifies it into the MCP tool result unchanged. Deliberately long-held:
/// it returns when the user answers the card (or policy/timeout decides).
async fn bridge_permission(
    State(state): State<AppState>,
    Json(req): Json<BridgePermissionReq>,
) -> ApiResult {
    let decision = state
        .chat
        .request_permission(&req.session_id, &req.token, &req.tool_name, req.tool_input)
        .await
        .map_err(bad_request)?;
    Ok(Json(serde_json::to_value(decision).map_err(bad_request)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgePermissionReq {
    session_id: String,
    token: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
}

// --- agent ----------------------------------------------------------------

async fn agent_status(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agent.status().await;
    Json(json!({ "running": !agents.is_empty(), "agents": agents }))
}

// --- /api/events SSE ------------------------------------------------------

async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    // Small buffer on purpose: run.log events can carry ~MB payloads, and a
    // stalled client must backpressure the loop, not queue hundreds of MB.
    let (tx, rx) = mpsc::channel::<Event>(16);
    tokio::spawn(event_loop(tx.clone()));
    // Chat events ride the same stream: chat.session / chat.message / chat.busy.
    let mut chat_rx = state.chat.subscribe();
    tokio::spawn(async move {
        loop {
            match chat_rx.recv().await {
                Ok((name, data)) => {
                    if tx.send(json_event(name, &data)).await.is_err() {
                        return;
                    }
                }
                // Lagged subscriber: drop missed events, keep streaming.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok(ev), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Diff state for one SSE subscriber.
#[derive(Default)]
struct EventCursor {
    projects: HashMap<String, i64>,
    experiments: HashMap<String, i64>,
    files: HashMap<String, u64>,
    runs: HashMap<String, (String, i64)>,
    log_offsets: HashMap<String, u64>,
}

/// 500ms poll loop: diff the store + log files, push named events into the
/// channel. Ends when the subscriber disconnects (send fails). Same idiom as
/// serve.rs, extended with project/experiment diffs.
async fn event_loop(tx: mpsc::Sender<Event>) {
    let mut cursor = EventCursor::default();
    let mut first = true;
    loop {
        if !first {
            // An idle store never sends, so a failed send can't be the only
            // disconnect signal — watch the receiver side too or the loop
            // (and its 2Hz store polling) leaks per closed EventSource.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = tx.closed() => return,
            }
        }
        if tx.is_closed() {
            return;
        }
        // Store hiccups (locked db) just skip a tick.
        let batch = collect_events(&mut cursor, first).unwrap_or_default();
        first = false;
        for ev in batch {
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
}

fn json_event(name: &str, data: &Value) -> Event {
    Event::default().event(name).data(data.to_string())
}

/// One diff pass. On the first pass everything is "changed", so a fresh
/// subscriber gets a full snapshot and needs no separate baseline fetches.
fn collect_events(cursor: &mut EventCursor, first: bool) -> Result<Vec<Event>> {
    let store = Store::open()?;
    let mut out = Vec::new();
    // Cap log bytes per tick so one pass never materializes a huge batch —
    // remainders (whole-log replays included) stream out on later ticks.
    let mut log_budget: u64 = 2_000_000;

    for project in store.list_local_projects()? {
        if cursor.projects.get(&project.id) != Some(&project.updated_at) {
            cursor
                .projects
                .insert(project.id.clone(), project.updated_at);
            out.push(json_event(
                "project.updated",
                &json!({ "project": project_json(&project) }),
            ));
        }
        push_experiment_events(&store, &project.id, cursor, &mut out)?;
        // Files appear live — anything written into the files dir (by the
        // agent or the user) pings the UI to refetch the listing.
        let fp = local::files::fingerprint(&project);
        if cursor.files.get(&project.id) != Some(&fp) {
            cursor.files.insert(project.id.clone(), fp);
            out.push(json_event(
                "files.updated",
                &json!({ "projectId": project.id }),
            ));
        }
    }

    for run in store.list_runs(200)? {
        let changed = match cursor.runs.get(&run.id) {
            None => true,
            Some((status, updated)) => *status != run.status || *updated != run.updated_at,
        };
        if changed {
            cursor
                .runs
                .insert(run.id.clone(), (run.status.clone(), run.updated_at));
            out.push(json_event(
                "run.updated",
                &json!({ "run": ApiRun::from(&run) }),
            ));
        }
        if first {
            // Live runs replay their whole log through the stream (chunked per
            // tick); terminal runs start at EOF — backfill is /api/runs/{id}/log.
            let start = if is_terminal(&run.status) {
                log_size(&run.id)
            } else {
                0
            };
            cursor.log_offsets.insert(run.id.clone(), start);
        }
        // Terminal runs were seeded at EOF above, so this is a no-op for them.
        push_log_delta(&run, cursor, &mut out, &mut log_budget);
    }
    Ok(out)
}

fn push_experiment_events(
    store: &Store,
    project_id: &str,
    cursor: &mut EventCursor,
    out: &mut Vec<Event>,
) -> Result<()> {
    for exp in store.list_experiments_by_project(project_id)? {
        if cursor.experiments.get(&exp.id) != Some(&exp.updated_at) {
            cursor.experiments.insert(exp.id.clone(), exp.updated_at);
            out.push(json_event(
                "experiment.updated",
                &json!({ "experiment": exp }),
            ));
        }
    }
    Ok(())
}

fn push_log_delta(
    run: &StoredRun,
    cursor: &mut EventCursor,
    out: &mut Vec<Event>,
    budget: &mut u64,
) {
    let offset = *cursor.log_offsets.entry(run.id.clone()).or_insert(0);
    let size = log_size(&run.id);
    if size <= offset || *budget == 0 {
        return;
    }
    let chunk = read_log_from(&run.id, offset, *budget);
    *budget -= chunk.len() as u64;
    cursor
        .log_offsets
        .insert(run.id.clone(), offset + chunk.len() as u64);
    // base64: chunk boundaries are arbitrary byte positions, and exact byte
    // lengths are what lets the client dedup replays.
    out.push(json_event(
        "run.log",
        &json!({
            "runId": run.id,
            "dataBase64": base64::engine::general_purpose::STANDARD.encode(&chunk),
            "offset": offset,
        }),
    ));
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

fn log_size(run_id: &str) -> u64 {
    std::fs::metadata(log_path(run_id))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn read_log_from(run_id: &str, offset: u64, max: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log_path(run_id)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if f.seek(SeekFrom::Start(offset)).is_ok() {
        let _ = f.take(max).read_to_end(&mut out);
    }
    out
}

// --- embedded SPA ----------------------------------------------------------

/// ui/dist, embedded at release build time (debug builds read from disk).
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist"]
struct UiDist;

const NOT_BUILT_PAGE: &str = "<!doctype html><html><head><title>orx up</title></head>\
<body style=\"font-family:system-ui;background:#111;color:#ddd;display:grid;place-items:center;height:100vh;margin:0\">\
<div><h1>UI not built</h1><p>Run <code>pnpm build</code> in <code>ui/</code>, then rebuild orx.</p>\
<p>The API is live at <code>/api/health</code>.</p></div></body></html>";

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("json") | Some("map") => "application/json",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    // index.html must revalidate every load or browsers heuristically cache it
    // and keep loading a stale (hashed) bundle; the hashed assets themselves
    // are immutable by name. favicon.svg is likewise served under a fixed name.
    let cache = if path == "index.html" || path == "favicon.svg" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    (
        [
            (header::CONTENT_TYPE, mime_for(path)),
            (header::CACHE_CONTROL, cache),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

/// Every non-/api non-/opencode path: exact asset if it exists, index.html
/// otherwise (SPA client routing), friendly page when the UI isn't built.
async fn spa(uri: Uri, headers: axum::http::HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "api" {
        return not_found("route").into_response();
    }
    // Runtime asset loads from an embedded playable use root-absolute paths
    // (`/assets/...` string literals in game code, which no build-time `base`
    // can rewrite). The requesting document's URL rides in Referer — resolve
    // such misses against the referring experiment's play build before the
    // dashboard SPA swallows them.
    if let Some(resp) = play_referer_asset(&headers, uri.path()).await {
        return resp;
    }
    let candidate = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = UiDist::get(candidate) {
        return asset_response(candidate, file);
    }
    match UiDist::get("index.html") {
        Some(file) => asset_response("index.html", file),
        None => Html(NOT_BUILT_PAGE).into_response(),
    }
}

/// Serve `path` from the play build of the experiment named in the request's
/// `/play/<id>/…` Referer, if that file exists there. None = not a play-page
/// request or no such file — fall through to the normal SPA handling.
async fn play_referer_asset(headers: &axum::http::HeaderMap, path: &str) -> Option<Response> {
    let referer = headers.get(header::REFERER)?.to_str().ok()?;
    let exp_id = referer
        .split("/play/")
        .nth(1)?
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?
        .to_string();
    let rel = path.trim_start_matches('/').to_string();
    if rel.is_empty() {
        return None;
    }
    tokio::task::spawn_blocking(move || -> Option<Response> {
        let store = Store::open().ok()?;
        let exp = store.get_local_experiment(&exp_id).ok()??;
        let project = store.get_local_project(&exp.project_id).ok()??;
        let root = std::fs::canonicalize(local::play::play_serve_dir(&project, &exp.id)).ok()?;
        let full = std::fs::canonicalize(root.join(&rel)).ok()?;
        if !full.starts_with(&root) || full.is_dir() {
            return None;
        }
        let bytes = std::fs::read(&full).ok()?;
        let content_type = local::files::content_type_for_path(&full.to_string_lossy());
        Some(
            (
                [
                    (header::CONTENT_TYPE, content_type.to_string()),
                    (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
                ],
                bytes,
            )
                .into_response(),
        )
    })
    .await
    .ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_key(path: Option<&str>) -> SshReadiness {
        SshReadiness::NoUsableKey {
            pub_path: path.map(str::to_string),
        }
    }

    /// The dispatch that shipped `claude-opus-5` even though the card showed
    /// "(provider default)": the agent suggested an id Claude Code's live catalog
    /// no longer lists, the card dropped it, and the omitted field let the server
    /// re-apply it. An explicit empty model must mean *default*, not *suggestion*.
    #[test]
    fn an_explicit_default_does_not_fall_back_to_the_suggestion() {
        // Card resolved the stale suggestion to the provider default.
        assert_eq!(
            resolve_dispatch_model(Some(""), Some("claude-opus-5")),
            None
        );
        // Whitespace is the same intent, not a model named " ".
        assert_eq!(
            resolve_dispatch_model(Some("   "), Some("claude-opus-5")),
            None
        );
        // No override at all → the orchestrator's suggestion still stands.
        assert_eq!(
            resolve_dispatch_model(None, Some("claude-opus-5")).as_deref(),
            Some("claude-opus-5")
        );
        // The human's pick wins over the suggestion.
        assert_eq!(
            resolve_dispatch_model(Some("opencode/kimi-k3"), Some("claude-opus-5")).as_deref(),
            Some("opencode/kimi-k3")
        );
        // Nothing anywhere stays nothing.
        assert_eq!(resolve_dispatch_model(None, None), None);
    }

    /// The row used to hardcode `~/.ssh/id_ed25519.pub`, which is wrong on any
    /// machine whose key is named something else.
    #[test]
    fn names_the_key_file_we_actually_found() {
        let s = openresearch_summary(true, &no_key(Some("~/.ssh/work_ed25519.pub")));
        assert!(s.contains("orx ssh-key add ~/.ssh/work_ed25519.pub"));
        assert!(!s.contains("id_ed25519"), "no guessed default");
    }

    /// Nothing on disk to register, so `ssh-key add` alone would fail.
    #[test]
    fn tells_you_to_generate_one_when_there_is_no_key() {
        let s = openresearch_summary(true, &no_key(None));
        assert!(s.contains("ssh-keygen"));
    }

    #[test]
    fn signed_out_beats_every_key_state() {
        assert!(openresearch_summary(false, &SshReadiness::Ready).contains("orx login"));
        assert!(openresearch_summary(false, &no_key(None)).contains("orx login"));
    }

    #[test]
    fn unverified_says_why_it_could_not_check() {
        let s = openresearch_summary(
            true,
            &SshReadiness::Unverified {
                reason: "timed out".to_string(),
            },
        );
        assert!(s.contains("timed out"), "surfaces the cause: {s}");
    }
}
