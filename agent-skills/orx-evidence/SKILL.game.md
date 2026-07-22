---
name: orx-evidence
description: "Judge a variant across every evidence channel: run logs (`orx logs`), ingested sim metrics ($ORX_METRICS_PATH), media artifacts ($ORX_ARTIFACTS_DIR), play sessions, and verdicts. Use after any run finishes, before declaring a variant better or worse, when metrics are missing from a sim, or when designing what a sim should print and save."
---

A game variant is judged through **four evidence channels** — don't declare a
variant better or worse on fewer than the question needs:

1. **Run logs** — the raw terminal stream of any run (sim, job, or Play build).
2. **Sim metrics** — a JSON doc the sim writes to `$ORX_METRICS_PATH`,
   ingested onto the run and shown with a **delta against the parent's
   metrics** on the run card.
3. **Media artifacts** — files the sim writes to `$ORX_ARTIFACTS_DIR`
   (screenshots, GIFs, replays, CSVs), the run's gallery.
4. **Play sessions + verdicts** — the human channel: time played lands as
   play-session runs, and keep/kill/iterate verdicts with notes record the
   judgment (recording them: the `orx-play` skill).

Numbers say whether it's *balanced*; the human channel says whether it's
*fun*. A variant that wins the sim but plays badly is a kill — when the
channels disagree, the verdict wins.

## Reading run logs — `orx logs`

A run's terminal output (the PTY stream) is captured live while it runs and
persisted afterwards.

```sh
orx logs <runId>                    # tail (the end — usually what you want)
orx logs <runId> --head             # read from the start instead
orx logs <runId> --bytes 200000     # raise the byte cap (default 64 KB, max 1 MB)
orx logs <runId> --range 4096:8192  # exact byte window [start, end)
```

- The log goes to **stdout** (pipe/redirect-friendly); a `[source] bytes a–b of N`
  status line goes to **stderr**, noting if content was truncated above/below.
- `<runId>` comes from `orx runs <projectId>` (the run id, not the experiment id).
- A **failed Play build** is diagnosed the same way — find its `play`-kind run
  in `orx runs` and read its log.

## Make the sim emit its own evidence

The run command is fixed (cardinal rule 2), so the *sim itself* — committed on
the branch — decides what evidence exists. When you touch sim code, leave it
emitting everything you'll need to judge the variant:

- **Metrics to `$ORX_METRICS_PATH`** — one JSON object of the numbers that
  answer this round's question (win rate, time-to-kill, deaths per level,
  episode lengths — arrays of per-episode numbers work). This is the
  structured channel; prefer it over grepping numbers out of logs.
- **Media to `$ORX_ARTIFACTS_DIR`** — a screenshot or short GIF per episode
  batch turns "the numbers moved" into "here's how it looked".
- **A compact end-of-run summary in the log** — the key numbers plus the
  tuning values the sim actually used, so a log alone identifies the variant.
- **Fixed seeds, committed on the branch** — a deterministic sim is what makes
  a parent/child metrics delta mean the *design* changed, not the dice. Sweep
  seeds *inside* one sim run (many episodes) rather than across runs.

## Comparing variants

- **Within a round**: compare siblings' ingested metrics on their run cards,
  and their galleries side by side. Same seeds + same episode count or the
  comparison is noise.
- **Against the parent**: the run card's metrics delta is the "did this round's
  change move the needle" readout.
- **The judgment itself** belongs on the node: verdict (keep/kill/iterate) for
  the decision, `orx exp desc` for the narrative — what the numbers said, how
  it felt, and why the verdict. A judged variant with an empty desc is
  evidence lost.
