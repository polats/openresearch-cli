# Map-sprite animation via image-to-video — spec and workflow

**Date: 9 August 2026.** This is the recipe that produced TacticaArena's first
real walk cycle, written down so the idle set and the remaining 24 classes can be
made the same way. Everything here was measured on `warrior`, not assumed.

Companion docs: [sprite-pipeline-video-route.md](./sprite-pipeline-video-route.md)
(the proposal), [sprite-pipeline-as-built.md](./sprite-pipeline-as-built.md) (the
3D route this replaced).

---

## 1. The finding the whole recipe rests on

**Pin the endpoints on the pose you want the loop to pass through, not on rest.**

MiniMax H3 is a first-last-frame model. If `first_frame == last_frame == the idle
hero art`, the model must leave rest and return to rest. It resolves that by
producing motion that looks the same played forwards and backwards — a two-pose
bob. Measured on the front-facing warrior:

| conditioning | shortest true period | leg swaps per clip | dead frames |
|---|---|---|---|
| `idle → idle` | 50 frames (the whole clip) | **0** | 13 |
| `first_frame` only, unpinned | — | 4 | 10 |
| **`contact → contact`** | **26 frames** | **2** | 0 |

A walk cycle is *anti*symmetric — the second half is the mirror of the first, with
the other leg leading. Pinning to a standing pose permits a symmetric solution and
the model takes it. Pinning to a mid-stride contact pose removes the resting state
the model can collapse onto, and the legs alternate.

Prompting does not substitute for this. The failing run's prompt already said
*"legs alternating forward and back"* verbatim and produced zero leg swaps.

**Corollary, and it matters for idles:** the same symmetry that ruins a walk is
*correct* for an idle. An idle genuinely is an out-and-back to rest, so
`idle → idle` pinning is the right conditioning there — the defect and the
requirement are the same shape.

---

## 2. Generation settings

ComfyUI graph, saved at `scratchpad/sprite/h3_fast.json`. Node 5 is
`MiniMaxH3ImageToVideo`; `first_frame` and `last_frame` are both optional.

| setting | value | why |
|---|---|---|
| UNet | `minimax_h3_fl2va_pruned_int8_convrot` | `fl2va` = first-last-to-video |
| CLIP | `qwen3vl_32b_minimax_h3_nvfp4_awq` | |
| VAE | `minimax_h3_video_vae_fp16` | |
| guider | **`BasicGuider`** | model is guidance-distilled; CFG is 2.8× slower for no gain |
| sampler | `res_multistep` | official template |
| scheduler | `simple`, **20 steps** | official template |
| sigma shift | `shift_video 12.0`, `shift_audio 3.0` | |
| **resolution** | **672×896** | see below |
| length | `17k + 5` — 22, 39, 56 | the model's frame grid; other values are invalid |
| seed | 31 | fixed across a class for consistency |

### Resolution is the entire cost lever

Measured, identical in every other setting:

| W×H | MP | frames | duration |
|---|---|---|---|
| 640×640 | 0.41 | 56 | **1.8 min** |
| 672×896 | 0.60 | 56 | **3.9 min** |
| 672×896 | 0.60 | 39 | **2.6 min** |
| 896×1184 | 1.06 | 56 | **7.0 min** |
| 896×1184 + CFG/30 steps | 1.06 | 56 | 19.3 min |

Scaling is worse than linear in pixel count — do not extrapolate, measure.

**672×896 is the choice.** It holds the source art's 3:4 aspect (896×1184 hero
art) so nothing is squeezed, and it puts the figure ~840px tall reducing to a
109px reduction of **÷7.7** — essentially the ÷7.3 the shipped battle tier already
uses. Generating at native 896×1184 costs 1.8× more and gives ÷10, *further* from
the ratio the bakery works at. The extra pixels are discarded by the downscale.

### Prompt structure

Three parts, in this order:

