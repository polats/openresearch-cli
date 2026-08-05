# How the analyst agent evaluates an idea (keyless)

This is the normal flow — **no API key**. You (the analyst agent, on whatever
harness this session uses) do the reading-and-judging; the sim does the math.

1. **Get the rubric.** Run `node tools/idea-evaluator/evaluate.mjs --print-signals`
   — it prints the 35 signal definitions, the Present/Absent/Unresolved coding
   rules, and a JSON template with the exact 35 keys.

2. **Code the signals.** Read `theses/<slug>.md` and code each of the 35 signals
   Present / Absent / Unresolved, grounded in what the thesis actually says.
   Special rules: `free_to_play` → Present if pricing is unmentioned;
   `game_age_gte_2yr` → always Unresolved (new concept). Write the result to
   **`theses/<slug>.coding.json`** (the 35 keys from the template).

3. **Score it.** Run `bash run.sh` (or `node tools/idea-evaluator/evaluate.mjs
   --slug <slug>`). The sim reads your coding and writes the deterministic
   metrics (`$ORX_METRICS_PATH`) + `evaluations/<slug>.md` (alignment,
   nearest comparables, confidence). None of this calls an LLM.

4. **Add the qualitative read.** In your turn, write the archetype match and a
   short strengths/weaknesses + one-line executive summary, grounded in the
   computed alignment + comparables, into the node notes (`orx exp desc`) or by
   appending to `evaluations/<slug>.md`.

Comparability across a gallery comes from dispatching the **coding** step to one
designated harness/model (the orchestrator's choice), not from any key or
hardcoded provider here.

(Headless batch, no agent in the loop: `EVAL_LLM=1 bash run.sh` lets the sim do
steps 2 + 4 itself via a pinned model — the only path that needs a key.)
