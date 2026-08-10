# Sprite pipeline — as built

**Date: 9 August 2026.** What actually got built and run, end to end, on one
character (`warrior`). Written so it can be re-run or picked up cold.

Companions: [master plan](./sprite-pipeline-master-plan.md) (sequence),
[research](./sprite-animation-research.md) (why diffusion-only is out),
[local capability](./local-3d-capability.md) (what this machine runs),
[candidates](./image-to-3d-candidates.md) (stage-2 shortlist).
Visual record: `~/Documents/tacticaarena-sprite-research/`.

---

## Status

**All six stages work end to end, entirely local.** Hero art → T-pose → mesh →
rig → looping motion → contract-passing 96×96 sprites.

**The output is not shippable yet.** Against the roster it is visibly rougher:
mushy face, noisy silhouette, and white blobs on the legs. The dominant defect is
**texturing**, diagnosed but not fixed — see §8.

| Stage | Tool | State |
|---|---|---|
| 1 T-pose | ComfyUI + FLUX.2-klein-4B | works |
| 2 Mesh | ComfyUI + Hunyuan3D v2 | works |
| 3 Rig | UniRig, Docker `:8081` | works |
| 4 Motion | Kimodo, CPU | works |
| 5 Retarget | Node + kimodo's own `animator.js` | works |
| 6 Render | Blender 5.2 + `spritekit` | works; texturing weak |

---

## 1. Stage 1 — T-pose

**Two steps, not one.** This was the plan's biggest miss.

**1a — caption.** A vision model reads the source sprite into a fixed schema:
skin / hair / face / torso / shoulders / straps / belt / arms / legs / feet, each
slot carrying an explicit colour. **crux needs no new model: the agent is the
vision model.**

**1b — generate.** One fixed, class-agnostic template. Only two per-class inputs,
both derived: the caption, and `classDef.weapon` from the game's own metadata.
Nothing is authored per class.

Measured at one seed, changing only the identity clause:

| Identity clause | Result |
|---|---|
| Class-agnostic | **Identity collapses** — pale skin, no tunic, sandals |
| Hand-written | Decent; loses chest strap, blue knot, bracers, trousers. Does not scale to 25 |
| **Vision-derived caption** | **Best** — recovers all of the above, and fur on the *right* shoulder only |

**Models** (`~/ComfyUI/models/`): `flux-2-klein-4b-fp8.safetensors` (4.07 GB,
Apache-2.0), `qwen_3_4b.safetensors` text encoder, `flux2-vae.safetensors`.

**Graph:** `LoadImage → FluxKontextImageScale → VAEEncode → ReferenceLatent →
FluxGuidance(5.0) → BasicGuider → SamplerCustomAdvanced(Flux2Scheduler, 28 steps)`

**Input:** side-by-side composite `[hero | T-pose reference]`, 768×1024 panels on
background `(245,240,230)`, built with woid's `scripts/tpose-composite.py`. The
hero's magenta chroma must be keyed and binarised first or it bleeds.

**Prompt:** woid's verbatim, which threads three documented traps —
identity-transfer wording returns black images; "reference sheet" produces
turnaround grids; without explicit "no armor/weapons" the reference's kit leaks.
Two deliberate departures: ask for a **smooth stylised render, not pixel art**
(image-to-3D is trained on smooth imagery), and realistic 7–8 head proportions.

**"No weapons" is load-bearing.** It removes the axe, which is what prevents a
weapon fused to the hand downstream — one of the three documented causes of
unriggable meshes.

**Ceiling found:** boot cuffs stayed olive through an explicit *"the cuffs are
brown, NOT green and NOT olive"*. Third time a diffusion prior beat an explicit
negation. **It does not matter** — the contract chain ends in `remapToPalette`,
so colour drift is self-correcting. **Silhouette drift is not.** Optimise captions
for silhouette (sleeveless vs sleeved, one shoulder vs two), not colour.

## 2. Stage 2 — Mesh

`hunyuan3d-dit-v2-0-fp16.safetensors` (4.93 GB) in `models/checkpoints/`.

```
ImageOnlyCheckpointLoader → CLIPVisionEncode(crop=none) → Hunyuan3Dv2Conditioning
  → KSampler(50 steps, cfg 5.5) → VAEDecodeHunyuan3D(octree 320)
  → VoxelToMesh(surface net, 0.6) → SaveGLB
```

**`crop=none` matters:** a T-pose is as wide as it is tall, and a centre crop
removes the hands — the geometry rigging needs most.

**Background removal needs a flood fill, not a colour key.** `keyBackground`
destroyed the figure: the cream fur sits within tolerance of the off-white panel.
A **border-seeded flood fill** removes only *connected* background and preserves
interior light tones.

**Result:** 365,418 faces, 100% triangles, 154 open edges, limbs cleanly
separated, hands empty, T-pose preserved (span 1.94 vs height 1.93).

