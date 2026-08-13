# Walk cycles for the target game's map sprites — plan

**Date: 8 August 2026.** Supersedes the first draft of this file, which planned a
from-scratch hand-modelled pipeline in ignorance of `woid`.

Built on [sprite-animation-research.md](./sprite-animation-research.md) (why
diffusion-only is out) and [local-3d-capability.md](./local-3d-capability.md)
(what this machine runs).

**Boundary, set by the owner:** generation lives in crux.
`<game>/tools/bakery/` is not modified. The game may gain code to *consume*
real frames.

---

## The headline

**The pipeline already exists**, in `woid/docs/design/e2e-character-pipeline.md`,
and it is a direct answer to every failure this session hit. It was built for
generating rigged game characters from a prompt; the target game needs the same
thing, stopping at a different output format.

| What failed here | What woid already does |
|---|---|
| Character identity drifted every generation | FLUX.1-Kontext with a **side-by-side `[avatar \| reference]` composite** — identity transfer documented as solid. Plus a trained character LoRA (`polats/weiner-klein-lora`, FLUX.2-klein-4B) |
| Dynamic weapon-holding pose → unriggable mesh | An explicit **T-pose generation step** before meshing — the exact "T-pose unlock" the research independently identified |
| No pose control; diffusion ignored every instruction | **Kimodo** with kinematic constraints, already deployed and characterised |
| Blender proxy was a box mannequin | **Trellis / Hunyuan3D** from the T-pose → a real character mesh |
| No rig | **UniRig as local Docker on `:8081`** — multipart POST, returns a rigged GLB |

### Two corrections to what I told you earlier

1. **I said Kimodo "doesn't produce loops by construction, so you don't need it."
   That was wrong**, and your own `pose-control-and-constraints.md` disproves it.
   In-betweening — pinning a full-body constraint at **frame 0 and the last
   frame** — is measured at **0 cm at both ends, 0 m root drift**. Pin the *same*
   pose at both ends and you have a seamless looping walk cycle by construction.
   That is exactly the capability I claimed was missing.
2. **I said AI-generated meshes are effectively unriggable without manual
   retopology.** That is what the public literature says, but woid **empirically
   ships Trellis → UniRig** and it works. Your measured result outranks the
   general web finding. Trellis is noted there as *"cleaner topology, better for
   rigging… UniRig prefers manifold input"*, which is why it's the default.

### The one hard-won lesson that transfers directly

woid hit the palms-forward problem: Kontext's diffusion prior pins palms forward
regardless of prompt phrasing, reference images, or cfg up to 7.0 — and the
research doc concluded it is *"a general weakness of every diffusion model we
surveyed."* The fix was **not** to fight the prior; it was a deterministic
**bone-level rotation baked into the rest pose after rigging**.

That is precisely the lesson of this session's four failures: don't argue with the
diffusion prior, fix it downstream where the operation is exact.

---

## What changes for sprites

### Steps we skip

- **(1) persona, (2) character creation, (3) avatar** — the target game already has
  25 characters with names, designs and art.
- **(8) kimodo web registry** — we render, we don't ship a GLB viewer.
- **(7) wrist rotation** — *probably*. At 96×96 a hand is ~2 px. Verify on the
  spike; if palms don't read, skip the step entirely. This is a real
  simplification woid doesn't get to make.

### Steps we reuse essentially as-is

- **(4) T-pose.** Our input is *better* than woid's: they restructure a
  head-and-shoulders portrait into a full body, whereas each class already ships
  **`raw-battle.png` at ~896×1186 — a full-body character**. Strictly less
  restructuring, so identity should hold better.
- **(5) mesh** — Trellis preferred for rig quality; Hunyuan3D as the fallback.
- **(6) UniRig** — local Docker, unchanged.

### Steps that are new

- **Kimodo walk loop.** One clip, reused across all 25. Generate with a full-body
  constraint pinned at **frame 0 and the last frame to the same pose**; per your
  docs that yields 0 cm at both ends and no drift. 30 fps, so a ~1 s loop is 30
  frames — sample the 2, 4 or 8 the sheet needs.
- **Orthographic pixel render + contract chain.** This is the only genuinely new
  engineering, and **it is already proven here**: during experimentation the
  Blender → `pixel.mjs` chain produced six contract-passing frames on the first
  attempt (96×96, 8–13 colours against a limit of 20, figure 80–82 px, exactly 3
  empty rows under the feet, binary alpha, deterministic reruns).

---

## How it runs on *this* machine

`local-3d-capability.md` establishes the constraint: torch 2.13.0+cu130 means
anything with a pre-2.13 compiled CUDA extension is ABI-dead inside the ComfyUI
venv. woid's architecture sidesteps this **by not putting any of it in ComfyUI**:

| Stage | woid runs it as | Here |
|---|---|---|
| T-pose | FLUX.1-Kontext on Cloud Run | Cloud Run as-is, **or** build `woid` task **#435** — an unbuilt, already-specified plan for a local ComfyUI T-pose backend with a `POST /generate {prompt, ref_image_url}` shim. Our ComfyUI runs fine for image work |
| Mesh | Trellis / Hunyuan3D on Cloud Run | Cloud Run as-is, **or** Trellis in Docker with torch ≤2.12 (Docker 29.6.2 + `nvidia-ctk` are present), **or** Hunyuan3D via kijai — pure-Python core, build-free on torch 2.13 |
| Rig | **UniRig, local Docker `:8081`** | Unchanged. It's a container, not a ComfyUI node — the ABI break never touches it |
| Motion | Kimodo | Local: <3 GB VRAM with `TEXT_ENCODER_DEVICE=cpu`, tested on RTX 3090 |
| Render | — | Blender 5.2 + MCP, already working |
| Quantize | — | `pixel.mjs`, imported unmodified |

**Nothing in this pipeline needs a compiled CUDA extension inside the ComfyUI
venv.** That single architectural fact makes it viable here where everything else
this session was not.

**Licensing to check before shipping:** Kimodo checkpoints split between "NVIDIA
Open Model" and "NVIDIA R&D Model" — the R&D variants are **non-commercial**.
`Kimodo-SMPLX-RP-v1` is what karate-wiener uses; confirm its variant. TRELLIS.2
ships **no LICENSE file**. Steam has required AI-content disclosure since
2026-01-19, explicitly covering *"3D models from AI geometry tools."*

---

## Plan

### Phase 0 — settle the 48 ↔ 96 drift (blocking)

Staging is 96×96 (`map48.mjs:128 MAP = 96`); every committed module still says
`map: { w: 48, h: 48 }`, and `bakedmap.ts`'s own comment reasons *"at 48 px"*.
Decide before authoring against a scale.

### Phase 0.5 — settle the frame count (blocking)

2 frames is off-spec for an 82 px figure (reference prescribes 8), but
over-animating is a documented beginner error — *"a 12-frame walk cycle often
looks worse than a 4-frame one."* **Test 4 first.** Note this is a
sheet-geometry change: the sheet is 4 columns per facing and
`poses.test.ts` asserts `sheetW === map.w * columns`.

### Phase 1 — one-character spike on `warrior` (decision gate)

Run the existing woid pipeline end to end, once:

```
warrior/raw-battle.png (896×1186)
  → T-pose (Kontext + side-by-side composite)
  → mesh (Trellis, fallback Hunyuan3D)
  → UniRig :8081
  → Kimodo walk loop (fullbody pin, frame 0 + last, same pose)
  → Blender ortho render, native 96×96
  → pixel.mjs contract chain
```

**Gate:** put the output beside the shipped band-shift sprite. If it doesn't beat
it, stop — that stopping rule has survived four routes and I'd keep it.

### Phase 2 — the other 24, if the gate passes

Batch. `generate_character.py` is already designed as a CLI orchestrator with
per-stage artifact persistence and resume-on-failure; this is a variant of it with
a different tail. **One Kimodo walk loop is shared by all 25** — that is the
economics that make the whole thing worth doing.

### Phase 3 — render settings that distinguish pixel art from a shrunk render

- Orthographic camera, fixed, snapped to the pixel grid
- **Toon shader: colour ramp, exactly N stops, interpolation = Constant** — this
  is *the* difference between pixel art and a small 3D render
- Kill AA: Cycles **filter width 0.01** (a box filter alone is not enough); EEVEE
  disable TAA/jitter
- **Render natively at 96×96** — do not render large and downscale
- Quantize last, fixed 20-colour palette **applied identically to every frame**;
  never per-frame adaptive, or the palette crawls
- **No dithering** on a looping cycle at 20 colours — it twinkles
- Then `trim` → `planFit` → `renderFit` → `remapToPalette` → `alphaThreshold`,
  plus a companion normal map per frame in register

### Phase 4 — consumption in the game (3 files)

`bakedtypes.ts` gains an optional `walk`; `bakedmap.ts` uses real frames when
present and band-shifts when absent; `baked/<class>.ts` is the data, emitted by
crux. `tools/bakery/` untouched, `src/core/` frozen.

### Verification

`npm test` (~629 cases), `npm run build`, and the **20 untouched classes
byte-identical**. Then `dev/sprites.html`, then watch a unit walk.

---

## Risks

1. **Pixel crawl and silhouette shimmer** — Motion Twin's own unsolved problem;
   *"flickering pixels"* with no satisfactory fix without manual cleanup. At 82 px
   a one-pixel edge wobble is ~1.2% of the figure. Budget hand-cleanup.
2. **Style match.** woid's pipeline was built for 3D game characters, not for
   matching an existing hand-tuned 20-colour pixel roster. The spike tests
   exactly this, and it is the most likely failure.
3. **Service sprawl.** The chain is four services plus Blender. Cold starts in
   woid's notes run 5–10 min for Trellis. Running locally trades that for setup.
4. **Kimodo licensing** on the specific checkpoint, before anything ships.
5. **The 48/96 drift** poisons everything downstream if unsettled.

## Rejected

Diffusion-generated sprite frames; instruction-editing a sprite into a new pose;
ComfyUI-3D-Pack (dead, torch 2.5.1); in-place rebuild of Trellis deps (CUDA major
mismatch across six packages); **hand-modelling 25 characters** — superseded, the
pipeline exists.
