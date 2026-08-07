# Scenario asset workflows: integration spec

Status: **step 1 built and verified against the live workspace; steps 2–8 designed,
two decisions open**. Author-facing implementation plan.

This brings Scenario (AI asset generation — images, video, 3D) into crux as a
workflows surface for artist teammates, reached over MCP rather than Scenario's
REST API. Step 1, the MCP client and its login, ships on `scenario-mcp-client`.

## Goal

Artists compose art-asset pipelines on a React Flow canvas: generate an image via
Scenario *or* drag one in from the project's files, then use it as the first frame
for video generation. ComfyUI-shaped chaining, but with Scenario as the engine and
**no sampler internals exposed** — the audience is artists, not engineers.

## Locked decisions

1. **MCP, not REST.** `https://mcp.scenario.com/mcp`, OAuth against the user's own
   Scenario SSO via Clerk dynamic client registration. Nobody holds an API key.
   The REST path (Basic auth, three calls per generation) was the cheaper build
   and was rejected for exactly this reason.
2. **Workflows are studio-wide, not per-project.** They live at the data-dir
   level as a reusable template library you instantiate; runs record which
   workflow version produced them.
3. **Result caching is core to the executor, not an add-on.** Cache key = hash of
   (tool name, model id, resolved params, input asset hashes). Because the key
   includes upstream asset hashes, editing a prompt late in the graph re-runs only
   downstream nodes. Video generation is slow and metered — a graph that re-runs
   everything on each edit burns credits fast.
4. **Usage reporting is team-wide**, not per-user.
5. **Two asset browsers**, mirroring tris-bot's split: the *local* content-hashed
   registry (view-only, the drag source for the canvas) and the *remote* Scenario
   workspace (with a Pull action that downloads bytes into the local registry).
6. **The login is reachable from the dashboard**, not CLI-only — artists need to
   sign in without a terminal. The CLI path stays as the escape hatch for when the
   dashboard is remote.
7. **Auth failures are their own error class.** "The user must log in again" is
   distinguishable everywhere it matters: the CLI prints a `connect` hint, the HTTP
   layer answers 401, the Settings card offers Reconnect. Never a generic error —
   that distinction is what makes the admin UI honest.

## Prior art

tris-bot's `.pi/extensions/scenario/` and `web/lib/scenarioMcp.ts`. The pattern
worth copying is the **interactive/silent split**: one file owns the browser-opening
flow, everything else refreshes tokens but can never start a login, so a
server-side request can't block on a login nobody is watching.

crux gets the split more cheaply than tris did — `crux up` is a single long-lived
local server that already binds loopback and already opens browsers, so it can own
both halves in one process. Tokens are still stored at `0600` and written
atomically, because `orx` CLI invocations run as separate processes from `crux up`
and a refresh in one must not leave the other holding a refresh token Clerk has
already invalidated.

## Steps, in build order

The order is deliberately not the dependency order: it puts something visible in
front of the artists after step 3, well before the canvas exists.

1. **MCP client + connect flow.** `src/local/scenario/`. Silent token refresh, a
   typed `call_tool`, and the one interactive `connect`. Verified with
   `crux scenario tools` / `call` against a real workspace. **Done.**
2. **Asset registry.** Content-hashed store with JSON sidecars, following
   tris-bot's `writeAsset` discipline: sha256 id so repeat saves dedupe,
   temp-write + rename so readers never see partials, sidecar written last.
   Registered into the project's `files_dir` so `FilesTab` and chat links work for
   free. Download host pinned to `cdn.cloud.scenario.com` so the endpoint can't be
   aimed at arbitrary origins. Kind detected by sniffing magic numbers, not the
   CDN's content-type — tris found it serves 3D models as `octet-stream`.
3. **Local asset browser.** The registry as a filterable grid: kind chips, label
   search, lightbox with provenance (which workflow run and node produced it).
   This is what artists drag onto the graph.
4. **Admin card + usage.** Settings → Scenario: connection state, model catalog,
   consumption. See the usage note below — this is *not* the harness-shaped
   remaining-percent bar.
5. **Graph data model + persistence.** Nodes, edges, per-node params. See the
   node-granularity finding below — nodes are coarse studio steps, not sampler
   primitives.
6. **Executor.** Topological walk, one job per node, poll with backoff, per-node
   status persisted so a reload resumes rather than restarts. Progress over the
   existing SSE `/api/events`. Caching per decision 3.
7. **The canvas.** Editable React Flow — `nodesDraggable` / `nodesConnectable` on,
   a small node palette, drag-from-files, thumbnails on nodes, per-node Run and
   Run All. `@xyflow/react` is already a dependency and `TreeView.tsx` proves the
   node-rendering idiom, but that view is read-only and derived
   (`nodesDraggable={false}`, positions computed from experiment parentage), so
   this is "the library is proven here", not "half the feature exists."
8. **Scenario workspace browser.** Remote assets via the MCP `search` tool, with
   Pull into the local registry.

## What live introspection changed

Step 1 turned up four things that revise the design. All four came from calling
the real workspace, not from documentation.

