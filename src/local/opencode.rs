//! opencode bootstrap for `orx up` — binary discovery, per-session config +
//! playbook written into the session's worktree, spawn + health check, and the
//! shared `AgentHost` handle the axum server holds (`Arc<AgentHost>` in state).
//!
//! One opencode serve child **per chat session**, cwd = that session's private
//! worktree (see `git::ensure_session_worktree`) so parallel agents never share
//! a checkout. Env is inherited (that's where ANTHROPIC_API_KEY /
//! OPENROUTER_API_KEY live — opencode auto-detects providers from env).
//! Children die with `orx up` via `kill_on_drop`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::error::{anyhow, Result};
use crate::local::agent_skills::Persona;
use crate::local::git;
use crate::local::model::LocalProject;
use crate::store;

/// Playbook path inside the session worktree; opencode re-reads it every turn,
/// so rewriting the file retargets a running server without a restart.
const PLAYBOOK_REL: &str = ".openresearch/agent/autoresearch-local.md";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// `opencode` on PATH, else the installer's default drop location.
pub fn find_opencode() -> Result<PathBuf> {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("opencode");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let fallback = home.join(".opencode").join("bin").join("opencode");
        if fallback.is_file() {
            return Ok(fallback);
        }
    }
    Err(anyhow!(
        "opencode not found (checked PATH and ~/.opencode/bin/opencode).\n\
         Install it with: curl -fsSL https://opencode.ai/install | bash"
    ))
}

/// Ask the OS for a free loopback port (bind :0, read it back, release).
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| anyhow!("Could not pick a free port: {}", e))?;
    Ok(listener.local_addr()?.port())
}

/// Where the spawned server's stdout/stderr land (startup diagnostics).
pub fn agent_log_path() -> PathBuf {
    store::data_dir().join("agent-opencode.log")
}

/// Project-local opencode config. Every real permission is pre-approved so a
/// headless turn never stalls on a TUI prompt; the interactive `question` tool
/// is denied AND disabled (it would deadlock serve mode — nothing can answer
/// it), repeated on the default `build` agent because the tool filter is
/// agent-scoped. `model` only when the user passed `orx up --model`.
fn opencode_config_json(model: Option<&str>, instructions: &str) -> String {
    let mut cfg = json!({
        "$schema": "https://opencode.ai/config.json",
        "permission": {
            "edit": "allow",
            "bash": "allow",
            "webfetch": "allow",
            "websearch": "allow",
            "read": "allow",
            "glob": "allow",
            "grep": "allow",
            "task": "allow",
            "skill": "allow",
            "lsp": "allow",
            "doom_loop": "allow",
            "external_directory": "allow",
            "question": "deny",
        },
        "tools": { "question": false },
        "agent": {
            "build": { "tools": { "question": false }, "permission": { "question": "deny" } }
        },
        "instructions": [instructions],
    });
    if let Some(model) = model {
        cfg["model"] = json!(model);
    }
    // Every managed MCP server crux is running (Blender, ComfyUI). Only added if
    // there is at least one: this child gets `OPENCODE_DISABLE_PROJECT_CONFIG=1`
    // in the tracked-config case, so anything we omit here is simply absent rather
    // than falling back to the repo's own `mcp` block.
    if let Some(mcp) = crate::local::mcp_servers::opencode_config() {
        cfg["mcp"] = mcp;
    }
    serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".to_string())
}

