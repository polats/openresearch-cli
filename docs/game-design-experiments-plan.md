# orx for game design experiments — plan

*Decided 2026-07-21. Research: tris-bot, gambit-arena-web, auto-battler, woid,
karate-wiener, tiny-army, plus prior art (modl.ai, Machinations, W&B/MLflow,
GameAnalytics, TITAN, Vercel previews / itch butler).*

## Why

Every piece of a game-design experiment loop already exists across our repos —
except the ledger. gambit-arena has deterministic seeded sims (`sim-batch.mts`),
~100 Playwright verify scripts, pixelmatch look-diffs, and PostHog telemetry;
karate-wiener has prompt ladders and A/B contact sheets; tris-bot's Battle Lab
computes win rates, time-to-kill, and dead-rule detection and hands results to
an agent for critique. But nothing persists: tris batch runs are ephemeral,
verify screenshots have no history or provenance, deploy history is 20 numbered
`deploy_out*.txt` files, and each repo rebuilds the same reporter→sessions-dir
glue. Nobody can answer "which variant won, and what exactly produced it?"
without archaeology.

orx already is that ledger: experiments as git branches with lineage, runs with
tracked status/logs/diffs, a live local dashboard, agent chat that can drive
the loop. The gap: orx assumes a run is a script that prints logs and exits,
while game runs produce playable builds, images/videos, and structured metrics,
and are judged by eyes and play-feel as much as numbers.

Prior-art lesson: small teams reject playtesting tools over friction, not
usefulness — zero-setup defaults win.

## Vision

orx becomes the studio's experiment ledger and play surface. A game project is
an orx project; every prototype variant is an experiment branch in the tree;
every evaluation of a variant is a typed run attached to that experiment, with
structured results and media. The tree view becomes a prototype gallery with
lineage: what we tried, how it branched from baseline, how each felt and
measured, and why we kept or killed it. The chat agent inherits tris-bot's
Build→Battle→Review→Learn loop, with every step persisted.

## Decisions

- **Run kinds:** `play` / `sim` / `verify` / `ladder`. Priority: play + sim
  first, then verify (look-diffs), then ladders.
- **Repo model:** prototypes are experiment branches in one game repo (orx's
  native model). No new grouping concept.
- **First actionable data:** human verdicts — keep / kill / iterate + notes —
  on runs and experiments. Telemetry (10-event starter schema, run-id-stamped)
  and agent QA reports (TITAN-style structured playthrough reports with
  coverage maps) are planned for, later.
- **tris-bot relationship:** orx absorbs tris ideas (Battle-Lab sim runs +
  agent critique, per-project design knowledge in chat context, distill-after-
  review as a skill) rather than staying a thin ledger. tris's core rule is
  kept: **never fork a renderer** — serve the game's own build, drive it via
  its existing postMessage bridge.

## Phases

**Phase 1 — play + verdicts + sims** (spec: `game-design-experiments-phase1-spec.md`)
- Play button on every experiment card: orx builds the branch and serves the
  output at a stable local URL.
- Verdict capture on runs and experiments.
- Sim runs: `metrics.json` ingestion, metric chips on run cards, delta vs the
  parent experiment.
- Media artifacts: a gallery pane on run cards, params-beside-artifact
  provenance.

**Phase 2 — visual verify**
- `verify` runs: screenshot/pixelmatch artifacts as first-class image pairs
  with pass/fail, and per-experiment image history ("what did this screen look
  like across the last N runs").

**Phase 3 — ladders + compare**
- `ladder` runs: variant × case media matrix with per-cell win/lose/tie
  judging (the Aya-ladder workflow, in the dashboard).
- Cross-experiment compare view; parallel-coordinates over params/metrics once
  there are enough runs to warrant it.

**Phase 4 — closing the loop**
- Telemetry ingest: `POST /api/runs/{id}/events` + a starter event schema
  auto-stamped with experiment/run id.
- Agent QA playthroughs producing structured, replayable reports.
- tris absorption completes: per-project `knowledge/` in chat context,
  distillation ritual as a chat skill, agent critique action on any sim run.

## Non-goals

- No new grouping concepts beyond project/experiment/run.
- No cloud or multiplayer telemetry infrastructure — loopback-first, like the
  rest of orx.
- No forked renderers or game-specific rendering in orx.
- No automated judging of aesthetic quality — humans give verdicts, sims give
  numbers.
