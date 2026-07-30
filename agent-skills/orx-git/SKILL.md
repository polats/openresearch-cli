---
name: orx-git
description: "Read, edit, and diff a node's code with plain git: sync, commit, and push before running. Use whenever you touch experiment code — before editing any branch, when a checkout or push fails, when comparing two nodes' code, or when a run seems to have picked up stale code."
---

Every experiment node **is a git branch** (`orx/<slug>`) on the project's GitHub
repo — `orx create-experiment` prints it. You interface with code through plain
git and your own tools.

## In a local `orx up` session (the usual case): work in your cwd, and ONLY your cwd

You are **already inside a private git worktree** of the project repo — your
current directory. Do everything here: `git fetch origin && git checkout <branch>`,
edit, commit, push. This worktree is yours alone.

**NEVER `cd` into or write to `~/.cache/openresearch/repos/<owner>/<repo>`** — that
is the shared **hub clone** that *every* session's worktree is derived from.
Checking out a branch or dropping files there corrupts other agents' checkouts and
can leave a branch locked to the hub. Stay in `$PWD`, and if a checkout fails with
"already used by worktree", use the detached flow below — don't go hunting in the
cache dir.

### When the branch is held by another session

Git allows one worktree per branch, so `git checkout <branch>` fails with
`already used by worktree …` whenever another live session holds it. This is
normal on a hand-off: the idea-foundry session that *created* a node still owns
its branch when the analyst is dispatched to evaluate it.

You do **not** need the branch checked out to work on it, and you must not break
the other session's lock. Work detached at the remote tip and push back to the
branch ref:

```sh
git fetch origin
git checkout --detach origin/<branch>      # read the thesis, edit, commit here
git add <files> && git commit -m "…"
git push origin HEAD:refs/heads/<branch>   # lands on the branch; lock untouched
```

The remote is what matters — sim runs clone the branch from GitHub, so a pushed
commit is visible to everything downstream even though the other worktree's local
ref hasn't caught up.

If that push is rejected as non-fast-forward, the branch holder committed while
you worked. Re-anchor and retry — never force-push:

```sh
git fetch origin && git rebase origin/<branch> && git push origin HEAD:refs/heads/<branch>
```

Only give up on the node if the rebase genuinely conflicts; then say what
conflicted and let the human sequence the two sessions.

## Outside a live session (cloud / full-set contexts only)

There, clone into the openresearch cache dir (keyed by repo, reused across a
project's experiments), **never** your cwd or `~/projects`:

```
~/.cache/openresearch/repos/<owner>/<repo>
```

`<owner>/<repo>` comes from `orx projects`.

This is how you **realize a child's hypothesis**: after `create-experiment
--parent`, check out the child's branch and make the specific code/config edits
its description calls for — then commit, push, and run. Edit only the files that
idea touches, and **don't touch the run command** (it's inherited; see the
`orx-experiment-tree` skill). A node a run has already answered is frozen —
branch a child instead.

The sync recipe is **idempotent** — run it verbatim whether or not the clone
already exists from a previous session. Always fetch + fast-forward before
editing, so a reused clone is never stale (and the experiment's branch, created
server-side, is fetched even when it's brand-new and not in your local clone
yet):

```sh
DIR=~/.cache/openresearch/repos/<owner>/<repo>

# Clone once (skips if it already exists), then ALWAYS sync before touching a branch:
[ -d "$DIR" ] || git clone https://github.com/<owner>/<repo> "$DIR"
git -C "$DIR" fetch origin
git -C "$DIR" checkout orx/<slug>                 # creates a tracking branch if it's remote-only
git -C "$DIR" merge --ff-only origin/orx/<slug>   # fails loudly rather than discarding unpushed work

#   …edit files under "$DIR" with your normal tools…
git -C "$DIR" commit -am "tune lr"     # one or more commits — your call
git -C "$DIR" push                     # push so runs and the tree see the change
```

Rules and notes:
- **Always sync first — but never blow away unpushed work.** `merge --ff-only`
  fails loudly instead of silently discarding commits you never pushed — a real
  hazard when a push has failed and you're returning to a branch to repair it.
  Never use `checkout -B <branch> origin/<branch>`: it hard-resets to the GitHub
  tip and throws that work away. The contract is still commit + push before
  moving on.
- **Auth is your own git.** Clone/push use whatever GitHub credentials your `git`
  already has — the repo lives under your account or your org, so access is the
  same as any of your repos. If a clone or push fails on auth, authenticate git
  for github.com (e.g. `gh auth login` or an SSH key) and retry.
- **Push before you run.** `orx exp run` launches from the branch's pushed tip on
  GitHub — uncommitted or unpushed edits won't be in the run. Commit and push
  first.
- **Never merge or rebase a branch once its node is frozen** (cardinal rule):
  its history is the code those results came from. Bring changes in on a
  **child** instead. On a *provisional* node a plain `git merge
  origin/<parent-branch>` is fine. Never rewrite history anywhere — no rebase,
  amend, `reset --hard`, or force-push.
- **Reading another node's code** without disturbing your checkout: that branch is
  already in the clone after a fetch — `git -C "$DIR" show origin/orx/<slug>:<path>`.

## Code diffs — local git

What did a run change vs. its parent experiment? `orx exp status <expId>` prints
the parent's branch, the latest run's full commit SHA, and this exact recipe —
compute the diff locally in the same clone:

```sh
DIR=~/.cache/openresearch/repos/<owner>/<repo>   # owner/repo from `orx projects`
[ -d "$DIR" ] || git clone https://github.com/<owner>/<repo> "$DIR"   # cold cache → clone first
git -C "$DIR" fetch origin                        # ALWAYS fetch first — the commit and parent tip live on GitHub
git -C "$DIR" diff origin/<parent-branch>...<full-commit-sha>
```

- The **three-dot** form diffs from the merge-base — what the run's branch
  changed, not what the parent gained since the fork. That's the cumulative
  "what this experiment did to the code" view.
- Fetch first is mandatory: the run's commit and the parent's tip exist on
  GitHub and may not be in your clone yet.
- Root experiments have no parent — there is no diff base, by definition.

## Repairing a node in place (`orx up` worktrees)

A node whose run answered nothing is provisional: fix it on **its own branch** and
re-run the same `<expId>` — don't create a child (`orx-experiment-tree`). Sync
as above, then commit, push, `orx exp run`.

If the checkout fails with "already checked out at …", read the path: your own
worktree means you already hold it. Another session's means that agent owns the
branch — never break the lock, and never branch a child to dodge it. Use the
detached flow from "When the branch is held by another session" above: commit at
`origin/<branch>` and push to `HEAD:refs/heads/<branch>`.
