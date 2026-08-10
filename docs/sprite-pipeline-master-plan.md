# Sprite pipeline — master plan

> **Status, 9 Aug 2026: all six stages built and working end to end, locally.**
> See **[sprite-pipeline-as-built.md](./sprite-pipeline-as-built.md)** for the
> as-built record — exact graphs, settings, gotchas and the open defect.
> Output is not shippable yet: **texturing** is the dominant remaining problem.

**Date: 8 August 2026.** The single ordered plan. The other docs are references:

- [sprite-animation-research.md](./sprite-animation-research.md) — why diffusion-only is out (evidence)
- [local-3d-capability.md](./local-3d-capability.md) — what this machine runs, and why things break
- [image-to-3d-candidates.md](./image-to-3d-candidates.md) — stage-2 model shortlist
- [sprite-pipeline-plan.md](./sprite-pipeline-plan.md) — the pipeline, derived from `woid`
- [sprite-pipeline-build.md](./sprite-pipeline-build.md) — harness/UX design detail

**Goal.** In crux: hand a class's art to a chat, watch the chain run as visible
tool calls, browse the artifacts in the Files tab, end with real walk frames the
game can consume.

**Boundaries.** Everything runs local. `TacticaArena/tools/bakery/` is never
modified. The game gains only consumption code.

---

## The chain

```
raw-battle.png ─1─► tpose.png ─2─► model.glb ─3─► rig.glb ─4─► animated
                                                              ─5─► frames/*.png
                                                              ─6─► out/*.png ─► <class>.ts
```

| Stage | Runs on | Status |
|---|---|---|
| 1 T-pose | ComfyUI + FLUX.2-klein-4B | to build |
| 2 Mesh | Docker (Pixal3D) or ComfyUI (Hunyuan3D) | **bake-off** |
| 3 Rig | Docker UniRig + `unirig-mcp` | to build |
| 4 Motion | Kimodo + `kimodo-mcp` | to build |
| 5 Render | Blender 5.2 + existing `blender-mcp` | **proven** |
| 6 Contract | `pixel.mjs` + `spritekit` CLI | **done** |

**Already proven:** stages 5–6 produced six contract-passing frames on the first
attempt — 96×96, 8–13 colours against a 20 limit, figure 80–82px, exactly 3 empty
rows under the feet, binary alpha, byte-identical on rerun.

---

## Milestone 0 — decisions that block authoring

**0a. The 48 ↔ 96 drift — likely already answered by the code.**
`map48.mjs:129-133` says of `LOD = 48`: *"Far-zoom LOD only… **Never the shipping
tier** — it is derived from the 96 art by an exact ÷2 so the two tiers cannot
drift apart."* So **96 is the shipping map tier**, 48 is a derived far-zoom level,
and the committed modules saying `map: { w: 48, h: 48 }` are simply stale —
they predate the move to 96. All 25 staged sprites measure 96×96.
**Remaining:** confirm this reading, and decide whether the stale modules get
regenerated as part of this work or separately.

**0b. Frame count.** 2 frames is off-spec for an 82px figure (reference says 8),
but over-animating is a documented beginner error — *"a 12-frame walk cycle often
looks worse than a 4-frame one."* **Test 4 first.**
**Note:** this is a sheet-geometry change — the sheet is 4 columns per facing and
`poses.test.ts` asserts `sheetW === map.w * columns`.
**Done when:** frame count chosen and the sheet-geometry implication is accepted.

*These are yours to decide, not mine. Everything below assumes 96×96 and 4 frames
until told otherwise.*

---

## Milestone 1 — `spritekit` CLI — **DONE**

**Not an MCP server.** The first draft planned one; that was cargo-culting
`blender-mcp` / `comfyui-mcp` without asking why *those* exist. They wrap
**long-running stateful services** — they hold VRAM, they die and need restarting,
"is it up?" is a real question worth a status card. `spritekit` reads a PNG and
writes a PNG. It has no lifecycle. The rule for the rest of this plan:

