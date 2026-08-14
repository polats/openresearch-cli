# Harness and upstream review

**Date: 14 August 2026.** A research pass over three questions asked together:
what upstream work we haven't merged, what it would take to add Pi as a fourth
harness, and what DeepSeek Harness (`dsh`) and the Cordis paper have to teach us.

The conclusion is mostly **don't**. This document records the evidence so the
question doesn't get re-opened from scratch, and so the one decision that *is*
live — whether we keep tracking upstream — gets made with numbers attached.

Companions: [scenario integration spec](./scenario-integration-spec.md),
[idea pipeline orchestration](./idea-pipeline-orchestration-plan.md).

---

## Status

| Question | Answer |
|---|---|
| Merge upstream? | 3 bug fixes yes; the other 45 commits depend on a strategic decision |
| Add Pi as a harness? | No, absent a concrete driver |
| Adopt anything from `dsh`/Cordis? | No architecture; two ideas parked for later |
| Live decision | **Keep tracking upstream, or hard-fork?** Blocks everything else |

---

## 1. Upstream sync state

`upstream` is `alphaXiv/openresearch-cli` (push-disabled). Last sync point is
`5e25afa` — *"Surface silently-dropped chat turns (v0.1.87) (#139)"*, **29 July
2026**, landed via `upstream-sync-tranche1..4` (all four merged into `main`).

- `main` is **50 behind / 50 ahead**. Upstream is on v0.1.101; we version on our
  own 1.x line.
- Two of the 50 we already cherry-picked out of order — `4ff8e3a` (Claude auth
  recovery) and `3e93476` (Ray Jobs) — so **48 are genuinely new**.
- `splats/pipeline-and-viewer` is 8 ahead of `main`, nothing behind.

A trial merge of `upstream/main` into `main` (throwaway worktree, discarded):

| | Files |
|---|---|
| Auto-merge clean | 229 |
| Pure additions from upstream | 109 |
| **Conflicted** | **55 (217 hunks)** |

### The blocking issue: Tailwind

`8100bdf` **OR-149: Replace CSS files with inline Tailwind classes** deletes
`ui/src/styles.css` (6,162 lines) and replaces it with `tailwind.css` +
`styleClasses.ts` across 45 UI files. Our Crux rebrand lives in that file
(+1,167 lines) plus a `theme.ts` upstream independently rewrote. The merge
reports `UD ui/src/styles.css` (they deleted, we modified) and `AA ui/src/theme.ts`
(both added, whole-file conflict).

Every upstream UI commit after 10 August is written against the Tailwind world.
**Until we port the Crux theme to Tailwind, the UI half of upstream is frozen for
us.** That is the real decision, not "which commits."

### What's worth taking

**Bug fixes — worth taking under any strategy:**

| Commit | What | Cost (measured, see §6) |
|---|---|---|
| `2530c65` | Handle retained provisioning failures | 0 conflicts |
| `a94c399` | Queue messages sent mid-turn (kills "session is busy — interrupt it first") | 2 conflicts + 2 unmerged-feature refs + Tailwind markup |
| `f28055c` | Fix stale run cancellation recovery | 11 conflicts / 15 hunks + 1 modify/delete |

**Only if we keep tracking upstream:**

`055e6bf` (Codex auto-approvals + permission card redesign — supersedes our
`d7075a1` bypass-spawn hack), `4738715` (Skills tab — on-thesis, we ship
`orx-splat`/`spritekit`/`propkit`/`splatkit`), `c43180c` (experiment wake-ups
opt-in), `4cffa77` (clickable file references).

**Skip — upstream's research pivot, actively wrong for Crux:**

