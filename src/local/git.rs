//! Git operations for local mode — shell out to the `git` binary (already a
//! hard dependency of the workflow; no libgit2). Clones live at
//! `~/.cache/openresearch/repos/<owner>/<repo>`, the same convention SKILL.md
//! documents for manual diffing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{anyhow, Result};

pub fn clones_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("openresearch")
        .join("repos")
}

pub fn clone_path(owner: &str, repo: &str) -> PathBuf {
    clones_root().join(owner).join(repo)
}

/// Root for per-chat-session worktrees of a repo's hub clone
/// (`~/.cache/openresearch/worktrees/<owner>/<repo>/<session-id>`). Kept
/// outside `repos/` so a worktree can never collide with a real repo name.
pub fn worktrees_root(owner: &str, repo: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("openresearch")
        .join("worktrees")
        .join(owner)
        .join(repo)
}

pub fn session_worktree_path(owner: &str, repo: &str, session_id: &str) -> PathBuf {
    worktrees_root(owner, repo).join(session_id)
}

/// Detached worktree a playable experiment build runs in
/// (`~/.cache/openresearch/play-builds/<owner>/<repo>/<experiment-id>`).
/// Its own root — never the chat session's worktree, so builds can't race
/// the agent's edits.
pub fn play_worktree_path(owner: &str, repo: &str, experiment_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("openresearch")
        .join("play-builds")
        .join(owner)
        .join(repo)
        .join(experiment_id)
}

/// Default ssh for headless git: never prompt, and give up on an unreachable
/// host in bounded time — a filtered port 22 must fail the ssh leg, not park
/// a server worker for minutes.
const SSH_BATCH: &str = "ssh -oBatchMode=yes -oConnectTimeout=10";

/// Run git with `args`, returning trimmed stdout; failures carry git's stderr.
/// Headless: git must fail fast rather than prompt on /dev/tty (these calls
/// run under a server, where a prompt would hang a worker forever).
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        cmd.env("GIT_SSH_COMMAND", SSH_BATCH);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `GITHUB_TOKEN` env, else the synced env file (UI-pasted token), else
/// `gh auth token`, else None (public-repo fallback).
pub fn resolve_github_token() -> Option<String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    if let Some(t) = crate::config::synced_env_var("GITHUB_TOKEN") {
        return Some(t);
    }
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Fail early on a typo'd baseline branch — otherwise it only surfaces much
/// later as an opaque `git push` refspec error on the first run.
fn assert_branch_exists(dir: &Path, owner: &str, repo: &str, branch: &str) -> Result<()> {
    let remote = format!("refs/remotes/origin/{branch}");
    if git(Some(dir), &["rev-parse", "--verify", "--quiet", &remote]).is_err() {
        return Err(anyhow!(
            "Branch '{branch}' not found in {owner}/{repo} — check the project's baseline branch."
        ));
    }
    Ok(())
}

/// One tick of clone progress: git's phase line ("Receiving objects", …) and
/// the parsed percent when the line carries one. Serialized camelCase — the
/// SSE `project.clone.progress` payload is this struct plus owner/repo.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneProgress {
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
}

/// Parse one stderr record from `git clone --progress` into a progress tick.
/// Records look like `Receiving objects:  47% (1234/2620), 5.2 MiB | 3.4 MiB/s`
/// (rewrites are `\r`-separated); phase-only records ("Enumerating objects:
/// 262, done.") yield no percent. Anything else returns `None`.
fn parse_clone_progress(record: &str) -> Option<CloneProgress> {
    let (phase, rest) = record.trim().split_once(':')?;
    let phase = phase.trim();
    if !matches!(
        phase,
        "Enumerating objects"
            | "Counting objects"
            | "Compressing objects"
            | "Receiving objects"
            | "Resolving deltas"
            | "Updating files"
    ) {
        return None;
    }
    let percent = rest
        .split('%')
        .next()
        .and_then(|n| n.trim().parse::<u8>().ok());
    Some(CloneProgress {
        phase: phase.to_string(),
        percent,
    })
}