> **MCP harness** = stateful service with a lifecycle.
> **Plain CLI** = stateless transform.

**Shipped:** `sprite-tools/spritekit.mjs`. Zero dependencies; imports the target
repo's `pixel.mjs` unmodified by absolute path, so "passes spritekit" and "passes
the game's importer" cannot drift. `check` and `fit`, both with `--json`; exit
0 pass / 1 fail / 2 error.

**Verified:** 25/25 shipped classes PASS. A raw 384×384 Blender render fails all
six rules with actionable detail. `fit` converts those renders into
contract-passing 96×96 sprites.

**Two bugs caught by testing against shipped art first** — both would have made
the checker reject valid work, which is its worst failure mode:

- `GROUND_ROWS = 3` is a **floor**, not an exact value (`planFit` derives
  `maxH = frame - baselinePad`); the roster actually runs **5–6** rows below the
  feet. Likewise `FIGURE_H = 82` is a **ceiling** — integer-factor snapping puts
  the roster at **79–82**.
- The 20-colour cap is a **target, not a limit**: `selectMapPalette` takes it as a
  parameter and `irisTargets` can force extras (`meta.mapAddedColors`). `thief`
  ships **21**. The check now uses each class's own palette size.

**Remaining:** a short `agent-skills/orx-sprite/SKILL.md` teaching the command.

---

## Milestone 2 — Stage 1, T-pose, in ComfyUI — **DONE**

**Result:** gate passed. Visual record in
`~/Documents/tacticaarena-sprite-research/` (`01`–`05`).

**Stage 1 is two steps, not one** — this was the plan's biggest miss. A
class-agnostic prompt collapses identity outright (pale skin, no tunic, sandals at
the same seed). A hand-written one works but is 25 hand-tuned prompts, which is
not a pipeline. A **vision-derived caption** beats both:

- **1a caption** — a vision model reads the source sprite into a fixed schema
  (skin / hair / face / torso / shoulders / straps / belt / arms / legs / feet,
  each slot carrying an explicit colour). **crux needs no new model for this: the
  agent is already the vision model.**
- **1b generate** — one fixed, class-agnostic template. The only other per-class
  input is `classDef.weapon`, read from the game's own metadata. Nothing authored
  per class.

**Models installed** (`~/ComfyUI/models/`): `flux-2-klein-4b-fp8` (4.07 GB,
Apache-2.0), `qwen_3_4b` text encoder, `flux2-vae`. Graph:
`LoadImage → FluxKontextImageScale → VAEEncode → ReferenceLatent →
FluxGuidance(5.0) → BasicGuider → SamplerCustomAdvanced(Flux2Scheduler, 28 steps)`.
Reuses woid's probed CFG of 5.0 and its prompt phrasing, which threads three
documented traps (identity-transfer wording → black images; "reference sheet" →
turnaround grids; no "no armor" → reference armor leaking).

**Ceiling found:** boot cuffs stayed olive green through an explicit *"the cuffs
are brown, NOT green and NOT olive"*. Third time this session a diffusion prior
beat an explicit negation. **It doesn't matter** — the contract chain ends in
`remapToPalette`, which snaps every pixel into the class's own 20 colours, so
colour drift is self-correcting. **Silhouette drift is not.** Optimise captions
for silhouette (sleeveless vs sleeved, one shoulder vs two, trousers vs bare),
not colour.

**Carried forward:** palms are not down — woid's documented bone-level fix after
rigging, and likely moot since a hand is ~2px at 96×96.

## Milestone 3 — Stage 2, mesh — **baseline done, bake-off open**

**Hunyuan3D v2, build-free, worked first try.** `hunyuan3d-dit-v2-0-fp16`
(4.93 GB) into `models/checkpoints/`; graph is
`ImageOnlyCheckpointLoader → CLIPVisionEncode(crop=none) → Hunyuan3Dv2Conditioning
→ KSampler(50 steps, cfg 5.5) → VAEDecodeHunyuan3D(octree 320) → VoxelToMesh
(surface net) → SaveGLB`. `crop=none` matters: a T-pose is as wide as it is tall
and a centre crop would remove the hands.