`3c30487` nanochat demo, `a80f1c9` lit review chat, `75b01c6` research profiles,
`601a201`/`68f1daf`/`b2c52a3`/`5412465`/`b4ee41e` researcher onboarding,
`0ddf502` OpenResearch signup docs, `dccc37c`/`4f0fb0b`/`8c17722` OpenResearch.app
DMG (fights our branding and the `69ada3b` updater repoint), `f6a7198` codeowners,
`c70e387`/`c1e41af` telemetry (we deliberately repointed at our own PostHog in
`4d4278d`).

**Judgement call:** `6e4f998` "make local file projects the default" (30 Rust + 13
UI files) is upstream doing the same thing as our `3cf8913`. One of the two has to
give — read theirs before building further on ours.

**Blocked on Tailwind:** `049891d`, `7edd229`, `12968ac`, `2379b1a`, `e53a84d`
(conflicts head-on with our `79d2640` composer rework), `323799a`, `9508aed`.

### The trend

The shared surface is shrinking every week. Upstream is building researcher
onboarding, lit review, paper integration, and a signed OpenResearch.app. We are
building game asset pipelines, splat viewers, and play sessions. We have already
reverted their telemetry and repointed their updater. Merges get more expensive
and buy less each time.

---

## 2. Pi as a fourth harness

[Pi](https://github.com/earendil-works/pi) (`@earendil-works/pi-coding-agent`
0.84.1) is a Node CLI agent, multi-provider, OAuth in `~/.pi/agent/auth.json`.
Our sibling project `tris-bot` runs it as its runtime.

**Verdict: don't, absent a concrete driver.** We have Claude, Codex, and OpenCode
working. Pi's advantages — multi-provider, cheaper models — OpenCode already has.
The reason it came up is that tris-bot uses it, which is a "we have this
elsewhere" reason, not a "crux needs this" one.

The research is recorded here because if a driver appears, it shouldn't be redone.

### Why it would fit well