**There is no image tool or video tool — there is `model_run`.** One universal
entry point whose parameter contract differs per model; its own description says
to call `model_schema_get` first, and warns that *"a parameter's name does not
tell you its cardinality"* (`images` / `referenceImages` want arrays even for a
single asset, flagged `array: true`). This kills fixed-field node designs: node
params must be **schema-driven** — pick a model, fetch its schema, render inputs
from it. Confirmed on `model_wan-2-7-i2v`, which uses singular `image` as a plain
`file`, *not* the array form, which is precisely why the schema has to be fetched
rather than assumed.

Also: some constraints exist only in prose (`endImage` "requires start image",
`video` "cannot be combined with start image"). No JSON schema expresses those, so
either hand-encode them per model family or surface the description text so the
artist sees it.

**Usage is compute units over a date range, not a quota window.** Real data:
`totalCU: 60, users: 1` for 2026-07-01 → 2026-08-05, with per-model and per-kind
breakdowns and optional daily timeseries (`include: ['usages.daily',
'modelUsages.daily']`). So the harness-shaped remaining-percent `ProgressBar`
doesn't fit — this wants consumption totals, a per-model breakdown, and a small
chart. Note the team currently has a single project (Sandbox), so team-wide and
project-scoped collapse today; the distinction reappears when projects are added.

**Blocklist filtering is not needed client-side.** `models_list` applies team and
project blocklists server-side: zero of the 98 returned models are blocked. The
348 team-level + 37 project-level blocked IDs are all superseded versions
(`kling-v2-*`, `ltx-2-*`, `wan-2-5/2-6`, `vidu-*`, `pika-*`). **34 of the 98 do
`img2video`** — the first-frame case this whole surface is for — including Veo 3.1,
SORA 2, Kling V3 I2V, Runway Gen4.5, Seedance 2.0, LTX 2.3, Luma Ray 3.2, Wan 2.7
I2V, and a FLUX.3 family with dedicated First-Last-Frame and Keyframes variants.
There is also `Sequence-to-Video` for image-sequence input, plus `-draft`/`-fast`
tiers worth defaulting to while iterating, given CU cost.

**Scenario already has visual workflows, and the team lives in them.** 172
workflows by 68 authors — 11 `ready`, 161 draft. `CoC Style` (authored 2026-05-20)
is a 4-node DAG whose entire artist-facing surface is one text field, because
`inputs_definition` collapses it:

| node | type | what it does |
|---|---|---|
| `image1` | `transform` | a literal list of 3 pinned reference assets — the style sheets |
| `promptBuilder1` | `transform` | string concat: wraps the object name in the CoC style prompt |
| `imageGenerator1` | `custom-model` → `model_openai-gpt-image-2` | `referenceImages` ← `image1`, `prompt` ← `promptBuilder1` |
| `tool1` | `custom-model` → `model_photoroom-background-removal` | `image` ← `imageGenerator1` |

Scenario has therefore already solved graph authoring: typed nodes, a
`ref: {node, name}` edge mechanism, a transform expression language, and
`inputs_definition`. Among the drafts are things directly on this roadmap —
`Sprite Sheet Generator`, `Character Run Cycle Spritesheet`, `3D Character
Auto-Rigging`, `Add characters into your video`, `looping video`.

**So the gap is not a node editor.** It is that only 11 of 172 are finished, they
are buried among drafts, and there is no way to chain two of them. The
recommendation is that crux **composes** Scenario workflows rather than replacing
them: each canvas node is a *studio step* — "run CoC Style", "animate this", "drag
in a file" — with workflow nodes rendering their own `inputs_definition` as the
form and model nodes rendering `model_schema_get`. Artists keep authoring style
workflows in Scenario's editor, where they already are.

## Open decisions

1. **Does the canvas compose Scenario workflows, or replace them?** Compose, per
   above. If artists should never leave crux, that is a substantially bigger build
   and it changes what step 5 *is* — so this blocks the graph model.
2. **Workflow node outputs may have to be untyped.** `outputAssetKinds` is empty
   on `CoC Style`, so a workflow's output kind can't be inferred from metadata
   without running it. That weakens typed ports for workflow nodes specifically —
   and typed ports are most of what keeps the canvas legible to non-engineers.

Worth doing before designing the Video node: read `looping video` and `FakeDoor
Video Ad Builder (Simple)`. They suggest video chaining already exists in some
form, and matching their conventions beats inventing parallel ones.

## Notes

- The dashboard is unauthenticated by design, so a Settings connect button means
  anyone who can reach the port can initiate a Scenario login as you. No worse
  than the existing surface, which can already run code as you, but worth stating.
- `crux scenario call teams_list '{}'` dumps ~1400 lines because the blocklists
  are enormous. If discovery tooling keeps getting leaned on, add `--compact` or a
  path selector.
- `rmcp` brings its own `reqwest 0.13` alongside crux's `0.12`, so two HTTP stacks
  ship until crux bumps. It brings no OpenSSL, so static musl release builds are
  unaffected — verified.
