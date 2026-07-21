# Phase 1 spec — playable builds, verdicts, sim metrics, artifacts

*Companion to `game-design-experiments-plan.md`. Design only — no code yet.*

Phase 1 delivers four capabilities, shippable as three milestones:

- **M1 Play**: a Play button on every experiment card; orx builds the branch
  and serves it at a stable local URL.
- **M2 Verdicts**: keep/kill/iterate + notes on runs and experiments.
- **M3 Sims + artifacts**: `sim` runs with ingested metrics, delta vs parent,
  and a media gallery on run cards.

## 1. Data model (all additive, via the existing best-effort migration list)

```sql
ALTER TABLE runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'job';
  -- 'job' (today's runs) | 'play' | 'sim' | later: 'verify' | 'ladder'
ALTER TABLE runs ADD COLUMN metrics_json TEXT;      -- ingested metrics doc
ALTER TABLE runs ADD COLUMN verdict TEXT;           -- keep | kill | iterate
ALTER TABLE runs ADD COLUMN verdict_notes TEXT;
ALTER TABLE runs ADD COLUMN verdict_at INTEGER;

ALTER TABLE local_experiments ADD COLUMN verdict TEXT;        -- standing status
ALTER TABLE local_experiments ADD COLUMN verdict_notes TEXT;
ALTER TABLE local_experiments ADD COLUMN verdict_at INTEGER;

ALTER TABLE local_projects ADD COLUMN play_command TEXT;  -- default: npm run build
ALTER TABLE local_projects ADD COLUMN play_dir TEXT;      -- default: dist
```

Verdict placement (decided): **both levels**. A run verdict records "this
session/batch felt X"; the experiment verdict is the standing keep/kill/iterate
status, set explicitly (never silently derived — the UI offers "promote to
experiment verdict" when saving a run verdict).

`ApiRun` gains `kind`, `metrics` (parsed object or null), `verdict`,
`verdictNotes`, `verdictAt`. Experiment payloads gain the verdict trio.
The SSE diff already keys runs on `(status, updated_at)`; verdict and metrics
writes must bump `updated_at` so cards refresh (unlike watcher heartbeats).

## 2. M1 — Play builds

**Build location.** A dedicated worktree of the experiment's branch under
`~/.cache/openresearch/play-builds/<owner>/<repo>/<experiment-id>/` (sibling of
the existing `worktrees/` root; same collision reasoning). Reusing the chat
session worktree is wrong — builds must not race the agent's edits.

**Build flow.** `POST /api/experiments/{id}/play-build`:
1. Create/refresh the worktree at the branch head (record the commit SHA).
2. Run the project's `play_command` (default `npm run build`; `npm ci` first
   only when `node_modules` is absent) with the run-dir conventions of the
   local backend — the build IS a run (`kind: 'play'`, status
   starting→running→done/failed), so logs stream to the existing log tail and
   failures are visible in the normal way. Supervised by `orx supervise` like
   any local run.
3. On success, record the servable dir (`<worktree>/<play_dir>`) in the run's
   `backend_json` (`{"kind":"play_build","dir":...,"sha":...}`).

