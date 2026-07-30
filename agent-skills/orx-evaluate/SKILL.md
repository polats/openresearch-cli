---
name: orx-evaluate
description: "Evaluate an idea against the FOUNDRY 35-signal market-fit rubric (keyless): code the signals in your own turn, commit the coding sidecar, run the evaluator sim (`orx exp run --kind sim`) for the alignment/comparables/confidence, then write the report + verdict rationale onto the node. Use when asked to analyze, score, or evaluate an idea node."
---

You evaluate one idea node. You do the reading-and-judging **yourself, in this
turn** — no API key, no separate model. The bundled evaluator does only the
deterministic math. This keeps every idea comparable while the judgment runs on
whatever harness you are.

**Where the evaluator lives.** It ships inside `orx`, not in the project repo —
`orx evaluator path` prints its directory (materializing it on first use), and
`$ORX_EVALUATOR_DIR` holds the same path inside your shell and inside sim runs.
Resolve it, don't hunt for it:

```sh
EVAL="$(orx evaluator path)"     # or: EVAL="$ORX_EVALUATOR_DIR"
```

Nothing about the evaluator is committed to the game repo. A blank idea repo has
everything it needs. If `node "$EVAL/evaluate.mjs" --print-signals` ever fails,
that's a bug worth reporting — not a cue to improvise a rubric from memory or to
go looking for one on disk.

## The loop

Work in your own worktree on the node's branch. Your task names the node
(`<expId>`); its thesis is `theses/<slug>.md` on that branch.

1. **Check out the node's branch.** `git fetch origin && git checkout <branch>`
   (the `orx/<slug>` branch — `orx exp status <expId>` shows it). See the
   `orx-git` skill for the flow.

   Expect this to fail with `already used by worktree` — the idea-foundry session
   that created the node usually still holds its branch when you're dispatched.
   That's routine, not a blocker: work detached and push to the branch ref, per
   **"When the branch is held by another session"** in `orx-git`.

   ```sh
   git fetch origin && git checkout --detach origin/orx/<slug>
   ```

2. **Get the rubric.** `node "$(orx evaluator path)/evaluate.mjs" --print-signals`
   prints the 35 signal definitions, the Present/Absent/Unresolved rules, and a
   JSON template with the exact 35 keys.

3. **Code the signals.** Read `theses/<slug>.md` and code each of the 35 signals
   Present / Absent / Unresolved, grounded in what the thesis actually says.
   Special rules: `free_to_play` → Present if pricing is unmentioned;
   `game_age_gte_2yr` → always Unresolved (new concept). Write the 35 keys to
   **`theses/<slug>.coding.json`**.

4. **Commit and push the sidecar.** `git add theses/<slug>.coding.json && git commit
   -m "Code signals for <slug>"`, then push to the branch ref —
   `git push origin HEAD:refs/heads/orx/<slug>` (this works whether or not you
   hold the branch). The sim clones from GitHub, so an unpushed sidecar won't be
   seen.

5. **Run the scoring sim.** An idea node starts with no run command. `orx exp
   cmd` does **not** exist in local mode — set the *project default* instead,
   which every node then inherits (check it first with `orx project view
   <projectId>`; if it's already this command, skip straight to the run):

   ```sh
   orx project edit <projectId> --run-command 'node "$ORX_EVALUATOR_DIR/evaluate.mjs"'
   ```

   Two things make that one command work for every idea in the project: the
   single quotes keep `$ORX_EVALUATOR_DIR` unexpanded so it resolves in the run's
   own shell (a different checkout than yours), and passing no `--slug` lets the
   evaluator take the one thesis on the branch. A branch carrying more than one
   thesis needs `--slug <slug>` — the evaluator says so plainly if it hits that.

   Then `orx exp run <expId> --kind sim --backend local` and `orx exp wait
   <expId> --timeout 120`. The sim reads your committed coding and produces the
   ingested metrics (`retention_align` / `mau_align` / `revenue_align` /
   `mean_align` / `confidence`) plus its own `evaluations/<slug>.md`. Read the
   result with `orx logs <runId>`.

   The sim needs no API key: it does arithmetic on your coding — per-KPI
   alignment fractions, anti-pattern and table-stakes counts, top comparables,
   and the resolved-signal share. It never scores, gates, or judges.

6. **Write the qualitative read — in all three places** (the same discipline
   idea-foundry uses for the thesis, so the report is never trapped on one node).
   From the computed alignment + comparables, author the archetype match, a short
   strengths/weaknesses, and a one-line executive summary (plain language — no
   signal jargon). Save that one report to:
   1. **Node notes** — `orx exp desc <expId> --stdin < read.md` (what the card shows).
   2. **The branch** — write it to `theses/<slug>.evaluation.md`, then commit and
      push it the same way as the sidecar (`git push origin
      HEAD:refs/heads/orx/<slug>`), versioned right next to the thesis. Use the
      `theses/` dir — some projects `.gitignore` an `evaluations/` dir, which
      silently drops the commit; `theses/` is always tracked.
   3. **The Files dir** — copy it to `{files}/<slug>-evaluation.md` so it appears
      in the dashboard's **Files tab** next to the thesis. Use the `-evaluation`
      suffix so it never overwrites the thesis's `<slug>.md`.

   This is your judgment on top of the math. Keep the three copies identical.

7. **Stop at the verdict.** Keep / kill / iterate is the **human's** call. Present
   the evaluation and let them decide; if they state a judgment, record it with
   `orx exp verdict <expId> <keep|kill|iterate> -m "…"`.

## Notes
- Comparability across many ideas comes from the orchestrator dispatching the
  coding to one designated harness/model — not from anything you set here.
- Never invent design content the thesis doesn't support; "Unresolved" is a
  valid, common code. Silence is not a signal.
- If `theses/<slug>.coding.json` already exists (a prior run), you may re-use or
  revise it rather than re-coding from scratch.
