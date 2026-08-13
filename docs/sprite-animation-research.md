# Can diffusion animate pixel-art sprites? Research findings

**Date of research: 8 August 2026.** All links checked on that date; this is a
fast-moving area and the conclusions below have a short shelf life.

**Context.** The target game ships one still front-facing 96×96 map sprite per class
(20-colour palette, ~82px figure) and synthesises its 16-frame sheet at boot by
shifting horizontal bands of that one image. The question was whether crux could
generate real per-frame walk art instead, using the local ComfyUI install.

**Verdict: no. Diffusion-based pixel-art sprite *animation* is unsolved as of
August 2026** — not "hard", not "needs better prompting". No shipped
counter-example was found across ~20 searches of Hacker News, itch.io, Steam,
GitHub, Hugging Face, Civitai, GDC and arXiv.

This document records the evidence, including the evidence against approaches we
tried and liked, so the question doesn't get reopened from scratch.

---

## 1. Has anyone shipped diffusion-generated pixel-art walk cycles?

No verified case found.

**What *is* shipping is static asset generation, and it works well.**

- **Melting Encyclopedia** (Steam, released) generated ~6,144 static item and
  creature sprites via PixelLab, then converted to indexed colour and packed
  sheets. Its Steam AI disclosure covers voice and character *stills* — animation
  is not mentioned.
  <https://store.steampowered.com/app/2826670/Melting_Encyclopedia/>
- **Skaldborn** built a ComfyUI + PixelLab pipeline for concept art and 8-direction
  character rotations. The devlog covers static portraits and rotations only.
  <https://www.skaldborn.com/devlog/04-art-content-pipeline/>

**The one shipped case involving animation is heavily qualified.** *Dueling
Dynasties* (game jam, itch.io) shipped SD-generated spritesheets — but via
ControlNet plus the `charturnerv2` textual inversion, in a **3D-render style, not
pixel art**, and charturner produces turnarounds rather than walk cycles. The
authors' own conclusion: when they wanted a different look they *"had to edit the
spritesheet themselves, which falls back on art skills and doesn't save much
time."*
<https://bromberry.itch.io/dueling-dynasties/devlog/522129/we-made-animations-with-ai-stable-diffusion>

**Practitioner reports are consistently negative and are not getting better over
time.** One describes our exact task:

> "My test is a walk cycle for animating... it has stomped every diffusion model
> I've tried... you would think that kontext would retain the context to get the
> same character but with the left foot forward instead of the right, and it just
> refuses." — HN, June 2025 <https://news.ycombinator.com/item?id=44167770>

> "even with state-of-the-art models (like NB Pro or GPT-Image-2), good luck
> getting them to generate sprite sheets with consistent frames for animation
> particularly if you're shooting for a pixel art aesthetic... still required a
> fair amount of manual photoshop (or in my case Aseprite) post-processing."
> — HN, **July 2026** <https://news.ycombinator.com/item?id=48897356>

> "You can generate tons of sprite sheets using AIs and each will look good and
> sometimes great, but getting right style, lighting and camera perspective is
> near impossible. And animations are even harder."
> — HN <https://news.ycombinator.com/item?id=44208567>

The July 2026 report is no more optimistic than the June 2025 one. That is the
single most important fact in this document: a year of model progress did not
move this problem.

**Industry context.** GDC 2026 State of the Industry: only 36% of game
professionals use genAI at all, 30% at actual studios. Top uses are brainstorming
(81%), email/code (47%), prototyping (35%) — asset production does not appear.
52% report genAI having a negative impact, up from 30%; visual and technical
artists are the most negative discipline at 64%. No GDC talk exists on shipping
diffusion-generated 2D character animation.
<https://gdconf.com/article/gdc-2026-state-of-the-game-industry-reveals-impact-of-layoffs-generative-ai-and-more/>

---

## 2. Why it fails: three compounding mechanisms

None of these is a prompting problem, which is why our four attempts all failed
in different-looking but related ways.

