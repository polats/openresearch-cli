# Vendored: the idea evaluator

This tree is **vendored, not authored here.** Source of truth:

- `polats/foundry-idea-demo:tools/idea-evaluator/` — the portable packaging
- which is itself generated from `ai-asylum/foundry`:
  `.claude/skills/game-signal-analysis/` (signal definitions + game profiles)
  and `.claude/skills/archetype-alignment/` (archetype reference), bundled by
  `game-foundry/scripts/gen-eval-data.mjs`

`src/local/evaluator.rs` embeds these files with `include_str!` and materializes
them under `<data dir>/idea-evaluator/` on demand. `orx evaluator path` prints
that directory; `$ORX_EVALUATOR_DIR` carries it into harness children and sim
runs.

## Why it's in the binary rather than in each project repo

The `orx-evaluate` skill used to point the analyst at `tools/idea-evaluator/`
inside the project repo. Every repo the idea pipeline creates is blank, so the
analyst's second step died on `Cannot find module` and it had no rubric to code
against — it then searched the filesystem and gave up. Shipping the evaluator in
the binary means one source of truth that cannot drift per project, resolves
offline, and keeps ~330KB of generated reference data out of every game repo.

## Updating

Re-copy from `foundry-idea-demo` and re-run the self-test — it recomputes the
alignment fractions independently and checks the comparables reference real
profiles:

```sh
node test-core.mjs      # expects: 6 checks passed
```

Then rebuild so `include_str!` picks up the new bytes. `src/local/evaluator.rs`
stamps the materialized copy with the crate version, so an upgraded binary
refreshes a user's cached tree automatically.

## What it does and does not do

Evaluation v2 arithmetic only: per-KPI alignment fractions (discriminating
present ÷ resolved), anti-pattern and table-stakes counts, top-5 comparables
with their measured KPI values, and the resolved-signal confidence share. No
invented weights, no thresholds, no pass/fail, no verdicts — the analyst codes
the signals, and a human decides keep/kill/iterate.