/// The local-mode autoresearch playbook: project context + cardinal rules +
/// the v1 local command surface. Ported from the cloud agent's
/// `autoresearchMd()`/`projectContextMd()` prompts, adapted for `orx up`
/// (external backends via `--backend`, analysis via `orx logs`, no
/// artifacts/query/chart).
/// The playbook template — a literal, GitHub-readable markdown file. Rendered
/// by [`playbook_md`]: the leading HTML comment is stripped and `{token}`
/// placeholders are substituted (project facts, the compute default, the
/// skills index, persisted memory).
const SYSTEM_PROMPT: &str = include_str!("../../SYSTEM_PROMPT.md");
/// The game-designer persona's playbook template — same token vocabulary,
/// game-design loop (branch → build → play → verdict) instead of the
/// auto-research loop. Selected via `local_projects.persona`.
const SYSTEM_PROMPT_GAME: &str = include_str!("../../SYSTEM_PROMPT_GAME.md");
/// The idea-foundry persona's playbook template — same token vocabulary, an
/// interview loop (infer → propose → name → capture) instead of a run loop; it
/// launches no compute. Selected via `local_projects.persona`.
const SYSTEM_PROMPT_IDEA: &str = include_str!("../../SYSTEM_PROMPT_IDEA.md");
/// The analyst persona's playbook template — evaluates an idea node against the
/// 35-signal rubric (keyless: agent codes signals, sim does the math).
const SYSTEM_PROMPT_ANALYST: &str = include_str!("../../SYSTEM_PROMPT_ANALYST.md");
/// The producer persona's playbook template — the orchestrator that runs the
/// discovery funnel by *suggesting* subagents (human approves), never doing the
/// worker jobs itself.
const SYSTEM_PROMPT_PRODUCER: &str = include_str!("../../SYSTEM_PROMPT_PRODUCER.md");
/// The Blender persona's playbook template — authors 3D assets in the user's
/// *running* Blender via the Blender MCP tools and exports for a game-designer to
/// load. Launches no compute; its output is a file.
const SYSTEM_PROMPT_BLENDER: &str = include_str!("../../SYSTEM_PROMPT_BLENDER.md");
/// The ComfyUI persona's playbook template — generates 2D assets on the local
/// ComfyUI via its MCP tools. Launches no experiment compute; its output is a
/// file, produced on a GPU shared with everything else on the machine.
const SYSTEM_PROMPT_COMFYUI: &str = include_str!("../../SYSTEM_PROMPT_COMFYUI.md");
const SYSTEM_PROMPT_ARTIST: &str = include_str!("../../SYSTEM_PROMPT_ARTIST.md");

/// Appended to EVERY persona's playbook (not just the producer) so any session
/// knows the one correct way to involve another agent. Without this, a worker
/// asked to "spawn an analyst" falls back to its harness's native sub-agent tool
/// (codex `subAgentActivity`, Claude `Task`) — same-provider, invisible to orx,
/// and its result is never tracked (it can finish and report nothing). Contains
/// `{session_id}`, resolved by the render-time token replace.
const DISPATCH_RULE: &str = "\
## Spawning or handing off to another agent

To involve another agent — an analyst to score an idea, a game-designer to build \
it, a fresh idea pass, a specialist on any provider — **never use your harness's \
own sub-agent / Task / subagent tool.** Those run same-provider and are invisible \
to orx: they don't show in the dashboard, don't nest in Recents, and their result \
is not tracked (a native subagent can finish and report nothing back — that is a \
silent failure, not a handoff). The ONLY correct way is to **suggest an orx \
sub-session** the human approves:

```sh
orx agent suggest --from-session {session_id} \\
  --persona <idea-foundry|analyst|game-designer|producer|blender|comfyui> \\
  --harness <claude-code|codex|opencode> --model <model-id> \\
  --parent <experimentNodeId> \\
  --task \"<what the subagent should do — name the node id>\" \\
  --why \"<one line: why this persona + provider fits>\"
```

`--from-session {session_id}` is THIS session: the suggestion renders as an \
approval card in this chat and nests the spawned agent under you in Recents. The \
human approves (and may swap provider/model); it becomes a first-class orx session \
— any provider — that reports back on its node. If the user names a provider/model \
(e.g. \"an analyst on gpt-5.6-sol\"), put it in the suggestion; they still approve.
";

/// A persona's playbook template with the leading repo-reader HTML comment
/// stripped — the text tokens are substituted into at render time, and what
/// the dashboard's Persona tab displays verbatim.
pub fn persona_template(persona: Persona) -> &'static str {
    let raw = match persona {
        Persona::Research => SYSTEM_PROMPT,
        Persona::GameDesigner => SYSTEM_PROMPT_GAME,
        Persona::IdeaFoundry => SYSTEM_PROMPT_IDEA,
        Persona::Analyst => SYSTEM_PROMPT_ANALYST,
        Persona::Producer => SYSTEM_PROMPT_PRODUCER,
        Persona::Blender => SYSTEM_PROMPT_BLENDER,
        Persona::Comfyui => SYSTEM_PROMPT_COMFYUI,
        Persona::GameArtist => SYSTEM_PROMPT_ARTIST,
    };
    raw.split_once("-->\n\n")
        .map(|(_, rest)| rest)
        .unwrap_or(raw)
}