**Background removal needs a flood fill, not a colour key.** `keyBackground`
destroyed the figure — the cream fur sits within tolerance of the off-white panel.
A **border-seeded flood fill** removes only *connected* background and preserves
interior light tones.

**Passed:** limbs cleanly separated, hands open and empty, T-pose preserved
(span 1.94 vs height 1.93), real depth from a single view. The literature's
failure modes — fused limbs, weapon fused to hand — **did not occur, because
stage 1 removed both conditions before meshing.**

**Failed as predicted:** 365,418 faces, **100% triangles, zero quads**, 15,993
non-manifold edges. ~12× the poly count auto-riggers are tested against, so
**decimation is required**. Whether that topology actually breaks rigging is
unresolved and only UniRig answers it.

**Bake-off run. Hunyuan3D wins — stage 2 is settled.**

| | Hunyuan3D | Pixal3D |
|---|---|---|
| Build | **build-free** | 8 blockers, 23.6 GB container |
| Faces | 365k | 994k |
| Quads | 0 | **0** — no topology advantage |
| Open edges | 154 | **493,267** |
| Mesh depth | **0.64** (human) | 1.56 (2.4× too deep) |
| Rigged | 52 bones, 100% weights | 52 bones, 100% weights |

Pixal3D's headline feature — O-Voxel support for **open surfaces and non-manifold
geometry** — is actively harmful here. It reconstructed the **drop shadow as a
flat plane** and blew the **fur pauldron into open shells**; Blender flags the
result "not a valid mesh". A fidelity win that is a usability loss. For a
character that must be a watertight riggable solid and then a clean 96×96
silhouette, Hunyuan3D's closed marching-cubes output is the right shape.

**Judging on rigging alone would have called it a tie** — both gave 52 bones at
100% weight coverage. Mesh geometry separates them, and silhouette is what
survives the palette remap.

**Container is kept anyway.** `pixal3d:local` proves every ABI-dead package
(`cumesh`, `o_voxel`, `flex_gemm`, `nvdiffrast`, `natten`) compiles cleanly once
the torch versions agree. That technique transfers to anything else on this box
that hits the torch-2.13 wall.

## Milestone 4 — Stage 3, rigging — **DONE** (ahead of schedule)

`unirig:local` was **already built and running** on `:8081` (8 days uptime), so no
harness was needed to answer the question. `POST /rig` multipart, 84 s on a 44k-tri
decimation of the Hunyuan3D mesh:

- **52 bones**, single root, max chain depth 10
- **Skeleton spans 97.7% of mesh width** — bones reach the hands
- **100% of vertices weighted**, 52 vertex groups, armature modifier bound
- Spine hips→head; shoulder/elbow/wrist with individuated finger bones; hip/knee/ankle

**So polygon soup did not break rigging** — the central open question. The
literature says triangle-soup meshes "cannot be rigged or animated properly"; this
one was. The cause is upstream: stage 1's T-pose with empty hands removed the two
conditions that actually cause rig failure (dynamic pose, weapon fused to hand).

**Predicted defect present:** foot bones project past the mesh — first on UniRig's
documented failure list. Most benign of them, and fixable with a bone-level
correction after rigging, as woid did for palms.

**Deformation verified**: a hand-keyed walk cycle renders with no tearing at
shoulders, hips or knees (`09-rigged-character.mp4`).

**Note:** rigging is a service, not a harness — `unirig-mcp` is unnecessary. crux
can `POST` to `:8081` directly. Same reasoning that removed `sprite-mcp`.

## Milestone 2 (original text) — Stage 1, T-pose, in ComfyUI

- Download **FLUX.2-klein-4B** (Apache-2.0, ~13 GB)
- Build the T-pose workflow: side-by-side `[hero | reference]` composite, per
  woid's finding that composite input is what makes portrait→full-body work
- **Prompt for a smooth stylised render, not pixel art** — de-pixelate in,
  re-pixelate at stage 6, because image-to-3D models are trained on smooth imagery