/// How long the clone's stderr may go silent before the child is killed. A
/// healthy clone chatters (progress rewrites many times a second); sustained
/// silence means a dead network, and the caller must get an error instead of
/// a worker parked forever.
const CLONE_STALL: std::time::Duration = std::time::Duration::from_secs(120);

/// `git clone --progress` with parsed progress callbacks (throttled to phase
/// edges / ~150 ms) and the stall watchdog above. `extra` carries additional
/// clone flags (`--depth=1`, `--branch`, …). On failure the error carries
/// the tail of git's non-progress stderr. Does NOT clean up a partial target
/// dir — the caller owns that (a killed clone leaves one behind).
fn clone_with_progress(
    url: &str,
    target: &str,
    extra: &[&str],
    on_progress: &(dyn Fn(CloneProgress) + Send + Sync),
) -> Result<()> {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        cmd.env("GIT_SSH_COMMAND", SSH_BATCH);
    }
    let mut child = cmd
        .args(["clone", "--progress"])
        .args(extra)
        .args([url, target])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let mut stderr = child.stderr.take().expect("stderr was piped");

    // Reader thread chunks stderr into records split on BOTH `\r` and `\n`
    // (git rewrites progress lines with carriage returns). The main thread
    // keeps ownership of the child so the watchdog can kill it; the child's
    // death closes stderr, which unblocks and ends the reader.
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if b == b'\r' || b == b'\n' {
                            if !pending.is_empty() {
                                let rec = String::from_utf8_lossy(&pending).into_owned();
                                pending.clear();
                                if tx.send(rec).is_err() {
                                    return;
                                }
                            }
                        } else {
                            pending.push(b);
                        }
                    }
                }
            }
        }
        if !pending.is_empty() {
            let _ = tx.send(String::from_utf8_lossy(&pending).into_owned());
        }
    });

    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut last_record = Instant::now();
    let mut last_emit: Option<(String, Instant)> = None;
    let mut stalled = false;
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(rec) => {
                last_record = Instant::now();
                if tail.len() >= 20 {
                    tail.pop_front();
                }
                tail.push_back(rec.clone());
                if let Some(p) = parse_clone_progress(&rec) {
                    let emit = match &last_emit {
                        Some((phase, at)) => {
                            *phase != p.phase
                                || p.percent == Some(100)
                                || at.elapsed() >= Duration::from_millis(150)
                        }
                        None => true,
                    };
                    if emit {
                        last_emit = Some((p.phase.clone(), Instant::now()));
                        on_progress(p);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_record.elapsed() >= CLONE_STALL {
                    let _ = child.kill();
                    stalled = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| anyhow!("git clone: {}", e))?;
    let _ = reader.join();
    if stalled {
        return Err(anyhow!(
            "git clone {url} stalled (no output for {}s) and was killed — check the network",
            CLONE_STALL.as_secs()
        ));
    }
    if !status.success() {
        // The interesting lines are the non-progress ones (fatal:, auth
        // errors); progress rewrites are noise in an error message.
        let msg = tail
            .iter()
            .filter(|l| parse_clone_progress(l).is_none() && !l.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let msg = if msg.is_empty() {
            format!("exit {status}")
        } else {
            msg
        };
        return Err(anyhow!("git clone {url} failed: {msg}"));
    }
    Ok(())
}

/// Clone `owner/repo` into the cache (ssh first, then https) or, when the
/// clone already exists, fetch. Validates that `baseline_branch` exists on
/// the remote. Returns the clone path.
pub fn ensure_clone(owner: &str, repo: &str, baseline_branch: &str) -> Result<PathBuf> {
    ensure_clone_with_progress(owner, repo, baseline_branch, &|_| {})
}

/// [`ensure_clone`] with clone progress fed to `on_progress` — the New
/// Project flow forwards these over SSE. A failed or killed leg's partial
/// clone dir is removed before the next attempt (git cleans up after its own
/// failures, but not after a watchdog kill).
pub fn ensure_clone_with_progress(
    owner: &str,
    repo: &str,
    baseline_branch: &str,
    on_progress: &(dyn Fn(CloneProgress) + Send + Sync),
) -> Result<PathBuf> {
    let dir = clone_path(owner, repo);
    if dir.join(".git").is_dir() {
        git(Some(&dir), &["fetch", "origin"])?;
        assert_branch_exists(&dir, owner, repo, baseline_branch)?;
        return Ok(dir);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let target = dir.to_string_lossy().to_string();
    let cleanup = || {
        if !dir.join(".git").is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    };
    // Test seam: ORX_GIT_REMOTE_BASE=file:///some/root clones <base>/<owner>/<repo>.
    if let Ok(base) = std::env::var("ORX_GIT_REMOTE_BASE") {
        let url = format!("{}/{owner}/{repo}", base.trim_end_matches('/'));
        clone_with_progress(&url, &target, &[], on_progress).inspect_err(|_| cleanup())?;
        assert_branch_exists(&dir, owner, repo, baseline_branch)?;
        return Ok(dir);
    }
    let ssh = format!("git@github.com:{owner}/{repo}.git");
    let https = format!("https://github.com/{owner}/{repo}.git");
    on_progress(CloneProgress {
        phase: "Connecting".to_string(),
        percent: None,
    });
    // ssh covers private repos with keys; https covers public repos and
    // credential-helper setups. Surface the https error (the common path).
    if clone_with_progress(&ssh, &target, &[], on_progress).is_err() {
        cleanup();
        if let Err(err) = clone_with_progress(&https, &target, &[], on_progress) {
            cleanup();
            return Err(anyhow!(
                "Could not clone {owner}/{repo} (tried ssh and https): {err}"
            ));
        }
    }
    assert_branch_exists(&dir, owner, repo, baseline_branch)?;
    Ok(dir)
}

/// Ensure a private worktree of the hub clone for one chat session, so
/// parallel agents on the same project never share (or stomp) a checkout.
/// Worktrees share the hub's object store and refs: a branch created in one
/// is immediately visible in all, one `fetch` updates everyone, and git
/// refuses to check out a branch that another worktree already holds.
///
/// The worktree starts **detached** on the baseline tip — checking out the
/// baseline branch itself would claim it and block every sibling; the agent
/// checks out its own experiment branch from there.
pub fn ensure_session_worktree(
    owner: &str,
    repo: &str,
    baseline_branch: &str,
    session_id: &str,
) -> Result<PathBuf> {
    let hub = ensure_clone(owner, repo, baseline_branch)?;
    let dir = session_worktree_path(owner, repo, session_id);
    if dir.join(".git").exists() {
        // `.git` is a gitdir-pointer file in a worktree. Validate it — a wiped
        // hub clone (cache cleared, then re-cloned by ensure_clone above)
        // orphans old worktrees, which must be rebuilt, not returned.
        if git(Some(&dir), &["rev-parse", "--is-inside-work-tree"]).is_ok() {
            return Ok(dir);
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|e| anyhow!("Could not remove stale worktree {}: {}", dir.display(), e))?;
    }
    // A manually deleted worktree dir leaves a stale registration behind that
    // would make `worktree add` at the same path fail.
    let _ = git(Some(&hub), &["worktree", "prune"]);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let target = dir.to_string_lossy().to_string();
    let base = format!("refs/remotes/origin/{baseline_branch}");
    git(Some(&hub), &["worktree", "add", "--detach", &target, &base])?;
    Ok(dir)
}

/// Remove a session's worktree (on session/project delete). Uncommitted
/// scratch is discarded deliberately — real work is committed and pushed per
/// the playbook contract. Best-effort: cleanup must never block the delete.
pub fn remove_session_worktree(owner: &str, repo: &str, session_id: &str) {
    let dir = session_worktree_path(owner, repo, session_id);
    if !dir.exists() {
        return;
    }
    let hub = clone_path(owner, repo);
    if hub.join(".git").is_dir() {
        let _ = git(
            Some(&hub),
            &["worktree", "remove", "--force", &dir.to_string_lossy()],
        );
        let _ = git(Some(&hub), &["worktree", "prune"]);
    }
    // Hub gone (cache wiped) or `worktree remove` refused: take the dir anyway.
    let _ = std::fs::remove_dir_all(&dir);
}

/// Seed a fresh (empty) GitHub repo from the tip of another repo — the
/// fork-by-copy the platform does on import. Shallow-clones the source
/// (`src_branch`, or its default branch), re-roots the snapshot as a single
/// orphan commit (a shallow tip's parents aren't in the clone, so pushing it
/// as-is would be rejected), and pushes it as the new repo's `main`.
pub fn seed_copy(
    src_owner: &str,
    src_repo: &str,
    src_branch: Option<&str>,
    dst_owner: &str,
    dst_repo: &str,
    on_progress: &(dyn Fn(CloneProgress) + Send + Sync),
) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("orx-seed-{}", uuid::Uuid::new_v4()));
    let result = seed_copy_in(
        &tmp,
        src_owner,
        src_repo,
        src_branch,
        dst_owner,
        dst_repo,
        on_progress,
    );
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[allow(clippy::too_many_arguments)]
fn seed_copy_in(
    dir: &Path,
    src_owner: &str,
    src_repo: &str,
    src_branch: Option<&str>,
    dst_owner: &str,
    dst_repo: &str,
    on_progress: &(dyn Fn(CloneProgress) + Send + Sync),
) -> Result<()> {
    let target = dir.to_string_lossy().to_string();
    let mut extra = vec!["--depth=1", "--single-branch"];
    if let Some(branch) = src_branch {
        extra.extend(["--branch", branch]);
    }
    // ssh first, https fallback — same auth order as ensure_clone.
    let ssh = format!("git@github.com:{src_owner}/{src_repo}.git");
    let https = format!("https://github.com/{src_owner}/{src_repo}.git");
    on_progress(CloneProgress {
        phase: "Connecting".to_string(),
        percent: None,
    });
    if clone_with_progress(&ssh, &target, &extra, on_progress).is_err() {
        let _ = std::fs::remove_dir_all(dir);
        if let Err(err) = clone_with_progress(&https, &target, &extra, on_progress) {
            let _ = std::fs::remove_dir_all(dir);
            return Err(anyhow!(
                "Could not clone {src_owner}/{src_repo} (tried ssh and https): {err}"
            ));
        }
    }
    git(Some(dir), &["checkout", "--orphan", "orx-seed"])?;
    git(Some(dir), &["add", "-A"])?;
    // An empty source stages nothing; seed the stub a blank project gets.
    if git(Some(dir), &["status", "--porcelain"])?.is_empty() {
        std::fs::write(dir.join("README.md"), format!("# {dst_repo}\n"))
            .map_err(|e| anyhow!("Could not write README.md: {}", e))?;
        git(Some(dir), &["add", "-A"])?;
    }
    git(
        Some(dir),
        &[
            "-c",
            "user.name=orx",
            "-c",
            "user.email=orx@openresearch.sh",
            "commit",
            "-m",
            &format!("orx: import {src_owner}/{src_repo}"),
        ],
    )?;
    let dst_ssh = format!("git@github.com:{dst_owner}/{dst_repo}.git");
    let dst_https = format!("https://github.com/{dst_owner}/{dst_repo}.git");
    on_progress(CloneProgress {
        phase: "Pushing copy".to_string(),
        percent: None,
    });
    if git(Some(dir), &["push", &dst_ssh, "HEAD:main"]).is_err() {
        git(Some(dir), &["push", &dst_https, "HEAD:main"])?;
    }
    Ok(())
}

/// Create `new_branch` from `parent_branch`'s tip and push it to origin —
/// the branch must exist on GitHub before an HF job can clone it.
pub fn create_experiment_branch(
    repo_path: &Path,
    parent_branch: &str,
    new_branch: &str,
) -> Result<()> {
    git(Some(repo_path), &["fetch", "origin"])?;
    // Prefer the remote tip; a never-pushed parent falls back to the local ref.
    let remote_parent = format!("refs/remotes/origin/{parent_branch}");
    let base = if git(Some(repo_path), &["rev-parse", "--verify", &remote_parent]).is_ok() {
        remote_parent
    } else {
        parent_branch.to_string()
    };
    // -f: a stale local branch is residue from an earlier failed attempt (a
    // live branch would have an experiment row and its slug never re-picked).
    git(
        Some(repo_path),
        &["branch", "--no-track", "-f", new_branch, &base],
    )?;
    if let Err(err) = git(Some(repo_path), &["push", "-u", "origin", new_branch]) {
        // Leave nothing behind — a retry re-picks the same slug.
        let _ = git(Some(repo_path), &["branch", "-D", new_branch]);
        return Err(err);
    }
    Ok(())
}

/// Result of a checkout-free merge attempt (see [`merge_branch_tips`]).
pub enum MergeOutcome {
    /// The merge commit landed on `new_branch` and was pushed.
    Merged,
    /// Content conflicts — the branch is left at its fork point; the summary
    /// names the conflicted paths for the agent to resolve in its worktree.
    Conflicts(String),
    /// This git lacks `merge-tree --write-tree` (< 2.38) — merge left to the
    /// agent.
    Unsupported,
}

/// Merge `merge_branch`'s tip into `new_branch` (currently at `parent_tip`)
/// **without a checkout** — agent worktrees may hold any of these branches,
/// so the hub clone's working tree must not be touched. Plumbing:
/// `merge-tree --write-tree` builds the merged tree, `commit-tree` wraps it
/// in a two-parent commit, `update-ref` moves the branch, `push` publishes.
pub fn merge_branch_tips(
    repo_path: &Path,
    new_branch: &str,
    parent_tip: &str,
    merge_branch: &str,
) -> Result<MergeOutcome> {
    let merge_tip = resolve_branch_commit(repo_path, merge_branch)?
        .ok_or_else(|| anyhow!("merge source '{merge_branch}' has no commits"))?;
    // Direct invocation, not the `git()` helper: a conflicted merge exits 1
    // with the report on STDOUT (tree OID first, then — with --name-only —
    // the conflicted paths), which the helper would discard.
    let out = Command::new("git")
        .current_dir(repo_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            parent_tip,
            &merge_tip,
        ])
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tree = match out.status.code() {
        Some(0) => stdout.lines().next().unwrap_or_default().trim().to_string(),
        Some(1) => {
            // Exit 1 = content conflicts. The first stdout block is the
            // conflicted tree OID followed by the conflicted paths
            // (--name-only); a blank line then informational messages we
            // don't want in the summary.
            let conflicted = stdout
                .split("\n\n")
                .next()
                .unwrap_or("")
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .skip(1)
                .take(10)
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(MergeOutcome::Conflicts(conflicted));
        }
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("--write-tree") || stderr.contains("unknown option") {
                return Ok(MergeOutcome::Unsupported);
            }
            return Err(anyhow!("git merge-tree failed: {}", stderr.trim()));
        }
    };
    if tree.is_empty() {
        return Err(anyhow!("merge-tree returned no tree id"));
    }
    let commit = git(
        Some(repo_path),
        &[
            "commit-tree",
            &tree,
            "-p",
            parent_tip,
            "-p",
            &merge_tip,
            "-m",
            &format!("orx: merge {merge_branch}"),
        ],
    )?;
    git(
        Some(repo_path),
        &[
            "update-ref",
            &format!("refs/heads/{new_branch}"),
            commit.trim(),
        ],
    )?;
    git(Some(repo_path), &["push", "origin", new_branch])?;
    Ok(MergeOutcome::Merged)
}

/// Head SHA of a branch — the *remote* tip when it exists (that's what a job
/// clones), the local ref otherwise. The opposite preference of
/// `resolve_branch_commit`, which serves the code browser and wants the
/// agent's not-yet-pushed local work.
pub fn branch_head_sha(repo_path: &Path, branch: &str) -> Result<String> {
    let remote = format!("refs/remotes/origin/{branch}");
    if let Ok(sha) = git(Some(repo_path), &["rev-parse", &remote]) {
        return Ok(sha);
    }
    git(Some(repo_path), &["rev-parse", branch])
}

/// Whether origin already has the branch (a cheap network check).
pub fn branch_on_remote(repo_path: &Path, branch: &str) -> Result<bool> {
    let out = git(Some(repo_path), &["ls-remote", "--heads", "origin", branch])?;
    Ok(!out.is_empty())
}

/// A file's content at a specific commit (`git show <sha>:<path>`), i.e.
/// exactly what a job cloning that sha will see — not the working tree.
pub fn file_at(repo_path: &Path, sha: &str, path: &str) -> Result<String> {
    git(Some(repo_path), &["show", &format!("{sha}:{path}")])
}

/// Whether the repo tracks `path` (local check, no network).
pub fn is_tracked(repo_path: &Path, path: &str) -> bool {
    git(
        Some(repo_path),
        &["ls-files", "--error-unmatch", "--", path],
    )
    .is_ok()
}

pub fn push_branch(repo_path: &Path, branch: &str) -> Result<()> {
    git(Some(repo_path), &["push", "-u", "origin", branch])?;
    Ok(())
}

// --- diffs ------------------------------------------------------------------

/// Whole-diff cap, mirroring the OpenResearch api's MAX_DIFF_BYTES.
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

pub struct DiffPayload {
    pub diff: String,
    pub truncated: bool,
    pub bytes_read: usize,
}

pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    /// Unix seconds.
    pub committed_at: i64,
}