fn playbook_md(project: &LocalProject, session_id: &str) -> String {
    playbook_md_with_memory(project, session_id, &super::memory::memory_section(project))
}

/// The render body, with the `{memory}` block passed in so tests can render
/// the playbook without touching the developer's real memory files.
fn playbook_md_with_memory(project: &LocalProject, session_id: &str, memory: &str) -> String {
    let id = &project.id;
    let name = &project.name;
    let repo = format!("{}/{}", project.github_owner, project.github_repo);
    let baseline = &project.baseline_branch;
    let files = super::files::files_dir(project)
        .to_string_lossy()
        .into_owned();
    let paper_line = project.paper_id.as_deref().map_or(String::new(), |p| {
        format!(
            "- Paper: arXiv {p} (https://arxiv.org/abs/{p}) — the paper this project starts \
             from; `orx paper {p}` fetches its report\n"
        )
    });
    // The default compute target (Settings → Compute) is read fresh on every
    // playbook rewrite, but how soon a rewrite reaches a live agent varies:
    // claude reads it at child spawn, so a rewrite reaches the agent on the next
    // respawn (config change / interrupt / crash), not every turn; codex only on
    // thread start/resume; a live opencode server keeps its playbook until
    // respawn (`AgentHost::ensure` early-returns for a running child). Launch-time
    // resolution in `exp run` stays authoritative either way: the agent is
    // told to OMIT `--backend`, never to echo the default back, so even a
    // stale prompt launches on the current default.
    let compute_default = crate::config::compute_default();
    let compute_bullet = match &compute_default {
        Some((b, f)) => {
            let flavor_part = f
                .as_ref()
                .map_or(String::new(), |f| format!(" (`--flavor {f}`)"));
            format!(
                "- Compute: default target **{b}**{flavor_part} — the user set it in \
                 Settings → Compute; omit `--backend` on `orx exp run` to launch there. \
                 Use another backend only when the user names one (see \"Compute backends\")"
            )
        }
        None => {
            "- Compute: backends — `hf`, `modal`, `k8s`, `ssh`, `slurm`, `ray`, or `local` —\n  \
                 chosen by the user per run; **there is no default backend** (see \"Compute\n  \
                 backends\")"
                .to_string()
        }
    };
    let backends_intro = match &compute_default {
        Some((b, f)) => {
            let flavor_part = f
                .as_ref()
                .map_or(String::new(), |f| format!(" --flavor {f}"));
            // The omit-instruction must match what a bare launch actually
            // needs: with a saved flavor both flags can go; a flavor-required
            // backend without one still needs --flavor; ssh always needs
            // --host. Contradicting the launch validation here sends the
            // agent into a guaranteed-failing command.
            let omit_hint = if f.is_some() {
                "Omit it (and `--flavor`) to use the default.".to_string()
            } else if super::FLAVOR_REQUIRED_BACKENDS.contains(&b.as_str()) {
                "Omit `--backend` to use the default, but still pass `--flavor` — no default \
                 flavor is saved."
                    .to_string()
            } else {
                "Omit it to use the default.".to_string()
            };
            let mut s = format!(
                "`orx exp run` launches on the user's configured default — **{b}{flavor_part}** \
                 — when you omit `--backend`. {omit_hint} \
                 Deviate only when the user names another backend for the task or this \
                 conversation — a connected token for some other backend is NOT a signal to \
                 switch."
            );
            if b == "ssh" {
                s.push_str(" (`--host <alias>` is still required on every launch.)");
            }
            s
        }
        None => "`orx exp run` requires an explicit `--backend` — **there is no default**.\n\
                 Which backend to use is the user's decision: if the task doesn't name one and\n\
                 the user hasn't already picked one in this conversation, ask before launching."
            .to_string(),
    };
    let launch_step = if compute_default.is_some() {
        "3. **Launch**: `orx exp run <expId>` — omitting `--backend` uses the default\n   \
         target (flags the default still needs are listed under \"Compute backends\") —\n   \
         or name one explicitly (`--flavor` for hf/modal/ray, `--host` for ssh/slurm; k8s\n   \
         reads the committed manifest; local takes no flags)."
    } else {
        "3. **Launch**: `orx exp run <expId> --backend <backend>` (`--flavor` for\n   \
         hf/modal/ray, `--host` for ssh/slurm; k8s reads the committed manifest; local\n   \
         takes no flags)."
    };
    // The persona picks the template and the skill set. The modular skills
    // are installed into this session's worktree (see
    // `agent_skills::ensure_session_skills`); the index is generated from the
    // same persona set so the playbook and the files on disk can never drift.
    let persona = project.persona();
    let skills_list = super::agent_skills::skills_for_persona(persona)
        .iter()
        .map(|s| format!("- **{}** — {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join("\n");
    // Every persona gets the shared cross-agent dispatch rule appended, so no
    // session ever falls back to its harness's native (invisible) subagent tool.
    let template = format!("{}\n\n{}", persona_template(persona), DISPATCH_RULE);
    template
        .replace("{name}", name)
        .replace("{id}", id)
        .replace("{session_id}", session_id)
        .replace("{repo}", &repo)
        .replace("{baseline}", baseline)
        .replace("{paper_line}", &paper_line)
        .replace("{compute_bullet}", &compute_bullet)
        .replace("{files}", &files)
        .replace("{skills_list}", &skills_list)
        .replace("{launch_step}", launch_step)
        .replace("{backends_intro}", &backends_intro)
        // Must stay LAST: later .replace calls rescan already-substituted
        // text, so memory content containing a literal `{files}`/`{id}` etc.
        // would get rewritten if this ran earlier.
        .replace("{memory}", memory)
}

/// Keep the files we drop into the checkout out of `git status` / accidental
/// commits via the local-only `.git/info/exclude` (never touches tracked
/// files or the repo's own `.gitignore`). Takes the **hub clone** path — its
/// `.git/info/exclude` is shared by every session worktree (a worktree's own
/// `.git` is just a pointer file). Best-effort.
fn exclude_agent_files(hub: &Path) {
    let path = hub.join(".git").join("info").join("exclude");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = [
        "opencode.json",
        ".openresearch/",
        ".claude/skills/",
        ".opencode/skills/",
        ".agents/skills/",
    ]
    .into_iter()
    .filter(|entry| !existing.lines().any(|l| l.trim() == *entry))
    .collect();
    if missing.is_empty() {
        return;
    }
    let mut block = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        block.push('\n');
    }
    for entry in missing {
        block.push_str(entry);
        block.push('\n');
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, block.as_bytes()));
}

