# Sprite pipeline as a crux feature — build plan

**Date: 8 August 2026.** Companion to
[sprite-pipeline-plan.md](./sprite-pipeline-plan.md) (the pipeline and why),
[local-3d-capability.md](./local-3d-capability.md) (what this box runs) and
[sprite-animation-research.md](./sprite-animation-research.md) (the evidence).

**Goal.** Drop an image into a crux chat, watch the whole 2D → T-pose → 3D → rig →
motion → sprite chain run as visible tool calls, and browse the artifacts in the
existing Files tab.

**Constraint.** Every stage runs **locally** — through ComfyUI where possible, and
behind a crux-managed MCP harness where not, following the shape already
established by `src/local/blender/` and `src/local/comfyui/`.

---

## 1. Stage-by-stage: local how?

| # | Stage | Local via | Why |
|---|---|---|---|
| 1 | T-pose | **ComfyUI** | Needs a Kontext-class editor. **FLUX.2-klein-4B is Apache-2.0, ~13 GB**, and karate-wiener already has klein + LoRA tooling. Pure model download — no compiled extensions |
| 2 | Mesh | **ComfyUI** (Hunyuan3D, kijai) | Core `requirements.txt` is pure Python. Texture-bake and extras are blocked but unneeded. **Trellis is ABI-dead here** and cannot be the local path |
| 3 | Rig | **new `unirig-mcp` harness** → Docker `:8081` | woid already runs UniRig as a container with `POST /rig`. A container is immune to the torch 2.13 break. (`ComfyUI-UniRig` with its pixi-isolated env is the fallback if we'd rather stay in-graph) |
| 4 | Motion | **new `kimodo-mcp` harness** | **No Kimodo ComfyUI node exists.** Kimodo runs local at <3 GB VRAM with `TEXT_ENCODER_DEVICE=cpu`; `kimodo-zerogpu-space` already has a working venv and server |
| 5 | Render | **existing `blender-mcp`** | Already wired, already proven for ortho render + alpha |
| 6 | Quantize | **new `sprite-mcp`** (small) | Wraps `pixel.mjs` unmodified: the contract chain plus a palette check. Node, no GPU |

**Two new harnesses, one small tool server.** Nothing loads a compiled CUDA
extension inside the ComfyUI venv — the property that makes this viable at all.

### Downloads required (one time)

| For | File(s) | Size |
|---|---|---|
| Stage 1 | FLUX.2-klein-4B (+ optionally a style LoRA) | ~13 GB |
| Stage 2 | Hunyuan3D v2 DiT + CLIP-vision encoder | ~3–5 GB |
| Stage 4 | `nvidia/Kimodo-SMPLX-RP-v1` + LLM2Vec text encoder | ~10 GB |

~30 GB against 91 GB free. **Check the Kimodo checkpoint licence before anything
ships** — the R&D variants are non-commercial.

---

## 2. The two new harnesses

Both follow `src/local/blender/` exactly, because that shape is already proven and
the UI already knows how to render it.

```
src/local/unirig/mod.rs      ← UnirigStatus { mcp_found, mcp_runnable, service_reachable }
src/local/unirig/server.rs   ← managed spawn, url_for(port) -> http://127.0.0.1:<p>/mcp
src/local/kimodo/mod.rs      ← KimodoStatus { mcp_found, mcp_runnable, model_present, vram_ok }
src/local/kimodo/server.rs   ← managed spawn; TEXT_ENCODER_DEVICE=cpu
src/local/sprite/mod.rs      ← SpriteStatus { mcp_found, game_reachable }
```

**Layered status, not a bool.** Blender's three-layer split
(`server_found` / `server_runnable` / `blender_reachable`) exists because those
failures have different fixes, and the same is true here: "UniRig MCP not
installed" and "UniRig container not running" want different messages. Reuse
`missing_interpreter()`'s trick of reading the shebang to diagnose dead venvs.

**Registration** is one line each into `src/local/mcp_servers.rs`, which already
fans out to all three harnesses (`claude_entries`, `codex_overrides`,
`opencode_config`). No per-harness wiring.

**Regression test to copy from ComfyUI:** `spawn_mcp` there has a test asserting
the server starts *without* `--token`, added after the token broke every harness
while crux's own client reported healthy. Write the equivalent for both new
servers before trusting their status cards.

### Tool surfaces (polled, not hard-coded)

Per the ComfyUI precedent, poll `tools/list` rather than hard-coding. Expected:

- **unirig-mcp** — `rig_mesh(glb_path) -> rig_glb_path`, `inspect_rig(glb) -> bone table`
- **kimodo-mcp** — `generate_motion(prompt, seconds, constraints) -> npz_path`,
  `generate_loop(prompt, seconds, pose)` (pins frame 0 **and** last to the same
  pose — the documented 0 cm / 0 m-drift pattern), `retarget(npz, rig_glb)`
- **sprite-mcp** — `to_sprite(png, palette, w, h) -> png`,
  `check_contract(png, palette) -> report`, `emit_baked_ts(frames, class)`

---

## 3. crux integration

### 3.1 Artifacts land in the project files dir — no new UI

`<data dir>/files/<slug>/sprites/<class>/`:

```
00-source.png     01-tpose.png     02-model.glb
03-rig.glb        04-walk.npz      05-frames/{facing}-{n}.png
06-out/{facing}-{n}.png            07-<class>.ts
```

`/api/projects/{id}/files/file?path=` already serves these with
`content_type_for_path`, `FilesTab` already lists them, `FileViewer` already
renders PNGs, and `filesDir` is already injected into the project payload so
**chat messages can link artifacts directly**. Numbered prefixes make the stage
order self-evident in the tree.

**Known gap:** `.glb` and `.npz` won't preview — they'll list and download only.
Acceptable; a `Preview3D`-style viewer is a later nice-to-have, not a blocker.

### 3.2 Progress in chat is already how harnesses work

Each stage is one MCP tool call, so the existing chat stream shows it. What we add
is *legibility*: every tool returns a **files-dir-relative artifact path**, so the
agent's narration links to something clickable. That's a convention in the skill,
not new plumbing.

### 3.3 A `SpriteSmith` persona

The chain spans four MCP servers, so it needs a persona that knows the order and
the failure modes. Following the existing pattern exactly:

- `Persona::SpriteSmith` in `src/local/agent_skills.rs`
- `SYSTEM_PROMPT_SPRITE.md`
- `agent-skills/orx-sprite/SKILL.md` — the stage order, the contract, and the
  hard-won rules: **don't fight the diffusion prior, fix it downstream**;
  T-pose before meshing, always; pin frame 0 **and** last for loops; render
  natively at target size, never downscale; fixed palette every frame; no
  dithering on a loop
- `personaMeta.tsx` entry for the UI

### 3.4 Status cards

Two more cards in the existing Generative AI tab beside Scenario / Blender /
ComfyUI. Same three-layer status, same "install this" remediation text. UniRig
gets a link to its container health; Kimodo shows which checkpoint is loaded **and
its licence tier**, since that's a shipping gate.

---

## 4. Build order

Each slice ends somewhere useful, so we can stop at any point without waste.

1. **`sprite-mcp` first.** No GPU, no downloads, and it makes the contract
   checkable immediately. It also re-validates the one part already proven.
2. **Stage 1 in ComfyUI** — download klein, build the T-pose workflow with the
   side-by-side `[hero | reference]` composite. **Gate: does `warrior` come back
   as a recognisable T-posed warrior?** If identity dies here, nothing downstream
   matters.
3. **Stage 2 in ComfyUI** — Hunyuan3D models, image→mesh. **Gate: is the mesh
   riggable?** Inspect in Blender before building the rig harness.
4. **`unirig-mcp`** — container + harness + status card.
5. **`kimodo-mcp`** — server + harness + the loop tool. Verify the loop is
   seamless *before* wiring it in: frame 0 and frame N should be identical.
6. **Blender render stage** — extend the existing `blender-mcp` usage with the
   ortho/toon/no-AA settings. Mostly already proven.
7. **`SpriteSmith` persona + status cards** — makes it drivable from chat.
8. **The `warrior` spike end to end**, then the gate: side by side against the
   shipped sprite. **If it doesn't beat the band-shift, stop.**
9. The other 24, batched, sharing one Kimodo loop.

## 5. Open questions this plan does not settle

1. **Pixel art is the wrong input for image-to-3D.** Hunyuan3D is trained on
   smooth imagery; `raw-battle.png` is chunky pixel art with hard edges. Mitigation
   is that stage 1 repaints anyway — so the T-pose prompt should ask for a
   **smooth stylised render, not pixel art**, de-pixelating in and re-pixelating
   at stage 6. Untested, and it gives identity more room to drift.
2. **Three proportion systems in one chain** — hero art ~4.5–5 heads, SMPL-X ~7.5,
   map sprite ~3. Retargeting realistic motion onto squat proportions causes foot
   sliding; whether that survives at 96 px is unknown.
3. **Whether Kimodo's SMPL-X skeleton retargets cleanly onto a UniRig skeleton.**
   woid does Kimodo-on-UniRig, so the path exists; the wrist-rotation step suggests
   rest-pose mismatches are the norm and need per-rig fixes.
4. **`.glb` preview in the file browser** — deferred.

## 6. Non-goals

- Modifying `<game>/tools/bakery/` — unchanged boundary
- Cloud services of any kind (the point of this plan is local)
- Replacing ComfyUI's own UI — crux links to it, as it does today
- Auto-routing or fallback between local and cloud backends