**(a) The VAE bottleneck.** Latent diffusion VAEs downsample 8×; detail smaller
than that threshold *"completely disappear[s] before the diffusion even
begins."* Pixel art is entirely high-frequency detail. An entire 2025–26 research
line exists specifically to escape latent space for this reason — DiP
(<https://arxiv.org/html/2511.18822>), PixelDiT
(<https://arxiv.org/pdf/2511.20645>), PixelFlow
(<https://arxiv.org/pdf/2504.07963>), DeCo (<https://arxiv.org/pdf/2511.19365>).
<https://medium.com/@efrat_taig/vae-the-latent-bottleneck-why-image-generation-processes-lose-fine-details-a056dcd6015e>

**(b) The small-face problem — our sprite's face is ~4px.** So universal that the
standard remedy is a separate tool: ADetailer exists because *"when generating
full-body poses or distant figures... the face often becomes distorted while the
body looks perfect. This is caused by the AI's lack of resolution."* Its method is
to crop the face, redraw it at high resolution, and composite back. **That method
is unavailable to us**: there is no higher resolution a 4px face can be redrawn at
and downsampled back to 4px without the result being effectively arbitrary.
<https://learn.thinkdiffusion.com/adetailer-after-detailer-your-automatic-image-enhancement-tool/>

**(c) Pixel art's constraints are the inverse of what diffusion produces.** Pixel
art needs grid alignment, a limited palette, deliberate dithering and minimal
anti-aliasing. Diffusion natively produces varying pixel sizes, inconsistent
outlines, blur, partial transparency and glow. Retro Diffusion's founder: *"If you
zoom in, you can pretty quickly tell that the 'pixels' are different sizes."*
Making it work required a bespoke dataset, a modified architecture, **and a
proprietary clustering-based downscaler + quantiser** — the pixel-perfection is
bolted on *after* diffusion, not learned by it.
<https://runware.ai/blog/retro-diffusion-creating-authentic-pixel-art-with-ai-at-scale>

**(d) Even at high resolution, AI doesn't do animation *as animation*.** A study
against the 12 principles found AI performs acceptably on static principles
(Staging, Solid Drawing, Appeal) but *"consistently fail[s] to replicate dynamic
principles like Squash and Stretch, Follow-Through, and Anticipation."* An
animator grading AI output on first-year exercises gave the walk cycle a **D**:
*"about a second and a half in the arms switch and the back arm suddenly jumps to
the front."* <https://core.ac.uk/outputs/656680783> ·
<https://lifeinthemachine.substack.com/p/can-ai-pass-1st-year-animation-school>

**Academic state of the art on exactly our problem.** *Sprite Sheet Diffusion*
(reference sprite + pose sequence → frames) admits in its own limitations that it
*"still struggles with overfitting during Stage 2 training and maintaining subject
consistency and finer details"*, with named failures on hair and held props, and
notes the sprite domain suffers from *"limited high-quality training data."*
<https://arxiv.org/html/2412.03685v2>

---

## 3. Instruction-editing an existing sprite into a new pose

This was our proposed fourth attempt (download Qwen-Image-Edit and tell it to
change the pose). The evidence killed it before we spent the disk. The mechanisms
below are architectural, and most are not specific to one model.

- **Resolution mechanics.** Qwen-Image-Edit resizes the condition image to a ~1MP
  budget; the VAE downsamples 8× and a patch-embed layer another 2× — **16×
  total**. A 1024×1024 condition image becomes a **64×64 token grid**. Our sprite
  is **96×96**. The model's internal representation is *coarser than the
  artwork*. <https://github.com/huggingface/diffusers/issues/12997>
- **"Pixel drift" is a named, documented, unsolved defect** — output does not
  align with input, shifting or zooming by a few pixels. At 96×96 with chunky
  pixels, a few pixels of drift destroys the grid. Published workarounds fix
  alignment but *"introduce a different problem: quality degradation."*
  <https://myaiforce.com/fix-pixel-drift-for-qwen-edit/>
- **Square inputs are the worst case** — and a sprite is square. Open issue:
  square outputs show *"strange hallucinations and consistency issues"* and lose
  likeness, while non-square stays crisp. Reproduced in FP8, BF16 and full
  unquantised BF16 with no LoRA — confirmed in the base model, no maintainer
  response. <https://github.com/QwenLM/Qwen-Image/issues/243>

**Stated fairly, the counter-evidence:** vendor benchmarks do rate this class of
model at or near SOTA for pose manipulation with identity preservation
(<https://genai-showdown.specr.net/image-editing>). But every such test is a
photorealistic or illustrated subject at ~1MP. None is pixel art, none is
small-format, none is a fixed 20-colour palette.

**The most instructive datapoint.** The one serious public tutorial titled
*"Create a consistent character animation sprite using ComfyUI"* — which does use
Qwen Image Edit — **does not use it to change the pose.** It generates T-pose
views, builds a Hunyuan3D model, rigs it in Blender/Mixamo, animates it, and only
then uses Qwen + Pose/Depth ControlNet to *restyle the 3D renders*. The people who
achieve consistency abandon image-edit reposing and build a 3D proxy.
<https://tawusgames.itch.io/ai-gen-sprite-tutorial>

---

## 4. What real pixel-art pipelines actually use

**3D-proxy-to-2D is the production-proven answer for our exact constraint.**

**Dead Cells** was built this way *specifically because one artist could not
hand-draw the volume.* Thomas Vasseur: *"To make up for the lack of bandwidth and
still deliver on quality, we had to find a pipeline that could give us great
looking pixel art, without having to hand draw each and every retake."* One artist
for the first year, two after. 3DS Max → FBX skeletons → homebrew renderer at low
resolution with **no antialiasing** → normal maps + toon shader. Admitted
downsides: *"flickering pixels"* requiring manual cleanup, less detail than
hand-drawn, and a deliberate choice to prioritise **movement over detail**.
<https://www.gamedeveloper.com/production/art-design-deep-dive-using-a-3d-pipeline-for-2d-animation-in-i-dead-cells-i->
· <https://www.gameanim.com/2018/01/31/dead-cells-3d-pipeline-2d-animation/>

**HD-2D** (Octopath Traveler, Triangle Strategy) — Square Enix Team Asano +
Acquire, UE4, sprites billboarded into 3D scenes with real lighting.
<https://en.wikipedia.org/wiki/HD-2D>

**Hand-animation remains the premium route.** Sea of Stars is *"animated almost
completely using hand-drawn keyframes and a generous amount of them."*

**Skeletal 2D (Spine/DragonBones) is a poor fit for chunky pixel art** — rotation
breaks the pixel grid and produces jagged, mixed-size pixels. Workarounds are
draw-at-8×-then-RotSprite and nearest-neighbour with smoothing off. Viable only if
you avoid rotation.
<https://esotericsoftware.com/forum/d/16842-how-to-keep-pixels-straight-when-animating-pixel-art>

---

## 5. A craft finding independent of all of the above

**For an 82px figure, a 2-frame walk cycle is off-spec.** Manning Krull's
reference tutorial rates 2-frame cycles for *"tiny sprites only; 32 pixels high at
the very most, but I'd recommend only using this for sprites that are more like 24
pixels high or smaller"*, and 4-frame for *"small sprites only; no taller than 32
pixels."* For our size it prescribes the **8-frame** cycle, rated *"24x24 all the
way up to fighting game character size (100+ pixels)."*
<http://manningkrull.com/__pixel-art/walking.php>

The 2-frame illusion works because the viewer's brain fills in the gap. It
collapses once there's enough resolution to see the legs clearly. This may explain
part of why nothing generated read as walking, regardless of technique.

**Tension with the current design:** the game's sheet is 4 columns per facing
(idle A/B, walk A/B) and `test/sprites/poses.test.ts` asserts
`sheetW === map.w * columns`. Moving to 8 frames means changing sheet geometry —
a larger game-side change than the original plan assumed.

---

## 6. Purpose-built sprite tools

- **Retro Diffusion / RD Animation** — the most respected pixel-art model, and it
  **explicitly cannot animate existing characters**: *"reference images serve as
  guidance for pose or design but are not used as starting frames."* Also trained
  at 256×256 and below, frame sizes fixed per style (walk animations may be
  limited to 48×48), and cannot be run locally.
  <https://help.scenario.com/en/articles/retro-diffusion-models-the-essentials/>
- **PixelLab** — the only credible "animate my existing sprite" option at our
  size. `POST /v2/animate-with-skeleton` accepts init images up to **128×128**
  (our 96×96 fits); `animate-character` (64–168px) requires an existing character;
  `animate-with-text-v3` takes a starting and optional ending frame, up to 16
  frames. <https://www.pixellab.ai/pixellab-api>
  **Caveat:** every "review" found was SEO affiliate content. No independent
  hands-on test of `animate-with-skeleton` on a ~96×96 sprite was found. That gap
  is the single most concrete untested option available to us.
- **Scenario.gg** — sprite sheets exist, but its animation path is video models
  (Seedance, Wan 2.2 I2V) → frame extraction, which inherits video compression and
  temporal wobble before you downscale.
  <https://help.scenario.com/en/articles/create-spritesheets-with-scenario/>
- **Layer.ai** — 200+ studios (Zynga, Tripledot, SciPlay), but the customer list
  shows the use case: casual/social marketing art, live-ops, UI — not gameplay
  character animation.
- **Sprite Fusion** is a tilemap editor; **Rosebud** is a game-making platform.
  Neither is a sprite animator.

---

## 7. What we actually tried, and how it maps to the above

Four attempts were made locally before this research was run. Recording them
because each failed in a way the literature predicts.

| Attempt | Result | Predicted by |
|---|---|---|
| Diffusion 2-up sheet (both frames in one pass) | Identity held across the pair; **zero pose differentiation**, face destroyed at 96px | §2(b) small-face, §1 "it just refuses" |
| Blender 3D box proxy → ortho render → quantise | **Exact pose control and cross-frame identity**, contract passed first try; art was a mannequin | §4 — right approach, wrong execution (boxes, not a modelled character) |
| Hybrid: 3D render as structure + sprite as identity | Identity transferred well; **pose transfer failed entirely** | §3 pixel drift + reference latents dominating composition |
| Qwen-Image-Edit download | **Not attempted** — withdrawn on this evidence | §3 |

The local stack has exactly two models: `krea2_turbo` (guidance-distilled, runs at
cfg 1, so editing instructions have no guidance channel to act through) and a
Minimax video model. No ControlNet, no checkpoints, no non-distilled editor.

**A reasoning error worth recording.** ControlNet was rejected early on the
grounds that pose *estimators* don't generalise to chibi proportions. That
objection is valid for OpenPose and invalid for **depth or canny** ControlNet,
which need no estimation at all — Blender exports a depth pass natively. The
documented working recipe in §3 uses exactly that.

---

## 8. Where this leaves us

1. **The 3D-proxy route is the one with a production record** for our exact
   constraint (small team, many characters, chunky pixel output). Our proxy
   attempt failing once is weak evidence against a pipeline that shipped Dead
   Cells; the failure was execution, not approach.
2. **Diffusion belongs downstream, if anywhere** — as a depth-ControlNet restyle
   over 3D renders, which is the documented recipe. It is not the generator.
3. **PixelLab's skeleton endpoint is the one untested, dimensionally compatible
   option.** Worth a day, not a month.
4. **The frame-count question is independent** and should be settled on its own
   merits: 2 frames is off-spec for this figure size.

**Contradictory evidence deliberately left unresolved:** vendor documentation
(Scenario, PixelLab, Retro Diffusion) claims this class of workflow works well;
independent practitioners with no commercial stake say it does not. We found no
independent hands-on test at our sprite size to break the tie.

---

## Process note

This research was run **after** four implementation attempts rather than before.
Each attempt was justified from mechanism ("a shared latent makes identity
structural", "3D gives exact pose control", "real CFG gives instructions
something to grip") rather than from evidence that the approach had ever worked.
Every mechanism behaved as predicted; every deliverable was unusable. The research
above was available the whole time and would have redirected the effort on day
one.
