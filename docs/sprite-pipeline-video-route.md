# Alternative route — image-to-video sprites (the "Scenario" method), locally

**Date: 9 August 2026.** A proposal, not yet run. Companion to
[sprite-pipeline-as-built.md](./sprite-pipeline-as-built.md) (the 3D route that
works today).

## What Scenario actually does

Per [their guide](https://help.scenario.com/articles/9088582240-create-spritesheets-with-scenario):

1. Generate or upload a **base character image** (side view, mid-action)
2. **Convert to video** — image-to-video with the base image as the **first frame**
   - Seedance 1 Lite/Pro (first-frame reference), or **Pixverse v4.5 (first *and*
     last frame)**
   - Simple action prompt ("a boy running"), 5 s, 1:1, **fixed camera**
3. **Extract key poses** by hand from the video — "typical 8-frame run cycle"
4. **Pack into a spritesheet**, evenly spaced and aligned
5. Test in an engine

Their own caveats: motion is *"slow or slightly imperfect"*, so pick frames that
"clearly represent essential stages"; and looped sequences come from
**first/last-frame swapping** (neutral → attack → neutral).

## The local equivalent — we have all of it

| Scenario | Local |
|---|---|
| Base image | **The shipped sprite itself** (`map.png`) or `raw-battle.png` |
| Pixverse first+last frame | **`MiniMaxH3ImageToVideo`** — `first_frame` *and* `last_frame` inputs, model `minimax_h3_fl2va` (20 GB, installed) |
| Manual frame extraction | Sample N frames evenly from the latent/video output |
| Spritesheet packing | **`spritekit fit`** — and it does far more than pack |

`fl2va` = *first-last to video*. The one capability that makes their loop trick
work is already here.

## Why this is worth trying

**It deletes four stages.** No T-pose, no mesh, no rig, no retarget. The current
route is hero art → T-pose → mesh → rig → motion → render; this is
sprite → video → frames.

**Identity is anchored to the real sprite, not a regenerated one.** The 3D route
regenerates the character as a smooth T-pose and never fully recovers the
original: the face is mush at 96×96 and the silhouette is noisier than the
roster. Here the **first frame *is* the shipped art**, so frame 0 is correct by
construction and drift is bounded by how far the model travels from it.

**No de-pixelate / re-pixelate round trip.** The 3D route deliberately asks for a
smooth render (image-to-3D needs it) and then re-quantises. This never leaves
pixel space.

**The loop is free.** `first_frame == last_frame` closes the cycle by
construction — structurally the same trick as Kimodo's in-betweening, which
measured a 0.283 cm seam.

## Why it might fail — the evidence is against it

[The research](./sprite-animation-research.md) is specifically negative here:

- Video-model sprite pipelines inherit **compression artefacts and temporal
  wobble**; a practitioner reported *"pixels that should be static tend to dance
  around"* and needed a custom temporal-smoothing pass
- **Grid misalignment** — *"pixels aren't aligned to a traditional grid which
  leads to really noticeable fringing"*
- Scenario's own animation path is video-model-based and is **not pixel-art
  native**; their guide gives no pixel-art guidance at all
- Diffusion **cannot hold a 4px face** (§2 of the research), which is the defect
  the 3D route also has

**But we have something Scenario users don't:** `spritekit`. `remapToPalette`
forces the class's exact 20 colours, `alphaThreshold` forces binary alpha, and
`planFit`/`renderFit` re-establish the grid and the 82px figure. Palette crawl,
soft alpha and off-grid pixels are precisely what that chain fixes. **Temporal
wobble is the risk it cannot fix.**

## Plan

### Stage V1 — input
Source: `map.png` (96×96, the shipped art) **nearest-upscaled** to the model's
working size, so the pixel grid stays hard. Alternative: `raw-battle.png` at
896×1186 for more detail, accepting that it is battle-tier framing.

### Stage V2 — generate
`MiniMaxH3ImageToVideo(clip, vae, prompt, width, height, length,
first_frame=SRC, last_frame=SRC)`.

- **first == last** → a closed loop
- Prompt: the action only, plus a fixed camera and no scene — e.g. *"the character
  walks in place, side view, fixed camera, plain background, no camera movement"*
- `length`: short. A 1.2 s cycle at 24 fps is ~30 frames, matching the 37-frame
  Kimodo loop
- Encoder is `qwen3vl_32b_minimax_h3_nvfp4_awq` (15 GB, installed); VAE
  `minimax_h3_video_vae_fp16`

### Stage V3 — extract
Sample 4 frames evenly (0, ¼, ½, ¾). Frame 0 is the source, so it is a free
correctness check: if frame 0 does not match `map.png`, conditioning is not
holding.

### Stage V4 — contract
`spritekit fit --outline` per frame, then `check`. Same tool, same 25/25-verified
rules.

### Stage V5 — judge
Beside the shipped sprite **and** beside the 3D route's output
(`17-textured-96px-sprites.png`). Two comparisons, because the question is not
"is it good" but "is it better than what we already have".

## Gate

**Pass only if all three hold:**
1. **Frame 0 reproduces the source** after the contract chain — the identity anchor works
2. **The legs actually move** — this is where diffusion failed four times before,
   and where the 3D route succeeds
3. **No temporal wobble** — static regions (head, torso) stay put frame to frame.
   Measurable: mean per-pixel change in the upper third should be near zero for a
   walk

If (2) fails it is the same wall as every earlier diffusion attempt, and the
answer is that the 3D route stands. If (3) fails, it is the documented video-model
defect and only a temporal-smoothing pass would rescue it.

## Cost

**Cheap to test.** Everything is installed; no downloads, no container. One
generation plus four `spritekit` calls. Compare against the 3D route's cost:
6 stages, ~30 GB of models, two containers, and a retarget that took six attempts.

**If it works it is dramatically simpler** — and it would make the 25-class batch
trivial, since there is no per-class mesh, rig or retarget.

## Honest expectation

The evidence says this fails on **pose** — the same way the 2-up sheet, the
hybrid, and the instruction-edit attempts all failed: diffusion produces
plausible frames that do not differ in the way a walk requires. The 3D route
exists precisely because pose control is the thing diffusion cannot do.

What is genuinely different this time: **an image-to-video model is trained on
motion**, not on single images. That is a different capability from the
image-edit models that failed, and it is the one reason to expect a different
outcome. Worth one test on that basis — and worth stopping quickly if the legs
do not move.
