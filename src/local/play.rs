//! Playable builds — build an experiment branch's tip (detached worktree +
//! the project's play command) into a static dir served at `/play/<expId>/`,
//! tracked as a `kind: play` run. Shared by the dashboard's Play button
//! handler and the auto-build hook on experiment creation, so a variant is
//! playable the moment its card exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::{now_ms, Store, StoredRun};

pub const DEFAULT_PLAY_COMMAND: &str = "npm run build";
pub const DEFAULT_PLAY_DIR: &str = "dist";

fn play_dir_rel(project: &LocalProject) -> &str {
    project
        .play_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(DEFAULT_PLAY_DIR)
}

/// Where an experiment's playable build is served from.
pub fn play_serve_dir(project: &LocalProject, experiment_id: &str) -> PathBuf {
    super::git::play_worktree_path(&project.github_owner, &project.github_repo, experiment_id)
        .join(play_dir_rel(project))
}

/// Outcome of a build request — mirrors the Play button's response states.
pub enum PlayBuild {
    /// A build is already in flight; its run id.
    Building(String),
    /// The served dir already reflects the branch tip — nothing to do.
    Ready,
    /// A new build run was launched; its run id.
    Started(String),
}

/// Build (or reuse) the experiment's playable: a detached worktree at the
/// branch head + the project's play command, tracked as a `kind: play` run.
/// The build script ends with a **playability lint**: root-absolute asset
/// references in the built HTML/CSS 404 under `/play/<expId>/`, so a build
/// that emits them fails with an actionable message instead of serving a
/// blank page. Synchronous (shells out to git) — call off the async workers.
pub fn start_play_build(
    store: &Store,
    project: &LocalProject,
    exp: &LocalExperiment,
) -> Result<PlayBuild> {
    // The branch head in the hub clone — what the build must reflect. Play
    // builds serve local work; no push/remote round-trip.
    let head = super::git::resolve_branch_commit(Path::new(&project.repo_path), &exp.branch_name)?
        .ok_or_else(|| anyhow!("experiment branch has no commits to build"))?;

    let play_runs: Vec<_> = store
        .list_runs_by_experiment(&exp.id)?
        .into_iter()
        .filter(|r| r.kind == "play")
        .collect();
    if let Some(r) = play_runs.iter().find(|r| !super::is_terminal(&r.status)) {
        return Ok(PlayBuild::Building(r.id.clone()));
    }
    let fresh = play_runs
        .iter()
        .find(|r| r.status == "done")
        .is_some_and(|r| r.commit_sha.as_deref() == Some(head.as_str()));
    if fresh && play_serve_dir(project, &exp.id).is_dir() {
        return Ok(PlayBuild::Ready);
    }

    let play_command = project
        .play_command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(DEFAULT_PLAY_COMMAND)
        .to_string();
    let worktree =
        super::git::play_worktree_path(&project.github_owner, &project.github_repo, &exp.id);
    let sq = crate::jobs::ssh::sh_quote;
    // `set -e` so a failed step fails the run; prune first so a hand-deleted
    // worktree dir doesn't wedge `worktree add` forever. The trailing lint
    // greps the built output for root-absolute references (src="/…",
    // href="/…", url(/…)) — the char class excludes `/` so protocol-relative
    // `//cdn…` URLs pass — and fails the build with the fix spelled out.
    let script = format!(
        "set -e\n\
         HUB={hub}\n\
         WT={wt}\n\
         SHA={sha}\n\
         git -C \"$HUB\" worktree prune\n\
         if [ ! -e \"$WT/.git\" ]; then\n\
           git -C \"$HUB\" worktree add --detach \"$WT\" \"$SHA\"\n\
         else\n\
           git -C \"$WT\" checkout --detach \"$SHA\"\n\
           git -C \"$WT\" reset --hard \"$SHA\"\n\
         fi\n\
         cd \"$WT\"\n\
         if [ -f package.json ] && [ ! -d node_modules ]; then\n\
           if [ -f package-lock.json ]; then npm ci; else npm install; fi\n\
         fi\n\
         {cmd}\n\
         PD={play_dir}\n\
         if [ -d \"$PD\" ]; then\n\
           BAD=$(grep -RnE \"(src|href)=[\\\"']/[A-Za-z0-9_]|url\\\\(/[A-Za-z0-9_]\" \"$PD\" --include='*.html' --include='*.css' | head -15 || true)\n\
           if [ -n \"$BAD\" ]; then\n\
             echo 'orx: PLAYABILITY CHECK FAILED — the build references root-absolute paths, which 404 when served under /play/<experiment>/:'\n\
             echo \"$BAD\"\n\
             echo \"orx: make the build subpath-portable — Vite: set base: './' in vite.config; plain HTML/CSS: use relative asset paths — then rebuild.\"\n\
             exit 1\n\
           fi\n\
         fi\n",
        hub = sq(&project.repo_path),
        wt = sq(&worktree.to_string_lossy()),
        sha = sq(&head),
        cmd = play_command,
        play_dir = sq(play_dir_rel(project)),
    );

    let run_id = uuid::Uuid::new_v4().to_string();
    let env: HashMap<String, String> = crate::config::list_synced_env().into_iter().collect();
    let dir = crate::jobs::localbox::run_job(&crate::jobs::localbox::LocalJobSpec {
        run_id: run_id.clone(),
        script,
        env,
    })?;
    let descriptor = crate::jobs::BackendDescriptor {
        kind: "local_job".to_string(),
        job_id: Some(dir.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let run = StoredRun {
        id: run_id.clone(),
        experiment_id: exp.id.clone(),
        project_id: project.id.clone(),
        status: "starting".to_string(),
        backend_json: descriptor.to_json(),
        command: play_command,
        created_at: now_ms(),
        updated_at: now_ms(),
        ended_at: None,
        exit_code: None,
        commit_sha: Some(head),
        result_markdown: None,
        cancel_requested: false,
        supervisor_heartbeat_ms: None,
        kind: "play".to_string(),
        metrics_json: None,
        verdict: None,
        verdict_notes: None,
        verdict_at: None,
    };
    store.upsert_run(&run)?;
    crate::commands::exp::spawn_detached_supervise(&run_id)?;
    Ok(PlayBuild::Started(run_id))
}

/// Whether this project uses the play surface at all — the gate for the
/// auto-build on experiment creation. True for game-designer projects, any
/// project with a play command configured, or one that has already had a
/// play build.
pub fn has_play_surface(store: &Store, project: &LocalProject) -> bool {
    if project.persona() == super::agent_skills::Persona::GameDesigner {
        return true;
    }
    if project
        .play_command
        .as_deref()
        .is_some_and(|c| !c.trim().is_empty())
    {
        return true;
    }
    store
        .list_runs_by_project(&project.id)
        .map(|rs| rs.iter().any(|r| r.kind == "play"))
        .unwrap_or(false)
}

/// Best-effort auto-build on experiment creation: a variant should be
/// playable the moment its card exists. Failures only log — creating an
/// experiment must never fail because a build couldn't start (empty baseline
/// branch on a blank project, for example).
pub fn auto_build(store: &Store, project: &LocalProject, exp: &LocalExperiment) {
    if !has_play_surface(store, project) {
        return;
    }
    if let Err(e) = start_play_build(store, project, exp) {
        eprintln!("orx: auto play build for {} skipped: {e}", exp.id);
    }
}