/// Like `git` but raw stdout bytes, no trim, and extra tolerated exit codes
/// (`git diff --no-index` exits 1 when the files differ).
fn git_bytes(dir: &Path, args: &[&str], ok_codes: &[i32]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let code = out.status.code().unwrap_or(-1);
    if !out.status.success() && !ok_codes.contains(&code) {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn cap_diff(mut bytes: Vec<u8>) -> DiffPayload {
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    if truncated {
        bytes.truncate(MAX_DIFF_BYTES);
    }
    let bytes_read = bytes.len();
    // lossy: the cap can land mid multibyte char
    DiffPayload {
        diff: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
        bytes_read,
    }
}

/// Resolve a branch name or sha to something git can diff. Prefers the local
/// ref (where the agent works) and falls back to origin.
fn resolve_commitish(repo: &Path, name: &str) -> Result<String> {
    for cand in [name.to_string(), format!("refs/remotes/origin/{name}")] {
        let probe = format!("{cand}^{{commit}}");
        if git(Some(repo), &["rev-parse", "--verify", "--quiet", &probe]).is_ok() {
            return Ok(cand);
        }
    }
    Err(anyhow!("unknown git ref: {name}"))
}

/// Cumulative diff `base...head` (merge-base semantics, same as the cloud
/// compare endpoint).
pub fn diff_range(repo: &Path, base: &str, head: &str) -> Result<DiffPayload> {
    let base = resolve_commitish(repo, base)?;
    let head = resolve_commitish(repo, head)?;
    let range = format!("{base}...{head}");
    Ok(cap_diff(git_bytes(
        repo,
        &["--no-pager", "diff", &range],
        &[],
    )?))
}

/// Single-commit diff. `git show` handles root commits, unlike `sha~1..sha`.
pub fn commit_diff(repo: &Path, sha: &str) -> Result<DiffPayload> {
    Ok(cap_diff(git_bytes(
        repo,
        &["--no-pager", "show", "--format=", "--patch", sha],
        &[],
    )?))
}

fn parse_commit_lines(out: &str) -> Vec<CommitInfo> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            Some(CommitInfo {
                sha: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
                committed_at: parts.next()?.parse().ok()?,
            })
        })
        .collect()
}