/// Ensure the project's hub clone and this session's private worktree exist,
/// and write the autoresearch playbook into the worktree. Every harness
/// adapter injects this same file (opencode via config `instructions`, Claude
/// Code via `--append-system-prompt`, Codex via `developerInstructions` —
/// legacy exec: first-turn context). Returns
/// `(workdir, playbook)` — the worktree the harness runs in and the playbook
/// path inside it.
///
/// `session_skills_dir` is the harness's worktree-relative native-skills dir
/// (`.claude/skills`, `.opencode/skills`, `.agents/skills`); when `Some`, the
/// modular `orx` skills are written there too, fresh alongside the playbook, so
/// the session's own agent auto-loads them with zero drift.
pub fn ensure_playbook(
    project: &LocalProject,
    session_id: &str,
    session_skills_dir: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    let workdir = git::ensure_session_worktree(
        &project.github_owner,
        &project.github_repo,
        &project.baseline_branch,
        session_id,
    )?;
    let playbook = workdir.join(PLAYBOOK_REL);
    if let Some(parent) = playbook.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    std::fs::write(&playbook, playbook_md(project, session_id))
        .map_err(|e| anyhow!("Could not write {}: {}", playbook.display(), e))?;
    // Modular skills, written fresh beside the playbook (same freshness
    // semantics) so this session's agent discovers them natively.
    if let Some(dir) = session_skills_dir {
        super::agent_skills::ensure_session_skills(&workdir, dir, project.persona())?;
        // User-uploaded skills land beside the built-ins, same freshness.
        super::user_skills::write_into_session(&workdir, dir, &project.id)?;
    }
    // One shared exclude covers every worktree.
    exclude_agent_files(&git::clone_path(
        &project.github_owner,
        &project.github_repo,
    ));
    // The playbook points the agent at the files dir — make sure it exists.
    let _ = super::files::ensure_dir(project);
    // Same for the memory paths it advertises: parents must exist so any
    // harness's file tools can create the .md files on first write.
    super::memory::ensure_memory_dirs(project);
    Ok((workdir, playbook))
}

