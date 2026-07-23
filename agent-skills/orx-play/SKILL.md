---
name: orx-play
description: "Make an experiment branch playable and gather play evidence: the dashboard Play button, play command and play dir, /play/<expId>/ URLs, play-session runs, sim runs (--kind sim) with ingested metrics, and verdicts (orx exp verdict). Use before asking the user to playtest, when a Play build fails, when setting up a new game's build, or when recording keep/kill/iterate decisions."
---

Every experiment branch is a **playable variant**. This module covers the two
evidence channels of game-design work — human play sessions and headless sims
— and the verdicts that turn them into decisions.

## The Play surface — how a branch becomes playable

Each experiment card in the dashboard has a **Play** button. Pressing it:

1. checks out the experiment branch's **local tip** into a dedicated detached
   worktree (no push needed — this serves local work);
2. runs `npm install`/`npm ci` if `package.json` exists and `node_modules`
   doesn't;
3. runs the project's **play command** (default **`npm run build`**);
4. serves the **play dir** (default **`dist`**, repo-relative) at the stable
   local URL **`/play/<expId>/`**.

The build is tracked as a `kind: play` run on the experiment — its log is
readable with `orx logs <runId>` like any other run, which is where you look
when a Play build fails. A rebuild only happens when the branch tip moved;
otherwise Play serves the existing build. **Creating an experiment kicks off
a play build automatically** (of the fork-point code), so every new variant
card starts playable; your committed changes are picked up on the next Play
press.

### Builds must be subpath-portable — the #1 playability failure

The playable is served under **`/play/<expId>/`**, not at the site root.
Bundlers default to **root-absolute** asset paths (`/assets/app.js`), which
404 there and render a blank page even though the build "succeeded". The
play run ends with a **playability check** that greps the built output for
root-absolute `src=`/`href=`/`url(` references and **fails the build with
the offending lines** when it finds any. Fix it at the source, once, on the
baseline:

- **Vite**: `base: './'` in `vite.config.{ts,js}`.
- **CRA**: `"homepage": "."` in `package.json`.
- **Plain HTML/CSS/JS**: relative paths only (`./assets/x.png`, never
  `/assets/x.png`); CSS `url(...)` resolves from the stylesheet's location.
- **Runtime loads** (fetch, dynamic imports, sprite sheets built from JS):
  resolve from the document, e.g. `new URL('sprites.png', import.meta.url)`
  or relative fetch paths — the check can't see these, so they're on you.

Get this right in the baseline and every branched variant inherits it.

Play settings live **on the project** (play command + play dir, one contract
for every variant — same spirit as the fixed run command) and are edited in
the dashboard, not the CLI. If the defaults are wrong for this repo (no npm,
different bundler, different output dir), tell the user what to set them to
rather than working around it per-branch.

An experiment can override its **entry page** (`play_entry`, e.g.
`arena.html?mode=1`) when the build's `index.html` isn't the thing to play.

### Your job before handing over a variant

- **Commit to the experiment branch** — Play builds the branch tip, so
  uncommitted edits in your worktree are invisible to it. A local commit is
  enough for Play; `git push` is still required before any `orx exp run`
  compute launch.
- **Keep the variant buildable by the play command.** A variant that doesn't
  build can't be judged. If you changed entry points or output layout, verify
  the play command still produces the play dir.
- Then point the user at the experiment card: playtesting is their move. Time
  they spend playing lands automatically as a `play-session` run on the node
  (with telemetry events when the game emits them) — you'll see it in
  `orx runs`.

## Sim runs — headless evidence

For anything measurable without hands on the controls (win rates, balance
sweeps, time-to-kill, soak tests), launch a **sim run**:

```
orx exp run <expId> --kind sim --backend local
```

Sims use the node's fixed run command like any run, plus two extra channels
the local backend exports:

- **`$ORX_METRICS_PATH`** — write one JSON object here (numbers, or arrays of
  per-episode numbers); it is **ingested onto the run** when it finishes and
  shows on the run card with a delta against the parent's metrics. This is
  the structured-results channel — prefer it over grepping logs.
- **`$ORX_ARTIFACTS_DIR`** — any files written here (screenshots, GIFs,
  replays, CSVs) become the run's media gallery.

Make the sim deterministic where you can (fixed seeds committed on the
branch) so parent/child deltas mean something. `--kind sim` requires
`--backend local`.

## Verdicts — the human judgment ledger

Verdicts are **keep / kill / iterate** + notes, and they are the *user's*
call. Runs and experiments each carry one: a run verdict records "this
session/batch felt X"; the **experiment verdict is the standing status** of
the variant, set explicitly (never derived).

```
orx exp verdict <expId> keep|kill|iterate|clear [-m "why"]
```

- When the user states a judgment in chat ("too floaty — kill it"), record it
  with their reasoning in `-m`. Don't invent verdicts from metrics alone.
- Treat standing verdicts as decisions already made: don't reopen a `kill`
  without new information, and descend from `keep` nodes when growing the
  tree.
- Verdict notes are terse; the fuller "how it felt and why" narrative belongs
  in `orx exp desc` alongside the numbers.