- One shared T-pose reference image, authored once

**GATE:** does `warrior` come back as a recognisable T-posed warrior — same face,
axe, kilt, fur pauldron? **If identity dies here, nothing downstream matters.**

**Risk:** high, and it's the first real test. woid's input was a smooth portrait;
ours is chunky pixel art, so stage 1 is doing more work than it does for them.
Their documented palms-forward lesson applies: if a detail won't come out of the
diffusion prior, fix it downstream, don't fight it.

---

## Milestone 3 — Stage 2, the mesh bake-off

Judged on **"does UniRig produce a clean rig from it?"** — not on how it looks.
Sub-criteria: limbs separated not fused; axe a separate part or cleanly cuttable.

**3a. Hunyuan3D v2 (baseline).** kijai wrapper, pure-Python core, build-free on
torch 2.13. Needs DiT + CLIP-vision (~3–5 GB). If it ties, it wins on simplicity.

**3b. Pixal3D (challenger).** MIT licence, SIGGRAPH 2026, newest. **Docker
required** — needs the TRELLIS.2 base plus `natten` (no cu130/torch2.13 wheel;
source build needs CUDA 13.0 we don't have).

Container target is now concrete: TRELLIS.2 specifies **PyTorch 2.6.0 + CUDA
12.4** and ≥24 GB VRAM. Base `pytorch/pytorch:2.6.0-cuda12.4-cudnn9-devel`, which
has `nvcc` so `natten` compiles inside. Skip `--nvdiffrast` / `--nvdiffrec` — we
need geometry, not texture baking or rendering. Set `ATTN_BACKEND=sdpa` to drop
the `flash_attn` build entirely. Then `natten==0.21.0`, the `utils3d` wheel, and
`inference.py --image --output .glb`.

Our 3090 is 24 GB — exactly TRELLIS.2's stated minimum, so expect to need
low-VRAM mode (1024 rather than 1536).

**Deferred:** TRELLIS.2 direct, until its **missing LICENSE file** is resolved.

**Done when:** one mesh per candidate from the same T-pose, inspected in Blender,
with a recorded verdict on rig-ability.

**Risk:** medium-high. Pixel-art-derived input may reconstruct as lumpy geometry —
mitigated by milestone 2 emitting smooth art. The Docker build is a half-day.

---

## Milestone 4 — Stage 3, `unirig-mcp`

- UniRig container on `:8081`, as woid runs it: multipart `POST /rig`,
  `file=model.glb` → rigged GLB. A container is immune to the torch 2.13 break
- `src/local/unirig/{mod,server}.rs`, three-layer status
  (`mcp_found` / `mcp_runnable` / `service_reachable`) because "not installed" and
  "container down" need different remediation text
- Tools: `rig_mesh(glb)`, `inspect_rig(glb)`

**Done when:** a mesh from milestone 3 comes back rigged, and `inspect_rig` names
the bones.

**Risk:** medium. Expect ~1 in 4 to need manual repair; watch for the fused-limb
case, which is unrecoverable automatically. woid's wrist-rotation step is a
precedent that **rest-pose fixes belong at the bone level**, applied after rigging
— we likely skip it (a hand is ~2px at 96×96) but the pattern stands.

---

## Milestone 5 — Stage 4, `kimodo-mcp`

- Local Kimodo, `TEXT_ENCODER_DEVICE=cpu` (<3 GB VRAM, tested on RTX 3090).
  `kimodo-zerogpu-space` already has a working venv to copy
- `src/local/kimodo/{mod,server}.rs`, status includes **which checkpoint and its
  licence tier** — R&D variants are non-commercial and that's a shipping gate
- Tools: `generate_loop(prompt, seconds, pose)` pinning a fullbody constraint at
  **frame 0 and the last frame to the same pose** — your measured 0 cm / 0 m-drift
  pattern; plus `retarget(npz, rig_glb)`

**GATE:** verify frame 0 and frame N are identical **before** wiring it in. A loop
that doesn't loop is worthless.

**Risk:** medium. The real unknown is **retargeting realistic SMPL-X proportions
(~7.5 heads) onto our squat characters** — a documented source of foot sliding.
Whether it survives 96×96 is unknown. **One walk loop is shared by all 25** — that
is the economics of the entire project.

---

## Milestone 6 — Stage 5, render settings

Extend existing `blender-mcp` usage. The settings that separate pixel art from a
shrunk 3D render:

- Orthographic, fixed, snapped to the pixel grid
- **Toon shader: colour ramp, exactly N stops, interpolation = Constant** — this
  is *the* difference
- Kill AA: Cycles **filter width 0.01** (box filter alone is not enough); EEVEE
  disable TAA/jitter
- **Render natively at 96×96** — never render large and downscale
- Fixed 20-colour palette applied identically to every frame; never per-frame
  adaptive, or the palette crawls
- **No dithering** on a loop at 20 colours — it twinkles
- Front and left only; the game derives up and mirrors right
- Companion normal map per frame, in register

**Risk:** pixel crawl and silhouette shimmer. Motion Twin never solved it and
hand-cleaned; at 82px a one-pixel edge wobble is ~1.2% of the figure. **Budget
cleanup time.**

---

## Milestone 7 — crux integration

- **Artifacts to the project files dir**, `sprites/<class>/`, numbered by stage
  (`00-source.png` … `07-<class>.ts`). `FilesTab` lists them, `FileViewer` renders
  PNGs, `filesDir` is already injected so chat can link them clickably.
  *Known gap:* `.glb`/`.npz` list and download but don't preview — deferred
- **Every tool returns a files-dir-relative path** so narration links to something
  clickable. Convention, not plumbing
- **`Persona::SpriteSmith`** + `SYSTEM_PROMPT_SPRITE.md` +
  `agent-skills/orx-sprite/SKILL.md` + `personaMeta.tsx`, carrying the rules:
  don't fight the diffusion prior; T-pose before meshing; pin both ends for loops;
  render natively; fixed palette; no dithering
- **Status cards** for UniRig and Kimodo in the Generative AI tab, same three-layer
  shape as Blender/ComfyUI

**Done when:** the whole chain is drivable from one chat with visible progress.

---

## Milestone 8 — the `warrior` spike

Run stages 1–6 end to end on one class.

**THE GATE:** put the output beside the shipped band-shift sprite. **If it doesn't
beat it, stop.** That rule has survived four dead ends and I'd hold to it.

---

## Milestone 9 — the other 24

Batch, sharing one Kimodo loop. `woid`'s `generate_character.py` is already
designed as a CLI orchestrator with per-stage artifact persistence and
resume-on-failure — this is a variant with a different tail.

**Verification:** `npm test` (~629 cases), `npm run build`, and the **20 untouched
classes byte-identical**. Then `dev/sprites.html`, then watch a unit walk.

---

## Milestone 10 — consumption in TacticaArena (3 files)

`bakedtypes.ts` gains an optional `walk`; `bakedmap.ts` fills walk columns from
real frames when present and band-shifts when absent; `baked/<class>.ts` is the
data. `tools/bakery/` untouched, `src/core/` frozen.

---

## Honest summary of risk

The infrastructure is largely de-risked — you've run this chain before in `woid`,
and stages 5–6 are already proven here. **The live risks are style and
proportion**, in this order:

1. **Stage 1 identity** (milestone 2) — the first gate, and the most likely to fail
2. **Pixel-art input → lumpy mesh** (milestone 3) — mitigated by emitting smooth
   art at stage 1, untested
3. **SMPL-X proportions → squat characters** (milestone 5) — foot sliding, unknown
   at 96px
4. **Pixel crawl** (milestone 6) — known unsolved industry-wide; budget cleanup
5. **Licensing** — Kimodo checkpoint tier, TRELLIS.2's missing LICENSE

Milestones 0 and 1 are risk-free and can start immediately. Milestone 2 is the
first real test, and I'd want to stop there and look before going further.