**Bake-off vs Pixal3D:** Pixal3D reconstructed the **drop shadow as a flat plane**
and the **fur as open shells** (493,267 open edges; Blender: "not a valid mesh").
Its O-Voxel open-surface support is actively harmful for a character that must be
a watertight riggable solid. **Reopened by the owner** on aesthetics — Pixal3D's
clothing detail is genuinely crisper, and it ships real UVs and textures, which is
exactly what §8 is missing.

## 3. Stage 3 — Rig

UniRig already ran as a container on `:8081` (`POST /rig`, multipart `file=`).

- Decimate to ~30–44k tris first (365k is ~12× what auto-riggers are tested on)
- **84 s → 52 bones**, single root, chain depth 10
- **Skeleton spans 97.7% of mesh width**, **100% of vertices weighted**
- `kimodo/web/scripts/unirig_mapping.py` derives the 22-joint SMPL-X mapping from
  the GLB's skin graph — UniRig bones are anonymous (`bone_0`…), so this is required

**Polygon soup did not break rigging.** The literature says triangle-soup meshes
"cannot be rigged or animated properly"; this one was. The cause is upstream —
stage 1's T-pose with empty hands removed the two conditions that actually cause
rig failure.

**Known defect:** foot bones project past the mesh — first on UniRig's documented
failure list, and the most benign at 96×96.

## 4. Stage 4 — Motion

Kimodo runs **on CPU**: 25 s to load, ~19 s for 37 frames at 50 diffusion steps.
No GPU. Venv is `kimodo-zerogpu-space/.venv` plus `einops peft transformers==5.1.0
accelerate omegaconf hydra-core bvhio sentencepiece protobuf`.

Two passes, because a natural loop is not clean enough:

| | Seam (root-relative, frame 0 vs N) | vs a normal frame step |
|---|---|---|
| Best natural period, unconstrained | 2.36 cm | **2.97×** — a visible pop |
| `FullBodyConstraintSet` pinned frame 0 **and** N | **0.283 cm** | **0.33×** — invisible |

The pose pinned at frame N is frame 0's pose **translated forward by one stride**,
so the clip still travels; the render treadmills it back.

## 5. Stage 5 — Retarget — *the hard-won one*

**Do not port the retarget. Run kimodo's own `animator.js` headlessly.**

```
Node   bake_headless.mjs   kimodo's Animator on the real GLB → bone quaternions
Python inject_animation     write those into the GLB as a glTF animation
Blender import              a finished animation; render only
```

`animator.js` has **zero DOM references**; THREE, GLTFLoader are in
`kimodo/web/node_modules`. Frames are stepped deterministically by setting
`animator.elapsed` rather than using the wall clock.

**Why porting fails.** Blender **rebuilds glTF skeletons**: its bones always have
+Y running head→tail, while glTF joints are nodes with arbitrary orientation.
Measured: **0/22 rest frames match, max 168.69° apart**, and no `bone_heuristic`
setting fixes it (`FORTUNE` included) because it is structural. THREE has exactly
one skeleton — it keeps the nodes verbatim and uses the file's inverse-bind
matrices — so `animator.js`'s formula is calibrated to that definition of "rest".
Any port has two skeletons to reconcile. Baking to a glTF animation avoids the
problem entirely, because Blender's importer already maps glTF node animation onto
its own bones correctly, as it does for every animated GLB.

**Gotchas:** `GLTFExporter` needs browser APIs (`FileReader`) and stalls under
Node even with shims — writing the glTF animation by hand is more reliable.
UniRig's GLB contains a **stray `Icosphere` (42 verts, no weights)** as a second
mesh. Dropping the pelvis **location** channel is what makes the clip walk in
place, which is what a sprite needs.

## 6. Stage 6 — Render

```python
filter_size          = 0.01     # box filter alone is not enough
taa_render_samples   = 1        # any sub-pixel jitter softens hard edges
film_transparent     = True     # real alpha; no chroma key at all
resolution           = 96×96    # NATIVE. never render large and downscale
dither_intensity     = 0.0      # dithering twinkles across a loop
view_transform       = Standard # Filmic pulls colours off-palette
```

Shading is a **Constant-interpolation colour ramp** on the diffuse factor — flat
bands, not gradients. Per the research this is the single biggest difference
between "pixel art" and "a small 3D render".

**Straight out of Blender the renders already had binary alpha**, the figure at
80–81px, and ≥3 ground rows. Only colour failed, which is `remapToPalette`'s job.

## 7. `spritekit`

`crux/sprite-tools/spritekit.mjs` — zero dependencies, imports the target repo's
`pixel.mjs` unmodified so "passes spritekit" and "passes the importer" cannot
drift. `check` and `fit`, both `--json`, exit 0/1/2.

**Verified: 25/25 shipped classes pass**, and re-verified after every change.