1. **The action**, stated concretely and with the phase structure named
   ("exactly TWO full steps: right leg forward, then LEFT leg forward, then right
   leg forward again, ending in exactly the same pose it started in").
2. **The negative behaviours**, as explicit prohibitions — never stops, never
   returns to a resting stance, does not step, does not attack.
3. **The invariant block**, verbatim every time:

> The character stays in the SAME view the whole time and does NOT turn, does not
> rotate, does not change which way the body faces. The character stays the SAME
> SIZE, no scale change, no zoom. Fixed camera, no camera movement, no panning.
> The character stays centered and does not move across the frame. Plain flat
> magenta background, nothing else.

Magenta is the chroma key `spritekit` expects (green for violet/plum/pink classes,
per `roster.mjs:285-298`).

---

## 3. Workflow

### Stage 0 — harvest the endpoint poses (once per class)

Only needed for cyclic motion. Idles skip this and pin to the hero art directly.

1. Generate `first_frame` = hero art, **no `last_frame`**, prompt = "steps forward
   and continues walking… does NOT stop, does NOT return to standing."
2. Fit every frame (stage 2) and measure **foot lead** — the signed horizontal
   offset of the foot-band centroid from the torso centroid:

   ```
   torso centre = mean x of opaque pixels, rows 40..80
   foot lead    = mean (x - centre) of opaque pixels, rows 108..128
   ```
3. Take the frames at min and max foot lead. Those are the two contact poses.
4. Upload the **raw** (pre-fit, 896×1184) frame as the conditioning image —
   conditioning wants full resolution, not the 128px sprite.

Warrior's came out at −2.3px and +5.0px. **That asymmetry is a known defect**: the
right leg takes a real stride and the left barely crosses, so the resulting walk is
lopsided. Re-rolling the seed or sourcing the weak contact from the 3D/Kimodo route
are the two ways to fix it; neither is done yet.

### Stage 1 — generate the clips

Per class, three clips for locomotion plus two for idle:

| clip | first_frame | last_frame | length | note |
|---|---|---|---|---|
| `idle → walk` | hero art | contact pose | 39 | transition |
| **`walk loop`** | contact pose | same contact pose | 56 | the cycle |
| `walk → idle` | contact pose | hero art | 39 | transition |
| **`idle loop`** | hero art | hero art | 39 | symmetry is correct here |
| **`idle flourish`** | hero art | hero art | 56 | one-shot gesture, returns to rest |

Pinning both ends of the transitions is what makes them chain — the handoff into
the loop is exact rather than searched for.

### Stage 2 — fit to the contract

```
node crux/sprite-tools/spritekit.mjs fit <raw.png> <out.png> \
  --pixel  TacticaArena/tools/bakery/pixel.mjs \
  --meta   TacticaArena/tools/baked-staging/<class>/meta.json \
  --chroma 255,0,255 --outline \
  --frame 128 --figure 109 --ground 4
```

Then `check` with the same flags. `tools/bakery/` is imported unmodified and never
written to.

`--figure` and `--ground` scale with the frame: `figure = round(frame × 82/96)`,
`ground = round(frame × 3/96)`.

**A `spritekit` fix was required to get here.** `planFit` snaps to a clean integer
factor `srcH/k`, so reachable heights form a ladder with ~10px rungs at the 128px
tier, against a 5px tolerance band — for some framings *no rung falls inside it*
and the convergence loop can never land (7/24 frames failed). `spritekit.mjs:231`
now pre-scales the trimmed source to `targetH × k` so the rung lands exactly where
asked, preserving the integer downscale that keeps the pixel grid hard. Result:
**24/24**, and the 25 shipped classes still pass at 96px unchanged.

### Stage 3 — extract the cycle

Autocorrelate the leg-band silhouette to find the true period, then take the
tightest window:

```
period P scores as mean over s of  silhouetteDiff(frame[s], frame[s+P])
```

Warrior: best period **26 frames at 5.2%**. Then rotate the phase so the cycle
starts at the pose the transitions were pinned to — a loop has no privileged start,
and rotating fixed both handoffs from ~35% to 6.0% and 3.5%.

Rotating moves the wrap seam from 1.1% to 7.9%, and that is an **improvement**:
1.1% meant two near-identical adjacent frames, i.e. a stall at the loop point. 7.9%
is an ordinary frame step (the cycle's mean is 10.8%), so motion is uniform through
the wrap.

### Stage 4 — sample to the shipped frame count

Evenly from the extracted cycle. **8 frames** is the recommendation, from measured
smoothness against cost:

| N | mean leg step | note |
|---|---|---|
| 16 | 19.6% | |
| 12 | 24.6% | |
| **8** | **32.0%** | past the pop-between-poses range |
| 6 | 36.8% | |
| 4 | 49.5% | reads as popping |

Below 12 the mean step climbs steeply. 12 is measurably smoother than 8 but costs
+52MB across the roster; 16 exposes more of the model's frame-to-frame detail churn
(the fur pad and belt buckle wobble even where the silhouette is stable).

---

## 4. Tier and playback specs

| property | value | source |
|---|---|---|
| frame | **128×128** | first tier where the face reads — see below |
| figure height | 109px (±4) | `frame × 82/96` |
| ground rows | 4 | `frame × 3/96` |
| palette | the class's own, ≤20 colours | `remapToPalette`, per-class cap |
| alpha | binary, threshold 128 | importer throws on partial alpha |
| chroma | `255,0,255` (green for violet/plum/pink) | `roster.mjs:285-298` |
| **frame hold** | **143 ms (7 fps)** | matches the shipped walk exactly |

### Why 128 and not 96

Head size by tier, from the same generated frame:

| tier | head px | verdict |
|---|---|---|
| 96 | 25 | eyes are single specks, no nose — a smudge |
| 112 | 27 | barely moves |
| **128** | **32** | **first tier where it is a face** — two eyes with highlights, brow separated, nose shadow |
| 160 | 40 | better still, but **equals the battle tier** — collapses the two-tier system |
| 192 | 49 | little gain over 160 |

Cost at 128, 25 classes, sheet VRAM including normals: 75MB at 6 columns / 4 rows,
**38MB** if `right` and `up` are derived at draw time as they are today.

Palette holds at 20 colours across every tier because `remapToPalette` forces the
class palette — the contract is satisfied by construction, not by luck.

### Frame rate

**Match the per-frame hold, not the cycle duration.** The shipped walk is 2 frames
at 7fps — a 0.286s cycle, but a 143ms hold. Matching the *cycle* forces 8 frames
into 286ms (28fps) and it reads as a blur. Matching the *hold* gives 7fps and a
1.14s cycle, which is a natural walk. The cycle is longer because it contains more
real poses; that is the entire point.

### Facing derivation

Do not generate the other three facings. `bakedmap.ts:370-375` derives them and the
new frames follow the same rules:

| row | shipped derivation | new side |
|---|---|---|
| S (down) | baked front art | **generated** |
| W (left) | front, head turned −1px | same frames |
| E (right) | front mirrored about its own centre | mirrored **per frame** |
| N (up) | `deriveBackView` — a repaint | not generated; front frames stand in, labelled |

Mirroring must be per frame. Mirroring the whole strip also reverses frame order,
which makes the walk moonwalk.

---

## 5. Verified results — warrior

- **104 frames**, all passing `spritekit check` at 128px: loop 26/26, `idle→walk`
  39/39, `walk→idle` 39/39
- **Registration is exact**: centre-x 63.5 and feet row 124 identical across every
  frame; head bobs 4px. No horizontal slide, no drift off the ground plane, so the
  per-frame anchor is stable by construction
- **Chain seams**: `idle→A` 3.4%, `A→loop` 6.0%, loop wrap 7.9%, `loop→C` 3.5%,
  `C→idle` 1.9% — every junction at or below a normal frame step
- **Cycle quality**: mean leg step 10.8%, head/torso drift 2.1%, torso width
  79–82px across the cycle (no turn, no zoom)
- **Total generation**: 9.1 min for all three clips

### Known defects

1. **The walk is lopsided** — foot lead +5.3px right against −1.7px left. It reads
   as a walk but not a balanced one.
2. **Style tier mismatch** — these look like small battle sprites, not the
   simplified redraw `MAP_PREAMBLE` specifies (*"eyes become two dark 1-2 pixel
   dots… No mouth, no nose"*). That preamble and the 25 shipped map sprites are
   written to a 96px target; 128px changes the art direction, which is the intent
   but has not been reconciled with the roster.
3. **Detail churn** — the fur pad, belt buckle and tassel change frame to frame
   even where the silhouette is stable. More frames expose more of it.
4. **One class only.** Nothing here is known to generalise past `warrior`.
5. **Licence unverified** — Kimodo R&D checkpoints are non-commercial; the video
   route does not use them, but the 3D route's outputs are still in the tree.

---

## 6. Idle set — design

Two clips, not one, because a single looping idle at any length reads as a
mechanism rather than a character.

- **`idle`** — the continuous loop. Weight shifts foot to foot, hips and shoulders
  counter-rotate with the shift, breathing, slight head drift. Explicitly *more*
  than a vertical breath: the whole body settles and re-settles.
- **`idle flourish`** — a one-shot gesture with a clear beginning, peak and return:
  shoulders roll, the axe is re-settled on the shoulder, a glance to the side, then
  back to rest. Pinned `idle → idle`, so it returns exactly.

**Playback: 4 idle loops, then one flourish, repeat.** Expressed in the manifest as
a sequence of clips with loop counts, which the dev modal plays directly — the
gallery's cell engine already supports a playlist with per-entry loop counts.

Conditioning for both is `first_frame == last_frame == hero art`. The symmetry
trap from §1 is the correct behaviour here.

---

## 7. Where things live

| what | where |
|---|---|
| Fitting/verification tool | `crux/sprite-tools/spritekit.mjs` |
| Saved ComfyUI graph | `scratchpad/sprite/h3_fast.json` |
| Generated frames, videos | `~/Documents/tacticaarena-sprite-research/` |
| Dev-only game assets | `TacticaArena/dev/newsprites/` (outside `src/`, not a build entry) |
| Comparison modal | `TacticaArena/src/sprites/dev/compare.ts` |

`TacticaArena/tools/bakery/` is imported read-only and never modified.