/// Write the opencode config + the playbook into the session's worktree
/// (self-healing via `ensure_session_worktree` if the cache was wiped).
/// Returns the worktree path plus, when the repo tracks its own
/// `opencode.json` (which we must never clobber — the agent commits and
/// pushes from this worktree), the path of our config to pass via
/// `OPENCODE_CONFIG` instead.
fn write_agent_files(
    project: &LocalProject,
    model: Option<&str>,
    session_id: &str,
) -> Result<(PathBuf, Option<PathBuf>)> {
    // Source of truth for the session-skills dir is the harness trait.
    use crate::local::harness::Harness;
    let skills_dir = crate::local::harness::opencode::OpenCode.session_skills_dir();
    let (repo, playbook) = ensure_playbook(project, session_id, skills_dir)?;
    let config_override = if git::is_tracked(&repo, "opencode.json") {
        // Out-of-root config: absolute instructions path (no root to anchor it).
        let path = repo
            .join(".openresearch")
            .join("agent")
            .join("opencode.json");
        std::fs::write(
            &path,
            opencode_config_json(model, &playbook.to_string_lossy()),
        )
        .map_err(|e| anyhow!("Could not write {}: {}", path.display(), e))?;
        Some(path)
    } else {
        std::fs::write(
            repo.join("opencode.json"),
            opencode_config_json(model, PLAYBOOK_REL),
        )
        .map_err(|e| anyhow!("Could not write opencode.json: {}", e))?;
        None
    };
    Ok((repo, config_override))
}

/// Wire status of one serve child for `GET /api/agent/status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

struct AgentChild {
    child: Child,
    port: u16,
    project_id: String,
    session_id: String,
    model: Option<String>,
}

impl AgentChild {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            running: true,
            port: Some(self.port),
            project_id: Some(self.project_id.clone()),
            session_id: Some(self.session_id.clone()),
            model: self.model.clone(),
        }
    }
}

/// Poll `/global/health` until opencode answers, watching for early exit.
async fn wait_healthy(child: &mut Child, port: u16) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let url = format!("http://127.0.0.1:{port}/global/health");
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "opencode exited during startup ({status}); see {}",
                agent_log_path().display()
            ));
        }
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "opencode did not become healthy on 127.0.0.1:{port} within {}s; see {}",
                HEALTH_TIMEOUT.as_secs(),
                agent_log_path().display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// Spawn `opencode serve` in the session's worktree and wait for it to come