/// Commits on `head` that aren't on `base`, newest first.
pub fn list_commits_between(
    repo: &Path,
    base: &str,
    head: &str,
    limit: usize,
) -> Result<Vec<CommitInfo>> {
    let base = resolve_commitish(repo, base)?;
    let head = resolve_commitish(repo, head)?;
    let range = format!("{base}..{head}");
    let out = git(
        Some(repo),
        &[
            "log",
            "--format=%H%x1f%s%x1f%ct",
            "-n",
            &limit.to_string(),
            &range,
        ],
    )?;
    Ok(parse_commit_lines(&out))
}

/// Latest commits on a branch, newest first.
pub fn list_commits(repo: &Path, branch: &str, limit: usize) -> Result<Vec<CommitInfo>> {
    let branch = resolve_commitish(repo, branch)?;
    let out = git(
        Some(repo),
        &[
            "log",
            "--format=%H%x1f%s%x1f%ct",
            "-n",
            &limit.to_string(),
            &branch,
        ],
    )?;
    Ok(parse_commit_lines(&out))
}

/// Uncommitted changes in the clone: tracked edits vs HEAD plus untracked
/// files rendered as new-file diffs. Returns (current branch, diff).
pub fn working_tree_diff(repo: &Path) -> Result<(Option<String>, DiffPayload)> {
    let branch = current_branch(repo);
    let mut bytes = git_bytes(repo, &["--no-pager", "diff", "HEAD"], &[1])?;
    let untracked = git(Some(repo), &["ls-files", "--others", "--exclude-standard"])?;
    for f in untracked.lines().filter(|l| !l.is_empty()) {
        if bytes.len() > MAX_DIFF_BYTES {
            break;
        }
        if let Ok(chunk) = git_bytes(
            repo,
            &["--no-pager", "diff", "--no-index", "--", "/dev/null", f],
            &[1],
        ) {
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok((branch, cap_diff(bytes)))
}

/// The checked-out branch name, or `None` when detached (`rev-parse
/// --abbrev-ref HEAD` prints the literal `HEAD` — e.g. a fresh worktree
/// before the agent checks out its branch) or when rev-parse fails outright
/// (unborn HEAD in an empty repo). Never errors — no branch is an answer.
pub fn current_branch(repo: &Path) -> Option<String> {
    git(Some(repo), &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|b| b != "HEAD" && !b.is_empty())
}

/// NUL-separated git output (`-z` flags) → lossy-decoded strings, empties
/// dropped.
fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Every path in the checkout that git would show as tracked or
/// untracked-but-not-ignored (`git ls-files --cached --others
/// --exclude-standard -z`). NUL-separated so non-ASCII paths aren't quoted;
/// gitignored trees (`target/`, `node_modules/`, `.git/`) drop out for free.
/// Repo-relative, unsorted.
pub fn list_worktree_files(repo: &Path) -> Result<Vec<String>> {
    let bytes = git_bytes(
        repo,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        &[],
    )?;
    let mut entries = split_nul(&bytes);
    // `--others` reports a nested git repo as `dir/` — a directory, nothing
    // servable as a file; drop those here so every client benefits.
    entries.retain(|e| !e.ends_with('/'));
    Ok(entries)
}

/// Refresh origin's refs in the hub clone (public wrapper for callers
/// outside this module that need current remote branch state).
pub fn fetch_origin(repo: &Path) -> Result<()> {
    git(Some(repo), &["fetch", "origin"]).map(|_| ())
}

/// The repo's remote branches (origin), `HEAD` and `orx/*` experiment
/// branches excluded — the pickable fork points for a project's baseline.
pub fn list_remote_branches(repo: &Path) -> Result<Vec<String>> {
    let out = git(
        Some(repo),
        &[
            "for-each-ref",
            "refs/remotes/origin",
            "--format=%(refname:short)",
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("origin/"))
        .filter(|b| *b != "HEAD" && !b.starts_with("orx/"))
        .map(str::to_string)
        .collect())
}

/// Resolve a branch name to its commit sha — the *local* ref first, then
/// origin's: the code browser wants the agent's latest work, which lives
/// locally before any push (the opposite preference of `branch_head_sha`,
/// which serves jobs that clone from the remote; `resolve_commitish` is the
/// diff-side sibling that also accepts raw shas). `Ok(None)` when neither
/// exists. Only real branch names are accepted: rev-suffix expressions
/// (`@{...}`, `^`, `~`, `:`, whitespace) are rejected up front, and the
/// leading-`-` check is belt-and-braces — the `refs/heads/` prefix already
/// keeps the name out of option position.
pub fn resolve_branch_commit(repo: &Path, name: &str) -> Result<Option<String>> {
    let suspicious = name.is_empty()
        || name.starts_with('-')
        || name.contains("@{")
        || name.contains(['^', '~', ':'])
        || name.chars().any(char::is_whitespace);
    if suspicious {
        return Ok(None);
    }
    for prefix in ["refs/heads/", "refs/remotes/origin/"] {
        let full = format!("{prefix}{name}");
        if let Ok(sha) = git(Some(repo), &["rev-parse", "--verify", "--quiet", &full]) {
            if !sha.is_empty() {
                return Ok(Some(sha));
            }
        }
    }
    Ok(None)
}

/// Every path in the tree of a commit (`git ls-tree -r -z --name-only`) —
/// the committed state, independent of any checkout. Repo-relative, unsorted.
pub fn list_tree_files(repo: &Path, sha: &str) -> Result<Vec<String>> {
    let bytes = git_bytes(
        repo,
        &["ls-tree", "-r", "-z", "--name-only", sha, "--"],
        &[],
    )?;
    Ok(split_nul(&bytes))
}

/// A file's committed content at `<sha>:<path>`, read from a streamed
/// `git cat-file blob` and capped at `limit` bytes — a multi-GB committed
/// blob costs one pipe buffer, not one allocation (unlike `file_at`, which
/// is fine for its known-small callers). Existence is checked first with
/// `cat-file -e` (exit code only, no error-message parsing): `Ok(None)`
/// when the path isn't in that tree. Returns `(content, truncated)`,
/// lossy-decoded, byte-exact up to the cap — no trimming.
pub fn file_at_capped(
    repo: &Path,
    sha: &str,
    path: &str,
    limit: u64,
) -> Result<Option<(String, bool)>> {
    use std::process::Stdio;
    let spec = format!("{sha}:{path}");
    let exists = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "-e", &spec])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    if !exists.success() {
        return Ok(None);
    }
    let mut child = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "blob", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let mut buf = Vec::new();
    let read = {
        use std::io::Read as _;
        let stdout = child.stdout.take().expect("stdout was piped");
        stdout.take(limit + 1).read_to_end(&mut buf)
    };
    let truncated = buf.len() as u64 > limit;
    // Reap the child before propagating any read error — no zombies. Kill
    // only when it may still be streaming (read error, or we stopped at the
    // cap): after a complete read EOF means git closed stdout and exits on
    // its own, and killing it then could race its natural exit into a bogus
    // signal-death status.
    if read.is_err() || truncated {
        let _ = child.kill();
    }
    let status = child.wait();
    read.map_err(|e| anyhow!("read failed: {}", e))?;
    if !truncated {
        // A cat-file failure after the `-e` probe (the path names a tree via
        // a crafted request, or the object vanished) must not masquerade as
        // an empty file. When truncated we killed it — any status goes.
        let status = status.map_err(|e| anyhow!("git cat-file blob: {}", e))?;
        if !status.success() {
            return Err(anyhow!("git cat-file blob {spec} failed"));
        }
    }
    buf.truncate(limit as usize);
    Ok(Some((
        String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_progress_parses_phase_and_percent() {
        let p = parse_clone_progress("Receiving objects:  47% (1234/2620), 5.2 MiB | 3.4 MiB/s")
            .unwrap();
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.percent, Some(47));

        let p = parse_clone_progress("Resolving deltas: 100% (120/120), done.").unwrap();
        assert_eq!(p.phase, "Resolving deltas");
        assert_eq!(p.percent, Some(100));

        // Phase line without a percent (count still running).
        let p = parse_clone_progress("Enumerating objects: 262, done.").unwrap();
        assert_eq!(p.phase, "Enumerating objects");
        assert_eq!(p.percent, None);
    }

    #[test]
    fn clone_progress_ignores_non_progress_records() {
        for rec in [
            "Cloning into '/tmp/x'...",
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Support for password authentication was removed.",
            "",
            "warning: You appear to have cloned an empty repository.",
        ] {
            assert!(parse_clone_progress(rec).is_none(), "parsed {rec:?}");
        }
    }
}
