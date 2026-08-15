# Merging from upstream: what to know before you start

**Date: 14 August 2026.** Written after a long session of doing it. The research
and the decisions live in [the review](./harness-and-upstream-review.md); this is
the operating guide — read it first, it will save you a day.

Upstream is `alphaXiv/openresearch-cli`. We track it deliberately (decision
recorded 14 Aug), but it is building a research product and we are building a
game-design one, and that difference is the source of nearly every trap below.

---

## The one rule

> **`ui/src/api.ts` is our contract of record and does not move**, unless our
> backend moved with it in the same change.

Upstream UI that references an api symbol we don't export is not a styling
change — it is UI for a *backend* commit we haven't merged. Rewrite the
reference to our spelling, or drop that feature.

Never widen `api.ts` to make an error go away. Merging it once broke `App`,
`ChatPanel` and `DetailDrawer` simultaneously, and — the real cost — removed the
only check that was catching the problem. With `api.ts` frozen, `tsc` names
every offending line and a green build is *evidence*. Widen the contract and the
compiler will happily accept UI for a server that doesn't exist, which then
fails at runtime where neither `tsc` nor the screenshots are looking.

## The check that saves the most time

**Before calling a missing symbol a blocker, look for it under another name.**
Three of the four "blockers" identified in that session dissolved on inspection:

| "Missing" | Actually | Fix |
|---|---|---|
| `toolsFound` | our `gitFound` | one token |
| `AgentSelection` | our `ModelSelection`, field for field | one line |
| `bypassPermissions` | our `bypass` (their `from_id` is just lenient) | one token |
| `configuredDefaultBackend` | our `defaultBackend` | one token |

Only genuinely-new server concepts are real blockers — `enabled`/`disabledReason`
on compute targets, `githubEnabled`, `cloneUrl`/`createFolder`. Those have
nothing to rewrite to, so the feature comes out.

## Check whether we already built it

We are ahead of upstream in places, and a commit that sounds like a gap may be
one we solved first, differently:

- **`c43180c` "make experiment wake-ups opt-in"** — we shipped `autoPrompts` in
  `758ad45`: per project, all defaulting off. Months earlier. Do not take it.
- Our **`orx agent` subagent dispatch** is harness-agnostic; upstream's is a
  native Task-tool nesting we deliberately steer away from.
- Our **`orx mcp-gate` + `plan_gate`** approval bridge has no upstream equivalent
  (they kept both, so there is no collision — but do not assume their approval
  work replaces ours).

---

## Tools

**Three-way merge, not manual re-application.** `git merge-file` with the
merge-base as ancestor put `SettingsPage` at 19 conflicts in a 3342-line file:

```sh
git show 5e25afa:ui/src/components/X.tsx > /tmp/X.base     # merge-base
git show upstream/main:ui/src/components/X.tsx > /tmp/X.theirs
cp ui/src/components/X.tsx /tmp/X.merged
git merge-file -L ours -L base -L upstream /tmp/X.merged /tmp/X.base /tmp/X.theirs
```

Conflicts sort into three buckets: **import unions**, **pure class-name swaps**
(take theirs — that is the conversion), and **our features** (keep ours).

**Screenshots before touching anything.** `scripts/screenshots.sh` — see its
header. Capture, change, capture, compare. It caught a `NewProjectForm` change
with no guesswork, and it is the only thing that can catch a silent CSS
regression.

**Do not build tooling for this.** A preflight was written to predict which
components could be taken, and the same brace-matching bug went into it three
times — an apostrophe inside a comment swallowed the rest of the file, and the
fix made it skip components instead. Every version printed a confident, wrong
list. `tsc` already answers the question exactly.

---

## Traps, each one paid for

**A clean merge can still be wrong.** `BackendLogos` merged with zero conflicts
and had two copies of `RayLogo` and `OpenResearchLogo` — both sides had added
them at different offsets. Only `tsc` noticed.

**Conflict boundaries land mid-function.** Taking "ours" at a hunk whose boundary
sits inside a function leaves the other half of the function behind. This
produced an unclosed `list_project_branches`, a duplicated `project_json`, and a
`list_skills` missing its query parameter — all caught by the compiler, none by
reading carefully.

**A commit's payload carries their product model.** `4cffa77` is described as
"make file references clickable" and its diff rewrites our reports skill to a
flat artifacts directory, explicitly un-reserving the `project/` namespace our
layout depends on. Port the idea; do not take the text.

**Inheriting a deletion is as consequential as inheriting an addition.**
`3cb782e` *removed* the per-commit browsing api (`listExperimentCommits`,
`getCommitDiff`, `getWorkingTree`). Our `DetailDrawer` still uses it and our
server still serves it, so the removal had to be reverted after the fact.

**Screens with the chat pane never reproduce byte-for-byte.** They were gated
three separate times on two runs, then three runs agreeing, and drifted every
time. The flake rate is low, not zero. They stay advisory.

---

## What to take next

**Wanted, in upstream order:**

1. **`055e6bf` — Codex approvals + permission cards.** The big one: 60+ hunks
   across 17 files, 945 lines of approval flow in `chat/mod.rs`, 28 hunks in
   `ChatPanel`. It does *not* collide with `mcp-gate`/`plan_gate` — checked. It
   is simply a coherent redesign spanning both halves. Budget days, not hours,
   and do it when our approvals actually need something it offers.
2. Anything newer than 13 Aug 2026 — unreviewed, ~54 commits behind and growing.

**Still unconverted** (all work today, all keep our styling until touched):
`ChatPanel`, `Md`, `DetailDrawer`, `SubagentTab`, `Header`, `App`, `FileViewer`,
`TreeView`, `NewProjectForm`, `Onboarding`, `ProjectsHome`, `StatusBadge`.

Two of those are blocked by more than effort:

- **`Md`** needs four new dependencies, one a JSR package not on npm
  (`@clo/react-markdown`). It swaps the markdown renderer — behavioural, on
  screens with only advisory coverage.
- **`ChatPanel`** imports `LitSourcesPicker`: alphaXiv/OpenAlex/bioRxiv toggles
  wired into the composer, backed by an endpoint we don't serve. The research
  pivot lives *inside* the file, so taking it means surgery.

**Never take:**

- `7dc5155` — copy hygiene for their prompts; ours are different prompts.
- `68f1daf`, `75b01c6`, `601a201`, `b2c52a3`, `5412465`, `b4ee41e` — researcher
  onboarding.
- `a80f1c9`, lit sources — literature review.
- `3c30487` — nanochat demo seed.
- `dccc37c`, `4f0fb0b`, `8c17722` — OpenResearch.app DMG; fights our branding
  and the `69ada3b` updater repoint.
- `c70e387`, `c1e41af` — telemetry; we point at our own PostHog (`4d4278d`).
- `f6a7198` — their codeowners.

**Open question:** `6e4f998` "make local file projects the default" does the same
job as our `3cf8913`, and supporting their GitHub-less projects means changing
the project model — 2401 lines across 30 files. One has to give, and nothing
forces the choice yet.

---

## Where a file stands

Twenty UI files are byte-identical to upstream and will merge cleanly forever.
About eight carry our features on upstream's converted markup and will conflict
manageably. The rest are pre-Tailwind. To check any one file:

```sh
git diff --quiet upstream/main HEAD -- ui/src/components/X.tsx && echo identical
```