/// up healthy.
async fn spawn_agent(
    project: &LocalProject,
    model: Option<&str>,
    session_id: &str,
) -> Result<AgentChild> {
    let bin = find_opencode()?;
    // The clone/worktree setup inside can hit the network; keep it off the
    // async workers.
    let (repo, config_override) = {
        let (project, model) = (project.clone(), model.map(str::to_string));
        let session = session_id.to_string();
        tokio::task::spawn_blocking(move || write_agent_files(&project, model.as_deref(), &session))
            .await
            .map_err(|e| anyhow!("agent file task failed: {e}"))??
    };
    // Best-effort: the playbook is the real guide; the shim just lets
    // opencode's skill tool surface `orx skill` too.
    if let Err(err) = crate::commands::install_skills::install_opencode_shim().await {
        eprintln!("warning: could not install the orx opencode skill: {err}");
    }
    let port = free_port()?;
    // The data dir may not exist yet (fresh machine, no Store::open before us).
    if let Some(parent) = agent_log_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(agent_log_path())
        .map_err(|e| anyhow!("Could not open {}: {}", agent_log_path().display(), e))?;

    let mut cmd = Command::new(&bin);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--hostname")
        .arg("127.0.0.1")
        // Without --print-logs the log file stays empty and startup failures
        // are undiagnosable.
        .arg("--print-logs")
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| anyhow!("{e}"))?))
        .stderr(Stdio::from(log))
        // Dies with `orx up` when the runtime drops the handle (Ctrl-C, exit).
        .kill_on_drop(true);
    // The agent shells out to plain `orx`; prepend this binary's dir so it
    // resolves to THIS orx (with local mode), not an older install on PATH.
    if let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) {
        if let Some(dir) = exe.parent() {
            let mut path = std::ffi::OsString::from(dir);
            match std::env::var_os("PATH") {
                Some(existing) if !existing.is_empty() => {
                    path.push(":");
                    path.push(existing);
                }
                _ => {}
            }
            cmd.env("PATH", path);
        }
    }
    // Vars saved in the dashboard's Environment tab reach the agent too;
    // the real process env still wins on conflicts.
    for (key, value) in crate::config::list_synced_env() {
        if std::env::var_os(&key).is_none() {
            cmd.env(key, value);
        }
    }
    // Tag runs the agent launches (`orx exp run`) with this session so the run
    // watcher notifies this chat. One serve child per session; set after the
    // synced-env loop so it isn't shadowed.
    crate::local::chat::set_chat_session_env(&mut cmd, session_id);
    if let Some(config) = &config_override {
        // The repo tracks its own opencode.json; ours rides OPENCODE_CONFIG.
        // Project configs load after OPENCODE_CONFIG and would override our
        // headless permission grants, so they are disabled for this child.
        cmd.env("OPENCODE_CONFIG", config)
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");
    }
    // Own process group: a terminal SIGINT reaches orx up alone, which then
    // tears the child down deliberately (kill_on_drop / shutdown()).
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not spawn {}: {}", bin.display(), e))?;
    if let Err(err) = wait_healthy(&mut child, port).await {
        let _ = child.kill().await;
        return Err(err);
    }
    Ok(AgentChild {
        child,
        port,
        project_id: project.id.clone(),
        session_id: session_id.to_string(),
        model: model.map(str::to_string),
    })
}

/// The `orx up` opencode host: one serve child per chat session, keyed by the
/// orx session id, each running in that session's worktree. Share as
/// `Arc<AgentHost>` in axum state.
pub struct AgentHost {
    /// `orx up --model` override, applied to every spawn.
    model_override: Option<String>,
    /// Serializes ensure() spawns (across all sessions — a spawn is seconds,
    /// and one at a time keeps clone/fetch traffic sane). Never taken by
    /// status()/port_for(), and `inner` is never held across a spawn — a slow
    /// clone or health poll must not block status reads or turn replies.
    spawn_lock: Mutex<()>,
    inner: Mutex<HashMap<String, AgentChild>>,
}