`pi --mode rpc` is newline-delimited JSON over stdio — architecturally the same
shape as `src/local/codex.rs` ("one long-lived `codex app-server` child per chat
session"), but simpler: no JSON-RPC envelope, no bidirectional id correlation.
`docs/rpc.md` in the npm package is 1,579 lines and unusually complete.

| Crux needs | Pi provides |
|---|---|
| `detect()` auth | `pi auth check --provider X --json` → `{"status":"ready","provider":"anthropic","authType":"oauth"}` |
| `detect()` models | `get_available_models` → objects with `contextWindow`, `cost`, `reasoning` |
| `run_turn` | `prompt` + `message_update`/`tool_execution_*` events |
| native session id | `--session-id <id>` **creates if missing** — better than Claude's `--resume` |
| `generate_title` | `set_session_name` / `-n` |
| `report_usage` | `get_session_stats` → `contextUsage.percent` |
| reasoning levels | `set_thinking_level`: off/minimal/low/medium/high/xhigh/max — near-exact match to `options.rs` |
| mid-run steering | `steer` / `follow_up` **natively** (what upstream's OR-166 just built) |
| orx skill shim | pi reads `~/.agents/skills/` — **where we already write Codex's shim.** Free. |
| `session_skills_dir` | project `.agents/skills` — same string Codex returns |

### The gaps

`docs/usage.md:303` is explicit: *"It intentionally does not include built-in MCP,
sub-agents, permission popups, plan mode, to-dos, or background bash."*

1. **No permission model, no sandbox.** Fixable with a crux-authored pi extension:
   `pi.on("tool_call")` can return `{block: true, reason, terminate}`, and
   `ctx.ui.confirm()` surfaces in RPC as `extension_ui_request`/`extension_ui_response`
   — which maps exactly onto our inline-approval path (`ResumeAction::Handled`, same
   as OpenCode). Pi ships `examples/extensions/permission-gate.ts` and a full
   `examples/extensions/plan-mode/` as references. Plan mode via `pi.setActiveTools()`
   would be *cheaper* than our 732-line `plan_gate.rs`.
2. **No MCP client at all.** No `@modelcontextprotocol` anywhere in the package.
   Would need an MCP→`pi.registerTool` bridge extension (~200–300 lines TS), or pi
   sessions ship without Blender/ComfyUI/Scenario. Note tris-bot didn't bridge — it
   hand-wrote `.pi/extensions/{scenario,generation,browser}` natively.
3. **Project trust.** Non-interactive modes never prompt and, under the default
   `defaultProjectTrust: "ask"`, **silently ignore** project resources — including
   the per-session `.agents/skills` we write into every worktree. Must pass
   `--approve`/`-a` on spawn. Silent-failure trap.

### Permanent losses (would not reach parity)

- **Plan/quota display.** Claude has a live probe (`harness/claude.rs:707`); Codex
  captures `rateLimits` on-turn (`harness/codex.rs:854`). Pi has neither. An
  extension can read `after_provider_response` headers, so API-key rate limits are
  recoverable, but subscription plan windows (the 5h buckets we render) are not.
  Pi would land where OpenCode is: cost-derived only.
- **Native subagent nesting.** `harness/claude.rs:1196` routes Task-tool subagent
  activity into the Task row's children. Pi has no native subagent tool. Mostly
  doesn't matter — our `orx agent suggest` dispatch is harness-agnostic, and
  `src/local/opencode.rs:155` already tells agents *not* to use their native
  subagent tool.

### Cost if we ever do it

Every chat harness is two files: a process host in `src/local/<name>.rs` and a
protocol adapter in `src/local/harness/<name>.rs`.

| Existing | Host | Adapter |
|---|---|---|
| cursor (detect-only) | — | 41 |
| opencode | 909 | 1,758 |
| claude | 1,063 | 2,925 |
| codex | 851 | 4,287 |

A pi host would be ~600–800 and an adapter ~1,200–1,600 *if* the protocol's
simplicity holds — but claude and codex were presumably also going to be simple.
Plus a TypeScript extension. **Realistic estimate: 3–4 weeks, not 1–2.**

Edits elsewhere: `harness/mod.rs` registry line; `chat/mod.rs:526` (ChatHost has
*named* host fields, it is not generic); `mcp_servers.rs`; `commands/up.rs`;
`ui/src/api.ts:1127` HarnessId union; `ModelPicker.tsx:27`; `SettingsPage.tsx:288`;
`ChatPanel.tsx:1893`; a logo. No DB migration — `chat_sessions.harness` is TEXT.

---

## 3. DeepSeek Harness and the Cordis paper

`dsh` is ~497k lines of TypeScript across 7,412 files and ~50 package groups.
Crux is 53k Rust + 23k UI across 314 files. Different weight classes.

`dsh` is not a competitor exactly — it's a harness, the layer we wrap. But it has
independently built our harness-adapter layer: `packages/subagent/` holds
`subagent-claude-code`, `subagent-codex`, `subagent-acp`, `subagent-fork-in-process`
and three more, all behind one provider interface.

The paper (*A Programming Paradigm for Spatiotemporal Composability*, 88pp) is
serious PL theory: revertible effects (every context transformation carries an
inverse the runtime tracks) and reactive coeffects (components declare
dependencies; context changes notify them), unified into a context type, with a
calculus and confluence proofs. **§1–4 is not actionable for us. §5.2 and §6 are.**

The authors' own §5.3 is worth quoting: *"a single ecosystem in a single host
language… observational rather than a controlled comparison… an
existence-and-adoption result rather than a quantitative one."* It is a well-argued
design philosophy with a formal skeleton, not evidence the architecture wins.

### What doesn't port — and the paper says why

§6.4 lists what a host language must supply: runtime module load/unload (Node has
a module registry; *"Native code exposes no module registry"* and needs
dlopen/dlclose) and dynamically mediated dependency access via a Proxy-like
primitive. Rust has neither ergonomically. §6.5 is a warning, not an invitation:
fine-grained decomposition means *"the number of integration components can grow
quadratically."* dsh's 7,412 files are what that costs.

**Our `Harness` trait does the same job as their subagent seam at a fiftieth of
the size.** The capability-defaults design (detect / chat / skill-install, each
independently optional, Cursor implementing only the third) is right. Don't
re-architect toward dsh.

### Two ideas parked for later

**Append-only session log.** dsh's `Session` is an append-only typed event log;
message history is *derived*, never stored. Our `chat_messages(… parts_json …)`
(`store.rs:299`) stores whole blobs, and `flush()` (`chat/mod.rs:1988`) rewrites
the **full** assistant message every 150ms. We have already paid for this twice:
`TOOL_TEXT_CAP = 16_000` (`chat/mod.rs:36`) exists explicitly because "every flush
re-broadcasts (and re-persists) the FULL assistant message", and the `msg_write`
mutex (`chat/mod.rs:558`) exists because `flush` and `respond` both read-modify-write
the same row. Real, but latent — the cap already mitigates it. **Cheap version:
skip the flush when parts haven't changed (~2 hours).**

**Closed approval outcome.** dsh's `ApprovalOutcome` is
`allowed-once | rejected | cancelled | unavailable`, explicitly fail-closed. Our
`PermissionDecision` (`chat/mod.rs:427`) is Claude's wire type wearing a Rust name
— `#[serde(tag = "behavior")]`, two variants, serialized verbatim into Claude's
permission-prompt-tool contract, and wired only in plan mode for Claude. The
two-tier design in `request_permission` (`chat/mod.rs:712`) is already right; only
the *type* is Claude-shaped. **Worth doing only as part of adding a harness with
no native gate (i.e. Pi).**

### MCP: a correction

An earlier draft of this analysis claimed we should route generative tools through
an orx-side proxy instead of per-harness MCP renderers. That was wrong — **we
already run both halves of that pattern:**

- Blender and ComfyUI are HTTP MCP servers *crux hosts itself*
  (`local/blender/server.rs:280`, `local/comfyui/server.rs:384`).
- We already ship a stdio MCP server of our own: `orx mcp-gate`
  (`main.rs:182`, `commands/mcp_gate.rs`), spawned as Claude's child for the
  permission bridge.

And the N×M is small: `mcp_servers.rs` is 178 lines with three ~10-line renderers.
A new harness *that has MCP* costs about ten lines. **Not worth restructuring.**

### Strategic note

DeepSeek shipping an MIT-licensed, "everything is a plugin" harness in developer
preview, with a `dsh-plugin` GitHub topic and a Discord, is a platform play. dsh
has profiles and bundles that install out-of-tree plugins, and no domain layer of
its own. If it gets traction, "ship the Crux game pipeline as a dsh bundle" becomes
a real distribution option — we'd supply exactly the vocabulary they've left empty.
Worth a bookmark, not a bet; they're pre-1.0 and warning about breaking changes.

---

## 4. Scorecard

Honest scoring, including a bias worth naming: comparing a 76k-line solo product
against a 500k-line corporate framework will *always* produce a list of things
we're "missing," and it doesn't account for missing them being the correct call at
our size.

| Change | Ships as estimated | Real benefit | Verdict |
|---|---|---|---|
| Cherry-pick 3 upstream fixes | ~85% | Medium — fixes bugs we plausibly have | **Do** |
| Full tranche-5 (8 commits) | ~60% | Low-medium | Depends on the upstream decision |
| Flush skip-if-unchanged | ~90% | Low, but ~2 hours | If chats feel sluggish |
| Approval outcome type | ~90% | **Near zero on its own** | Only as part of Pi |
| Pi harness | ~40% at 1–2 weeks | **Unclear** | No, absent a driver |
| Event-sourced chat log | ~50% | Low today | No |
| Tailwind port | ~70% | Zero product value | Only if we keep tracking upstream |

---

## 5. The live decision

**Keep tracking upstream, or hard-fork?** Both are defensible.

**Keep tracking** — we inherit their harness work (Codex approvals, streaming
markdown, file rendering) for free, which is real: that's the layer we'd otherwise
maintain alone. Cost: the Tailwind port, plus a merge tax that grows every week.

**Hard fork** — declare independence, cherry-pick individual bug fixes when we
notice one, never merge again. Cost: we own the harness layer forever. Benefit: we
stop paying a tax on a codebase heading somewhere else, and Tailwind becomes a
non-question.

Everything else scores differently depending on this answer. The Tailwind port is
either mandatory or moot. Tranche 5 is either the first of many or the last one.

**Nothing else on this list should go on the calendar until this is decided.**

---

## 6. Done

**14 Aug 2026** — cherry-picked `f28055c`, `a94c399`, `2530c65` (upstream order)
onto `upstream-fixes-aug14`. 413 tests pass (was 410), `cargo fmt --check` and
`cargo clippy --all-targets` clean, `ui` typechecks and builds.

The three commits cost more than the estimate in §1's first draft, in ways worth
recording because they generalize to any future tranche:

1. **`f28055c` was not "small."** 11 conflicted files, 15 hunks, plus a
   modify/delete on `ui/src/components/ExperimentOverview.tsx` (deleted in our
   rebrand; kept deleted). Resolutions of note:
   - `commands/up.rs` — `ApiRun` gains `cancel_requested`; the cancel handler
     keeps our play-session short-circuit but swaps the bare
     `set_cancel_requested` for upstream's `request_local_run_cancel`, which is
     the actual fix (locks, persists intent, respawns an orphaned supervisor,
     rolls back on spawn failure).
   - `commands/supervise.rs` — upstream's terminal-status early return placed
     *before* our watcher-heartbeat task, so a supervisor respawned for a
     finished run exits instead of idling.
   - `TreeView.tsx` — adopted `runDisplayStatus` + the `cancelling` live state,
     kept our game vocabulary (Idea/Building/Merge/Prototype) and dropped
     upstream's re-added run-squares row.
2. **Two upstream tests were dropped**, both asserting behaviour from commits we
   have *not* merged; they rode along only because git couldn't split the block:
   - `remote_clone_fetches_recorded_commit_detached` — asserts an
     `hf_clone_script` that fetches a recorded commit detached. `f28055c` never
     touched `hf_clone_script`; ours still uses `clone --depth 1 --branch`.
     Taking the test would have meant taking an unrelated behavioural change.
   - `project_json_exposes_artifacts_dir_with_legacy_alias` — builds a
     `LocalProject` with `github_sync_enabled`, a field our struct doesn't have.
   Upstream test fixtures also needed our six extra `StoredRun` fields.
3. **The Tailwind blocker is already live, not hypothetical.** `a94c399` (11 Aug)
   is post-migration, so its queued-message chip markup arrived as Tailwind
   utility classes — dead strings in our build. Resolved by keeping the semantic
   class names and writing real CSS into `styles.css` (`.composer-queued`,
   `.queued-chip*`). Every future UI pick will need this same translation until
   the theme is ported.
4. **`a94c399` also referenced an unmerged feature.** Its queue path called
   `setAttachError` and read `a.name` off an attachment — both from the PDF/file
   upload commit `899c3b6`. Dropped to match our existing send path.

**Read-through:** upstream commits do not arrive self-contained. Two of three
carried assertions or call sites belonging to commits we skipped, and one
carried markup for a UI system we don't have. Budget for that, or take whole
tranches in upstream order rather than cherry-picking across gaps.

Everything else waits for a concrete trigger: Pi if a real reason appears, the
event log if a chat gets slow, the approval type if Pi happens.

The harness layer is inherited infrastructure that currently works. Our
differentiator — the experiment tree, play sessions, verdicts, the asset pipeline
— is the part nobody else is building, including dsh and including upstream.