**Serving.** `GET /play/{experiment_id}/{*path}` — static file serve from the
most recent successful play build's dir, `index.html` fallback for SPA routes,
`no-cache` on HTML. Stable URL per experiment; rebuilds swap content in place.
Path traversal guarded the same way as `project_file` (canonicalize + prefix
check). Multi-entry builds (gambit-arena's `game.html`) work because the whole
dist dir is served; the Play button can carry an optional
`play_entry` (project-level, default `index.html`) if the root page is wrong —
defer adding this column until gambit-arena is actually onboarded.

**Devvit caveat (accepted):** gambit-arena inside a real Reddit webview needs
`devvit playtest`; the static build still covers the local game.html surface,
which is what the existing verify scripts drive. Proxy-to-dev-server mode is
out of scope for Phase 1.

**UI.** TreeView card gets a **Play** action (opens `/play/<id>` in a new tab;
if no successful build exists — or the branch head moved past the last build's
SHA — it triggers a build first and shows the building state on the card).
DetailDrawer terminal view works unchanged for build logs.

## 3. M2 — Verdicts

**API.**
- `POST /api/runs/{id}/verdict` `{verdict: "keep"|"kill"|"iterate", notes?}` —
  also accepts `{verdict: null}` to clear.
- `POST /api/experiments/{id}/verdict` — same shape.

**UI.**
- Run rows/cards: three-chip selector (Keep / Kill / Iterate) + a notes
  popover. Lives in the DetailDrawer terminal header and on sim run cards.
- Experiment card (TreeView): standing verdict badge (green keep / red kill /
  amber iterate) next to the status badge; set from a small menu on the card
  or promoted from a run verdict.
- CLI: `orx exp verdict <exp-id> keep|kill|iterate [-m notes]` for
  agent/scripted use — verdicts must be writable by the chat agent.

## 4. M3 — Sim runs, metrics, artifacts

**Launching.** A sim run is a normal local-backend run with `kind: 'sim'`:
`POST /api/experiments/{id}/run` gains an optional `kind` field, and the CLI
`orx exp run --kind sim`. The command comes from the request or the
experiment's `run_command` (e.g. `node scripts/sim-batch.mts < specs/base.json
> "$ORX_METRICS_PATH"`). Sim specs live committed in the repo so every sim run
is reproducible from its commit SHA.

**Metrics ingestion — the contract.** Two env vars are injected into every
local run (all kinds):

- `ORX_METRICS_PATH` — a file path; if the command leaves a JSON document
  there, the supervisor ingests it into `runs.metrics_json` on terminal
  status.
- `ORX_ARTIFACTS_DIR` — a pre-created directory; any files the command writes
  there become the run's media artifacts.

Zero-setup fallback: if `ORX_METRICS_PATH` was not written and the run's log
is a single parseable JSON document (sim-batch's stdout contract), ingest
that. Size cap ~2 MB; oversize metrics are truncated to their `aggregate` key
with a flag.

**Metrics shape.** Free-form JSON object. The dashboard treats one key
specially: a top-level `aggregate` object of numeric leaves is surfaced as
metric chips. This matches `sim-batch.mts` today
(`aggregate: {heroWinRate, meanTtk, ttkStdDev}`) with no changes on the game
side. Everything else renders as a collapsible JSON tree.

**Delta vs parent.** `GET /api/experiments/{id}/metrics-delta` — compares the
latest done `sim` run's `aggregate` against the parent experiment's latest,
returning per-key `{value, parentValue, delta}`. UI: delta chips
(`heroWinRate 0.54 ▲+0.07`) on the experiment card eyebrow and the sim run
card. Baselines (no parent) show plain values.

**Artifacts.** Stored under `<data_dir>/run-artifacts/<run_id>/` (moved by the
existing data-dir move machinery; run-id sanitized like `log_path`). Local runs
get `ORX_ARTIFACTS_DIR` pointed there directly — no copy step. API:
`GET /api/runs/{id}/artifacts` (recursive listing: path, size, content type)
and `GET /api/runs/{id}/artifacts/file?path=` (bytes, same guards as
`serve_file`). UI: a gallery pane on the run view — images/videos inline in a
grid, other files as rows; if `<name>.json` sits beside `<name>.png`, the JSON
renders as the image's caption (the params-beside-artifact convention from
tiny-army, made first-class).

## 5. Out of scope for Phase 1

Verify/ladder run kinds, telemetry ingest endpoint, agent QA playthroughs,
A/B pair verdicts, parallel-coordinates compare, per-project knowledge dirs,
Devvit dev-server proxying, remote-backend artifact upload (artifacts are
local-backend-only in Phase 1; remote backends keep logs-only).

## 6. Build order & risk notes

1. Schema + `ApiRun`/store plumbing (small, unblocks everything).
2. M2 verdicts (smallest end-to-end slice, immediately useful on existing runs).
3. M1 play builds (worktree + static serve; the only new infrastructure).
4. M3 env-var contract in the local backend + supervisor ingestion, then the
   gallery and delta endpoints.

Risks: `npm ci` cost on first build per experiment (mitigate: shared node
package cache is out of scope, just surface build time honestly in logs);
serving untrusted build output on the LAN when `--host` is widened (the play
surface inherits the dashboard's existing no-auth caveat — document it);
metrics ingestion racing log tail on terminal status (ingest in the same
terminal-status path the supervisor already serializes).