Two rules learned from the shipped art, both counter-intuitive:

- **Targets are bounds, not equalities.** `GROUND_ROWS = 3` is a *floor* (roster
  runs 5–6); `FIGURE_H = 82` is a *ceiling* (roster runs 79–82). Demanding
  equality rejects art the game itself ships.
- **The colour cap is per class.** 20 is a target that forced iris colours can
  exceed — `thief` ships 21.

`fit` **converges rather than assumes**: `planFit` snaps to integer downscale
factors and can overshoot, so it measures the result and steps the request down
until the figure lands in range. Without this, 3 of 8 frames came out 83–85px.

## 8. The open defect — texturing

**Root cause: only half of Hunyuan3D was run.** It has two models — **shape**
(`hunyuan3d-dit`) and **paint**. The four native ComfyUI nodes cover shape only;
`VAEDecodeHunyuan3D → VoxelToMesh` emits geometry with **no materials, no
textures, no UVs**. woid's meshes are textured because their pipeline calls
Hunyuan3D on Cloud Run with **`texture: true`**, which runs both halves.

Measured, same question asked of every mesh:

| Mesh | materials | textures | images | UVs |
|---|---|---|---|---|
| woid's Trellis + Hunyuan3D characters | 1 | 1 | 1 | yes |
| **Ours (ComfyUI native Hunyuan3D)** | 0 | **0** | **0** | **no** |
| Pixal3D | 1 | 2 | 2 | yes |

Painting the untextured mesh by planar projection was the wrong fix and produced
the white leg blobs: where the mesh's legs don't coincide with the T-pose image's
legs, they sample the image's off-white **background**.

**Pixal3D cannot rescue this.** It ships real textures and UVs, and its turntable
genuinely looks better — but its mesh is **6,523 disconnected shells** (largest
145 verts of 42,915). Thousands of separate surfaces read as a body from outside;
there is no coherent solid underneath. At 96×96 the sprite is a giant fragment
plus sheets of the reconstructed drop shadow. This settles the reopened stage-2
question on much stronger evidence than open-edge counts.

**The fix is the paint model.** Locally that means kijai's
`ComfyUI-Hunyuan3DWrapper` texture path, whose `custom_rasterizer` ships
Windows-only wheels — the same class of problem the Pixal3D container already
solved (`cumesh`, `o_voxel`, `flex_gemm`, `nvdiffrast`, `natten` all compiled once
the torch versions agreed).

### Superseded notes

Hunyuan3D's voxel→mesh path produces **untextured geometry**. Colour came from
projecting the T-pose image planar from the front, and that is the weak link:

- **White blobs on the legs** — where the mesh's legs don't coincide with the
  image's legs, they sample the image's off-white *background*
- **Mushy face**, **noisy silhouette**

The fix in progress when work stopped: **vertex colours** rather than planar UVs —
sample only front-facing vertices where the image is opaque, then propagate colour
across mesh edges to everything unsampled. No background can leak in and there are
no seams. Alternatives: proper UV unwrap + bake, or **use Pixal3D's mesh, which
ships real UVs and textures** — which is also how to settle the reopened stage-2
question honestly.

## 9. Open decisions

- **0a — 48↔96.** Answered by the code: `map48.mjs:129-133` says `LOD = 48` is
  *"far-zoom LOD only… never the shipping tier — derived from the 96 art by an
  exact ÷2"*. **96 ships**; committed modules saying `48` are stale.
- **0b — frame count.** 4 frames were rendered and animate; 2 is off-spec for an
  82px figure, 8 is the textbook answer. Changing it is a sheet-geometry change
  (`poses.test.ts` asserts `sheetW === map.w * columns`).
- **Kimodo licence.** Checkpoints split "NVIDIA Open Model" vs "NVIDIA R&D Model";
  **R&D variants are non-commercial**. Confirm before shipping.
- **Stage 2 choice** — reopened on aesthetics; §8 is the deciding test.

## 10. Process notes

Recorded because they cost real time.

- **Six failed attempts to port the retarget** before running the working code
  instead. Two hypotheses were asserted with high confidence and both were
  disproved by measurement — the rest-frame mismatch *cancels out of the algebra*,
  and a readback showed the pose application was exact (0.00° error). The bug was
  never in the layer I was debugging.
- **Research was run after four implementation attempts, not before.** Each
  attempt was justified from mechanism rather than from evidence that the approach
  had ever worked.
- **Verify against ground truth first.** Running `spritekit` over the shipped
  roster caught two wrong contract rules immediately.
- **Check which process serves a port.** 5174 was assumed to be the Kimodo viewer;
  it was `gambit-arena-mobile`.
- **Piping build output through `tail` masks exit codes** — a failing `setup.sh`
  looked successful. And `docker build` cannot satisfy a GPU probe; install in a
  running `--gpus` container and commit.
