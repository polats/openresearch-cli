# Idea → Prototype pipeline: multi-agent orchestration plan

Status: **design locked, not yet built**. Author-facing implementation plan.

This brings FOUNDRY's game-discovery flow into `orx` as a set of personas plus a
first-class, cross-provider, human-gated subagent-dispatch layer. The
`idea-foundry` persona (idea capture) already ships; everything below is the
plan for the rest.

## Goal

A **Producer** agent runs a discovery funnel over an idea gallery: capture ideas
→ analyze each → greenlight → build a playable prototype. The Producer *suggests*
each next step and *which provider* should do it; a human approves (and can
override provider/persona/model) on every dispatch. Subagents are first-class
orx sessions and may run on **any provider** (Claude / GPT / Kimi …), regardless
of the parent's provider.

## Locked decisions

1. **Analysis** = a spawned **analyst subagent** (human-suggested, any provider)
   that **codes the 35 signals in its own turn** (keyless — using whatever
   harness the session runs on) and hands a coding sidecar to a **keyless
   deterministic sim** (arithmetic, comparables, confidence). The agent also
   authors the qualitative read (archetype, prose). No API key and no hardcoded
   provider on the normal path. Comparability across ~40 ideas comes from
   dispatching the *coding* to one designated harness/model (pin the **analyst**,
   not an API inside the sim). A self-contained `--llm` headless path (pinned
   model, needs a key) exists only for batch runs with no agent in the loop.
