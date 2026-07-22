---
name: orx-experiment-tree
description: "The experiment-tree model and the design loop: shape the prototype tree (stacked bushes), branch/build/playtest/promote, and `orx exp desc` notes. Use before creating, planning, or reorganizing experiments, when deciding which design idea to try next, when a round of variants has been judged, or whenever you're unsure how work maps onto the tree."
---

A game project is a **tree of prototype variants**. The root (**baseline**)
holds the reference build and a **run command** — the single shell command a
sim or job run executes. Every other node is a **child** branched off a
parent, inheriting its code and its run command, and every node is playable at
its branch tip (the `orx-play` skill). The two rules this depends on — **never
edit the baseline** and **the run command + env is a fixed contract** — are
the cardinal rules; everything below assumes them.

## Shape the tree — stacked bushes, not a flat fan or a noodle

The single most common way to drive a project badly is to get the **shape** wrong.
There are two opposite failures, and the right shape sits between them:

```
FLAT FAN (wrong)            NOODLE (wrong)            STACKED BUSHES (right)
root                        root                      root
├ a ├ b ├ c ... ├ n         └ a                       └ jump-head       ┐ round 1:
                              └ b                        ├ jump 0.35s   │ a small fan of
                                └ c                      └ jump 0.50s   ┘ co-equal options
                                  └ d ...                   └ keeper ── enemies-head  ┐ round 2
                                                               ├ sparse-fast          │ descends onto
                                                               └ dense-slow           ┘ round 1's keeper
```

- **Flat fan** (every idea hanging off the root): every variant is felt against
  the *start*, so wins never accumulate and the game never converges on a feel.
- **Noodle** (a long single-child chain): depth manufactured for its own sake —
  each step doesn't actually build on the one above it.
- **Stacked bushes** (correct): a *small fan within a round* (the options of one
  design decision), then **descend onto that round's keeper** for the next round.

**The one rule that produces this shape.** Before you make X a child of Y, name
what Y established that X builds on:

- **You can name it** ("Y is the jump-feel keeper; X keeps that jump and changes
  the enemy pacing") → real depth. X is a **child** of Y. Descend.
- **You can't — X and Y are co-equal options you're trying at the same time**
  (jump 0.35s vs 0.50s) → they don't build on each other. They're **siblings**
  in the same bush. Fan, don't chain.

So: **width = the open options of one design decision** (fan freely — a 3-way
movement-feel comparison *should* be three siblings under a common head);
**depth = decisions already settled, stacked** (one level down per keeper).
A new *round* never hangs off the root — it hangs off the previous round's
keeper. That keeps the tree moving **downward** as the design converges,
without stringing unrelated variants into a line.

Re-read the tree each round — `orx project view <projectId>` lists every node
(id, title, branch; roots marked `[root]`) — and check the shape: a wide row of
direct children off the root with no grandchildren means you're fanning when you
should be descending; a long depth-N chain with no branching means you're chaining
co-equal variants that should have been siblings.

## The design loop

To drive a project toward a design goal (e.g. "combat that feels punchy but
fair"), this is the intended flow — do **not** edit the baseline or rewrite the
run command:

1. **Read the baseline's code and play it.** You already sit in a private git
   worktree of the project's repo — `git fetch origin && git checkout <branch>`
   and read it with your normal tools (see the `orx-git` skill). See the node's
   run command with `orx exp status <expId>` and find where the design levers
   live (tuning constants, config files, entity definitions). If you haven't
   seen the baseline played, that's the first gap to close — ask the user, or
   run a sim.
2. **Form one round's worth of design options** — the co-equal options of a
   *single* decision (how floaty is the jump? how dense are enemies? which
   scoring rule?), each a concrete change you can make and judge against the
   others in this round. Don't mix decisions from different rounds into one
   batch — that's what produces the flat fan.
3. **Create the round as a bush, and pick its parent deliberately.** All of this
   round's options are **siblings under one parent** — the title is the idea, the
   description is the concrete change you'll make on that node's branch. The parent is:
   - the **baseline**, only for the very first round (nothing has been kept yet); or
   - the **previous round's keeper**, for every round after — so this round's
     changes build *on top of* what already feels right instead of resetting to
     the start. This is what walks the tree downward (see "Shape the tree" above).

   ```sh
   # Round 1 — one decision (jump feel), its options fanned off the baseline:
   orx create-experiment <projectId> --parent <baseId> --title "Jump 0.35s snappy" \
     --description "Set jumpDuration in tuning.ts to 0.35 with high gravity; change nothing else."
   orx create-experiment <projectId> --parent <baseId> --title "Jump 0.50s floaty" \
     --description "Set jumpDuration in tuning.ts to 0.50 with low gravity; change nothing else."

   # Round 2 — snappy jump kept → the next decision (enemy pacing) descends onto it:
   orx create-experiment <projectId> --parent <snappyKeeperId> --title "Dense slow enemies" \
     --description "On top of the snappy-jump keeper, double spawn density and halve enemy speed in waves.ts."
   ```
   The child inherits its parent's run command automatically — you don't set it,
   and you never give siblings different commands or env vars (cardinal rule 2).
4. **Implement each child's change on its git branch** — `orx create-experiment`
   prints the child's branch (`orx/<slug>`); in your worktree:
   ```sh
   git fetch origin && git checkout orx/<child-slug>
   #   …edit only the files that idea touches…
   git commit -am "snappy jump: 0.35s, high gravity" && git push
   ```
   **Leave the run command alone.** A local commit is enough for the Play
   button; the push is what compute runs clone. While you're in the code,
   **make the sim emit the evidence you'll need to judge it** — metrics to
   `$ORX_METRICS_PATH`, media to `$ORX_ARTIFACTS_DIR` (see the `orx-evidence`
   skill).
5. **Get each variant judged — both channels.** Launch sims for what's
   measurable: `orx exp run <childId> --kind sim --backend local` (remote
   backends and flags: the `orx-compute` skill; `--backend local` shares this
   machine, so run those one or two at a time). And hand the round to the user
   for feel: each variant is playable from its experiment card (the `orx-play`
   skill) — say which decision this round is probing so they know what to
   compare.
6. **Keep the round moving — drive a per-completion loop, not a wait-for-all
   barrier.** You want control back the moment *any one* run finishes so you can
   analyze it and either refill its slot or stop — not after the whole batch
   drains. `orx exp wait --project <projectId>` is built for exactly this: it
   returns on the **first** completion. Treat it as one **tick** of a loop, where
   *you* are the loop body:

   ```
   # after launching your runs, loop until the project is drained:
   loop:
     orx exp wait --project <projectId>   # sleeps; returns on the first completion
     orx runs <projectId>                 # SOURCE OF TRUTH: re-read all run states
     # for each run now terminal that you haven't handled yet:
     #   - read its results (step 7) and decide: launch a refill? promote it? stop?
     #   - launch the next queued child to refill the freed slot (step 5)
     # if `exp wait` printed "drained: no runs in flight"  → batch is done, break
   ```

   Three things make this robust — follow all of them:
   - **`exp wait --project` is a sleep-until-change signal, not the source of
     truth.** It only reports completions it observed *during that one call*. A
     run that finishes while you're analyzing the previous one is already terminal
     by the next call and **won't be reported**. So on every wake, re-read
     `orx runs <projectId>` and reconcile against the set of runs you've already
     handled — act on *every* newly-terminal run, not just the line `exp wait`
     printed. (This is the one time you do look at `orx runs` in a loop — as the
     reconcile after each wake, **not** as a tight poll in place of `exp wait`.)
   - **Re-issue `exp wait` each tick.** One completion → one return → you decide →
     you call it again. Don't expect a single `exp wait` to block until everything
     is done; that's the failure mode this loop avoids.
   - **Terminate on drained.** When no runs are in flight, `exp wait --project`
     returns immediately printing `drained: no runs in flight`. That — or seeing
     every run terminal in `orx runs` with no more children to launch — is your
     exit condition. Don't keep calling it into a timeout. Play sessions are
     outside this loop entirely: the user plays on their own time — end your
     turn and pick their feedback up next message.
7. **Analyze each result as it lands, then iterate.** For sims, **actually read
   the results**: `orx logs <runId>` plus the ingested metrics and their delta
   vs the parent (see the `orx-evidence` skill). For playtests, gather the
   user's reactions and record their verdicts (`orx exp verdict` — the
   `orx-play` skill). To see exactly what a node changed, diff its branch
   against its parent's (see the `orx-git` skill). Don't infer from status
   alone. Each completion is a decision point with three moves:
   - **Refill** — result is mediocre or inconclusive: launch or build the next
     queued variant to keep the round moving (step 5).
   - **Promote** — the round has a clear keeper (verdict `keep`): this node
     becomes the **parent for the next round**. The next batch of variants
     branch off *it*, not the baseline, so the win carries forward and the next
     ideas stack on top of it. This is the move that makes the tree grow deeper;
     skipping it is what produces a flat, sweep-only tree.
   - **Stop** — the design goal is met, or the line is exhausted (verdict
     `kill` across the round).

   The baseline stays untouched throughout — promotion moves the *focal parent* down the
   tree, it never edits the root.

**Combining variants.** When two branches both earned a `keep` and the next
step is one build with both changes, create a **merge node**:

```sh
orx create-experiment <projectId> --parent <keeperA> --merge <keeperB> \
  --title "A + B"
```

The merge commit lands on the new child's branch in one step and the node is
immediately playable; the tree draws a dashed merge edge from B. On
conflicts, the command says so — check out the new branch in your worktree,
`git merge <B's branch>`, resolve, push. Never merge into A's or B's own
branches (cardinal rule 6).

Stop when the goal is met, or when a round comes back all-kill with no new
ideas. When a line of work concludes, make sure every judged node carries its
verdict and its `orx exp desc` says how it felt and why it was kept or killed
— the tree itself is the design record.

## Experiment description / notes — `orx exp desc`

Each experiment node carries a free-form **description** (markdown) — the same
field set by `create-experiment --description`. Use it for notes: observations,
play-feel impressions, or a running summary. It is a whole-document field:
writing overwrites whatever was there.

```sh
orx exp desc <expId>                          # print the description to stdout (empty → hint on stderr)
orx exp desc <expId> --set "snappy but wall-jumps feel unfair; retry with coyote time"   # short note
cat notes.md | orx exp desc <expId> --stdin   # overwrite from stdin (long markdown)
```

- **Read** prints the text to **stdout** (pipe/redirect-friendly); when empty, a
  hint is printed to **stderr** and stdout stays empty.
- **Write** with exactly one of `--set` (inline) or `--stdin` (whole of stdin).
  Passing both is an error. Writing **replaces** the entire description — to
  append, read first, edit, and write back.
- `<expId>` comes from `orx create-experiment` output or `orx project view
  <projectId>` (the experiment id, not a run or project id).