impl AgentHost {
    pub fn new(model_override: Option<String>) -> Self {
        Self {
            model_override,
            spawn_lock: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Status of every live child; reaps children that died behind our back.
    pub async fn status(&self) -> Vec<AgentStatus> {
        let mut guard = self.inner.lock().await;
        guard.retain(|_, agent| matches!(agent.child.try_wait(), Ok(None)));
        guard.values().map(AgentChild::status).collect()
    }

    /// Loopback port of the session's live server (for inline replies/aborts).
    pub async fn port_for(&self, session_id: &str) -> Option<u16> {
        let mut guard = self.inner.lock().await;
        let agent = guard.get_mut(session_id)?;
        if matches!(agent.child.try_wait(), Ok(None)) {
            Some(agent.port)
        } else {
            guard.remove(session_id);
            None
        }
    }

    /// Spawn (or reuse) the opencode server for this session. Idempotent when
    /// the session's server is already alive; a dead child is replaced.
    pub async fn ensure(&self, project: &LocalProject, session_id: &str) -> Result<AgentStatus> {
        let _spawning = self.spawn_lock.lock().await;
        {
            let mut guard = self.inner.lock().await;
            if let Some(agent) = guard.get_mut(session_id) {
                if agent.project_id == project.id && matches!(agent.child.try_wait(), Ok(None)) {
                    return Ok(agent.status());
                }
            }
            if let Some(mut old) = guard.remove(session_id) {
                let _ = old.child.kill().await; // kill() also reaps
            }
        }
        // inner released: status()/port reads keep answering while the spawn
        // (clone/fetch + health poll) is in flight instead of hanging.
        let agent = spawn_agent(project, self.model_override.as_deref(), session_id).await?;
        let status = agent.status();
        self.inner
            .lock()
            .await
            .insert(session_id.to_string(), agent);
        Ok(status)
    }

    /// Kill and reap one session's child (on session delete). No-op when the
    /// session has none.
    pub async fn kill_session(&self, session_id: &str) {
        if let Some(mut agent) = self.inner.lock().await.remove(session_id) {
            let _ = agent.child.kill().await;
        }
    }

    /// Kill and reap every child (also happens via kill_on_drop on exit).
    pub async fn shutdown(&self) {
        for (_, mut agent) in self.inner.lock().await.drain() {
            let _ = agent.child.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::agent_skills;

    fn sample_project(persona: Persona) -> LocalProject {
        LocalProject {
            id: "proj_test".into(),
            name: "Test Project".into(),
            slug: "test-project".into(),
            github_owner: "acme".into(),
            github_repo: "widget".into(),
            baseline_branch: "main".into(),
            repo_path: "/tmp/nonexistent".into(),
            run_command: None,
            paper_id: None,
            play_command: None,
            play_dir: None,
            persona: Some(persona.as_str().to_string()),
            auto_prompts: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Render with a fixed memory stub — tests must never read the
    /// developer's real memory files through `data_dir()`.
    fn sample_playbook(persona: Persona) -> String {
        let memory = crate::local::memory::render_memory_section(
            "/tmp/x/user.md",
            "/tmp/x/memory.md",
            None,
            None,
        );
        playbook_md_with_memory(&sample_project(persona), "chat_sample", &memory)
    }

    /// Each persona's playbook "## Skills" index must list exactly that
    /// persona's skills, in order — regenerate-and-compare so it can never
    /// freeze out of sync with `agent_skills::skills_for_persona` (the same
    /// set written into the session worktree).
    #[test]
    fn playbook_skills_index_matches_persona_set() {
        for persona in Persona::ALL {
            let md = sample_playbook(persona);
            let expected: Vec<String> = agent_skills::skills_for_persona(persona)
                .iter()
                .map(|s| format!("- **{}** — {}", s.name, s.description))
                .collect();

            // The "## Skills" section body: between the heading and the next `## `.
            let after = md
                .split("## Skills\n")
                .nth(1)
                .expect("no ## Skills section");
            let section = after.split("\n## ").next().unwrap();

            let listed: Vec<String> = section
                .lines()
                .filter(|l| l.starts_with("- **"))
                .map(str::to_string)
                .collect();
            assert_eq!(
                listed,
                expected,
                "{}: playbook Skills index drifted from persona set",
                persona.as_str()
            );
        }
    }

    /// Both playbooks keep their templated conditional logic — every
    /// placeholder must resolve (no leftover `{...}` braces from a botched
    /// edit in either template).
    #[test]
    fn playbook_has_no_unresolved_placeholders() {
        for persona in Persona::ALL {
            let md = sample_playbook(persona);
            // Every token a template may carry must be substituted — a typo'd
            // or newly added token that playbook_md doesn't know about fails
            // here.
            for token in [
                "{name}",
                "{id}",
                "{session_id}",
                "{repo}",
                "{baseline}",
                "{paper_line}",
                "{compute_bullet}",
                "{files}",
                "{skills_list}",
                "{launch_step}",
                "{backends_intro}",
                "{memory}",
            ] {
                assert!(
                    !md.contains(token),
                    "{}: unresolved placeholder {token}",
                    persona.as_str()
                );
            }
            // The template's leading HTML comment (repo-reader documentation)
            // must be stripped — the prompt starts at the persona's title.
            let title = match persona {
                Persona::Research => "# OpenResearch local agent",
                Persona::GameDesigner => "# OpenResearch game-design agent",
                Persona::IdeaFoundry => "# OpenResearch idea agent",
                Persona::Analyst => "# OpenResearch analyst agent",
                Persona::Producer => "# OpenResearch producer agent",
                Persona::Blender => "# OpenResearch Blender agent",
                Persona::Comfyui => "# OpenResearch ComfyUI agent",
                Persona::GameArtist => "# OpenResearch game artist",
            };
            assert!(md.starts_with(title), "template comment not stripped");
            assert!(!md.contains("<!--"), "HTML comment leaked into the prompt");
            // Sanity: the slimmed pointers to the modules survived, and each
            // persona points only at its own modules.
            match persona {
                Persona::Research => {
                    assert!(md.contains("orx-compute"));
                    assert!(md.contains("orx-evidence"));
                    assert!(md.contains("orx-reports"));
                    assert!(md.contains("orx-lit"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-ideate"));
                }
                Persona::GameDesigner => {
                    assert!(md.contains("orx-compute"));
                    assert!(md.contains("orx-evidence"));
                    assert!(md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                    assert!(!md.contains("orx-ideate"));
                }
                // The interview persona launches nothing — no compute, evidence,
                // or play modules, just the single ideate skill.
                Persona::IdeaFoundry => {
                    assert!(md.contains("orx-ideate"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                }
                // The analyst evaluates: the evaluate skill + git (to commit the
                // coding sidecar), nothing else.
                Persona::Analyst => {
                    assert!(md.contains("orx-evaluate"));
                    assert!(md.contains("orx-git"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                    assert!(!md.contains("orx-ideate"));
                }
                // The producer orchestrates via one skill; it launches no runs
                // and does no worker job itself.
                Persona::Producer => {
                    assert!(md.contains("orx-produce"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                }
                // Author an asset and commit it: the blender skill plus git.
                // Launches nothing, so no compute/evidence/play modules.
                Persona::Blender => {
                    assert!(md.contains("orx-blender"));
                    assert!(md.contains("orx-git"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                    assert!(!md.contains("orx-ideate"));
                }
                // Same shape as Blender, and it must not claim Blender's tools
                // either — the two hand work to each other and should stay
                // distinct about what they can actually do.
                Persona::Comfyui => {
                    assert!(md.contains("orx-comfyui"));
                    assert!(md.contains("orx-git"));
                    assert!(!md.contains("orx-blender"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                    assert!(!md.contains("orx-ideate"));
                }
                // The artist owns the outcome across both backends, so unlike
                // Comfyui it legitimately carries orx-blender too. It still
                // launches nothing, so no compute/evidence/play modules.
                Persona::GameArtist => {
                    assert!(md.contains("orx-animation"));
                    assert!(md.contains("orx-comfyui"));
                    assert!(md.contains("orx-blender"));
                    assert!(md.contains("orx-git"));
                    assert!(!md.contains("orx-compute"));
                    assert!(!md.contains("orx-evidence"));
                    assert!(!md.contains("orx-play"));
                    assert!(!md.contains("orx-reports"));
                    assert!(!md.contains("orx-lit"));
                    assert!(!md.contains("orx-ideate"));
                }
            }
            // The memory section rendered with both scopes present.
            assert!(md.contains("## Memory"));
            assert!(md.contains("### User memory"));
            assert!(md.contains("### Project memory"));
        }
    }
}