2. **Prototypes** = **graduate each greenlit idea into its own repo/project**
   (persona `game-designer`), seeded with `theses/<slug>.md` as the founding GDD.
   The gallery idea node keeps a "→ linked project" pointer; the playable lives
   in the graduated project's own experiments view. Matches ai-asylum's
   one-repo-per-game reality, FOUNDRY's graduation handoff, and orx's tree
   semantics (a project's tree = variants of *one* codebase).
3. **Orchestrator** = a distinct **Producer** persona. `idea-foundry`, `analyst`,
   and `game-designer` are the three worker personas it dispatches.
4. **Dispatch UX**: the orchestrator **suggests**, the human **picks**. Spawning
   is a proposal card (pre-filled + editable provider/persona/model/task), not a
   direct action. Reuses orx's existing approval-card channel.
5. **Cross-provider** is the reason subagents must be first-class orx sessions:
   Claude's native Task tool is same-provider + invisible to orx, so it can't be
   the dispatch mechanism.

## Roles

```
Producer (control tower, over the idea-gallery project)
  ├─ suggest idea-foundry subagent → captures an idea → idea node
  ├─ suggest analyst subagent → runs pinned evaluator sim → node scored + report → human verdict
  └─ on greenlight: suggest "graduate + game-designer"
        → new repo+project seeded with the thesis, persona=game-designer
        → game-designer subagent builds the playable in THAT project's tree
     gallery node shows "→ linked: <slug>"; playable lives in its own experiments view
```

Every arrow is a human-approved, any-provider dispatch card.

---

## Core primitives to build

### P1 — Per-session `{harness, model, persona}`

Today persona is per **project** (`local_projects.persona`); harness/model are
per session but persona is not. Make persona per session so a game-designer
subagent can run alongside an idea-foundry session on the same project, and so
each dispatched worker gets the right playbook + skills.

- Add `persona TEXT` to `StoredChatSession` (`src/store.rs` — mirror the
  `local_projects.persona` migration at `store.rs:335`; extend the chat-session
  column list / `SELECT`).
- Add `persona: Option<String>` to `StoredChatSession` (`store.rs:1038-1053`) and
  `CreateChatSessionReq` (`src/commands/up.rs:3239-3245`).
- Thread it into `TurnCtx` (`src/local/chat/mod.rs:908-929`).
- In `ensure_playbook` / `playbook_md` make the **session persona win over
  `project.persona()`** (`src/local/opencode.rs:230`, `:302`); fall back to the
  project persona when the session has none (back-compat).
- `skills_for_persona` unchanged (`src/local/agent_skills.rs:266`).

### P2 — Session ↔ node link

A dispatched worker needs to know which node it works on, and its output must
attach there.

- Add `experiment_id TEXT NULL` and `parent_session_id TEXT NULL` to
  `StoredChatSession`.
- For a worker bound to a node, `ensure_session_worktree` should check out the
  node's branch instead of detaching on baseline (`src/local/git.rs:361-388`) —
  or the worker creates a child node itself and checks that out. Decide per
  persona (analyst reads the node; game-designer creates a child).

### P3 — Dispatch primitive (suggest → approve → spawn)

Two verbs, both landing on a first-class sub-session:

- `orx agent suggest --persona <p> --suggest-harness <h> --suggest-model <m>
  --parent <expId> --task "<text>" --why "<text>"` — orchestrator-callable;
  emits a **proposal** (does NOT spawn).
- `orx agent spawn …` — the confirmed action orx runs after human approval
  (also usable directly for a future autonomous-mode toggle).
- `orx agent list | status <id> | wait <id>` — supervise, mirroring
  `orx exp run|wait` ergonomics agents already know.

New top-level `Agent` subcommand in the `Command` enum (`src/main.rs:56-170`).
Backing calls go through an **authenticated internal API** (mirror the
token-gated `/api/internal/permissions` at `up.rs:330-333`), NOT the
unauthenticated loopback surface. Hand the harness the token via env at spawn
(same channel as the mcp bridge token, `src/local/harness/claude.rs:718`).

`suggest` creates a pending proposal; the UI renders it as a card; on approval
the server calls the existing `create_chat_session` path + sends the task as the
first message (`chat/mod.rs:773`).

### P4 — The evaluator sim (analysis engine)

Port FOUNDRY's Evaluation v2 into a committed, runnable evaluator, launched per
idea via `orx exp run <ideaNode> --kind sim --backend local`.

- Port `game-foundry/src/lib/eval-core.mjs` (35 signals, `CLS` discrimination
  table, `computeEvaluationCore` arithmetic, archetype/summary/market prompts)
  and `game-foundry/src/lib/eval-data.mjs` (signal defs, archetype reference,
  game-signal profile library) into the project template.
- Split (keyless normal path):
  - **Agent-coded (keyless)**: the analyst codes the 35 signals in its turn
    (`--print-signals` gives the rubric) and writes `theses/<slug>.coding.json`;
    it also authors the archetype + prose. Uses whatever harness the session
    runs on — no key, no hardcoded provider.
  - **Deterministic sim (keyless)**: reads the coding, computes KPI alignment,
    comparables, confidence. Pure.
  - **`--llm` headless (opt-in, keyed)**: the sim codes signals + writes prose
    itself via a pinned model — batch runs with no agent in the loop only.
- Comparability = pin the **analyst** (dispatch coding to one designated
  harness/model), not an API key inside the sim.
- Outputs: a metrics JSON (ingested as node metrics via `$ORX_METRICS_PATH` —
  `retention_align/mau_align/revenue_align/mean_align/confidence`) + a committed
  `evaluations/<slug>.md` report (+ a copy in the Files dir).
- Re-run on `theses/<slug>.md` commit change (store the evaluated SHA — the
  `evaluated_at_commit` analog).
- **Status: SHIPPED (Phase 0)** — `polats/foundry-idea-demo:tools/idea-evaluator/`.
  Sensor Tower market fetch not yet ported (deferred).

### P5 — Graduation action

Turn a greenlit idea into its own game project.

- Reuse `create_project` (`up.rs:595`) + `local::github::create_repo` to make a
  new repo, seed it with `theses/<slug>.md` (founding GDD) + a design-checklist
  stub (mirror FOUNDRY `server/create-project.mjs`).
- Create the orx project with `persona = game-designer` (auto-build on node
  creation turns on via `has_play_surface`, `src/local/play.rs:165`).
- Add a `linked_project_id` (+ repo) pointer on the idea node so the gallery
  shows "→ linked: <slug>" and can navigate across.

### P6 — Playable-settings write path (gap to close)

There is currently **no write path** for `play_command` / `play_dir` — defaults
`npm run build` / `dist` are effectively hard-wired (every construction site
passes `None`; `UpdateProjectReq` at `up.rs:725` doesn't accept them). Only
needed if a graduated game uses a non-Vite/non-`dist` build. Fix: extend
`UpdateProjectReq` + a UI control, or seed the columns at graduation. For the
default Vite template (`base: './'`, `dist`) it works as-is.

---

## The four personas / skills

| Persona | Role | Playbook | Skill(s) |
|---|---|---|---|
| `producer` | Control tower: fan out analysis, suggest providers, dispatch, decide graduation | `SYSTEM_PROMPT_PRODUCER.md` | `orx-produce` (the funnel loop, `orx agent suggest`, provider-matching judgment, verdict/greenlight criteria) |
| `idea-foundry` | Interview one idea → thesis (SHIPPED) | `SYSTEM_PROMPT_IDEA.md` | `orx-ideate` |
| `analyst` | Evaluate one idea node via the pinned sim + write prose | `SYSTEM_PROMPT_ANALYST.md` | `orx-evaluate` (run the evaluator sim, read metrics, write the report + verdict rationale) |
| `game-designer` | Build a playable prototype (SHIPPED) | `SYSTEM_PROMPT_GAME.md` | `orx-play`, `orx-experiment-tree` (game), … |

`producer` and `analyst` are new `Persona` enum variants
(`agent_skills.rs:57-66`, bump `ALL`), each with `as_str/parse/label/blurb`, a
`persona_template` arm (`opencode.rs:120`), a `skills_for_persona` arm, and the
two persona tests updated (`opencode.rs` skills-index + no-unresolved-placeholder
matches). Same pattern the `idea-foundry` persona already followed.

**`orx-produce` must carry provider-matching guidance** (the suggestion
intelligence): cheap/fast model for signal-coding analysis, strong coding model
for prototyping, respect the human's budget — always as a *suggestion* the human
overrides.

---

## UI changes

Mostly additive; the Experiments tree, Play iframe, run metrics, persona color
bars, and harness/model picker already exist.

- **Session list → nested tree**: sub-sessions nest under their parent, each row
  showing a **provider glyph** + the **persona color bar** (exists:
  research=blue, game=orange, idea=purple; add producer + analyst colors) + the
  **bound node**. (`ui/src/components/ChatPanel.tsx` session rows, `App.tsx`.)
- **Dispatch/spawn card** in the chat stream: pre-filled + editable
  provider/persona/model + task + "why", `[Dismiss]`/`[Spawn]`; provider list =
  installed+authed harnesses (`/api/harnesses` `agentReady`). New wire type
  alongside the existing `WirePrompt` kinds (`src/local/chat/mod.rs:62`).
- **Node hover actions** "Analyze" / "Build prototype" → the same spawn card
  with no pre-fill (`ui/src/components/TreeView.tsx`, `DetailDrawer.tsx`).
- **Provenance badges** on nodes ("evaluated by GPT", "built by Codex") +
  **"→ linked: <slug>" chip** on graduated idea nodes.
- **Needs-you bubbling**: a subagent hitting `AskUserQuestion` flips its row to
  `needs-you` and badges the parent + rail.

---

## Data model changes (summary)

- `chat_sessions`: `+ persona`, `+ experiment_id`, `+ parent_session_id`.
- idea node (experiment): `+ linked_project_id`, `+ linked_repo`,
  `+ evaluated_at_commit` (or store on the node's metrics/notes).
- `agent_proposals` (new): pending suggestions awaiting human approval
  (persona, suggested harness/model, parent expId, task, why, status).

---

## Phased build order

- **Phase 0 — evaluator sim** (P4): standalone, testable; port eval-core/eval-data
  + market fetch; `orx exp run --kind sim` produces metrics + report. No
  orchestration yet. Ships analysis value immediately (human runs it per node).
- **Phase 1 — per-session persona** (P1) + session↔node link (P2). Enables mixed
  personas on one project; still human-created sessions.
- **Phase 2 — dispatch** (P3): `orx agent suggest/spawn/status/wait` + internal
  authed API + the approval card UI.
- **Phase 3 — Producer + analyst personas** and wiring: Producer fans out
  analyst subagents over the gallery; verdicts.
- **Phase 4 — graduation + game-designer dispatch** (P5) + playable-settings
  write path (P6) as needed.

## Open gaps / risks

- **Comparability vs cross-provider analysis**: mitigated by pinning the scoring
  model in the sim (P4). Revisit if analysts diverge.
- **`play_command`/`play_dir` have no write path** (P6) — fine for the default
  Vite template, a blocker for other bundlers.
- **Subpath portability**: play builds serve under `/play/<expId>/`; a build lint
  fails on root-absolute asset refs. Graduated game template must use
  `base: './'` / relative assets.
- **Secrets in sim env**: the evaluator needs scoring-model + Sensor Tower keys
  available to the local run.
- **Auth on the internal spawn API**: must be token-gated; the unauthenticated
  loopback API is not an acceptable spawn channel.
