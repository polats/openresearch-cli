# 2D → 3D map props, generated in crux

**Date: 10 August 2026.** Verified against this machine and against
TacticaArena at `3cf8913`. Re-check the node/model facts before acting — they
moved between 8 and 10 August already (see §1.3).

Companion to [image-to-3d-candidates.md](./image-to-3d-candidates.md) and
[local-3d-capability.md](./local-3d-capability.md). Those target **characters**,
where the mesh must rig. This targets **map props**, which never deform, and the
criterion changes completely as a result.

---

## 1. What is actually true today

### 1.1 The game's map is real 3D, and loads no meshes

TacticaArena renders the map with **three.js 0.170** (`src/render/terrain.ts`,
`flora.ts`, `surround.ts`, `geobuild.ts`, `lighting.ts`). Characters are 2D
billboards standing on it — HD-2D.

**There is no mesh loader anywhere in `src/`.** Zero hits for `GLTFLoader`,
`.glb`, `.gltf`, `OBJLoader`, `FBXLoader`. Every prop is procedural: merged
`BufferGeometry` per material, built in code.

So this workflow needs a **new consumption path in the game**, unlike the sprite
work which rode an existing sheet format. That is the single biggest difference
from the character pipeline, and it is a deliberate choice (§4).

### 1.2 The props today are billboards, and the spec already says that fails

`FloraKind` is 9 kinds, all slots in a **texture atlas** — pixel sizes, anchors
bottom-centre, at `PX_PER_UNIT` = 26 px/unit:

| kind | atlas slot | height (tiles) | band |
|---|---|---|---|
| `conifer` | 40×78 | 3.2 | big tree 2.75–3.5 |
| `oak` | 52×64 | 3.0 | big tree |
| `deadTree` | 34×52 | 2.9 | big tree |
| `coniferSmall` | 28×52 | 1.45 | small tree 1.0–1.5 |
| `oakSmall` | 40×48 | 1.4 | small tree |
| `bush` | 28×22 | 1.2 | small tree |
| `boulder` | 34×20 | 0.55 | rock 0.3–0.7 |
| `flowers` | 16×16 | 0.34 | ground dressing |
| `tuft` | 16×12 | 0.28 | ground dressing |

A character is **1.25 tiles**, so a conifer towers 2.6× over a unit.

`docs/VISUAL_BAR.md` already makes this workflow **binding**:

- **§A9.1** — props must be geometry, not cards. *"A 3D world with flat scenery
  reads as a diorama with stickers in it… now that characters read as volumes
  (five phases of work), the props are the weakest thing on screen."*
- **§A9.4 (BINDING, owner request 2026-08-08)** — *"I want variation in the
  trees, rocks and vegetation."* A single mesh instanced forty times *"still
  reads as a copy-paste forest."*
- **§A9.5 (BINDING, owner request)** — built structures as obstacles.
- **§D3** — 60 fps at 1440p, **zero per-frame allocation**; more distinct meshes
  means more draw calls, so *"a budget stated in the plan — not a hope expressed
  in a summary."*

`src/render/spritevol.ts` (789 lines) exists purely to fake volume on billboards
— normal maps derived from the alpha silhouette, a curved slab, rim and bounce
— because of the owner defect *"they look super 2d, just a flat piece of paper
on the 3d landscape."* Real prop geometry is the actual fix for that class of
defect.

**Spec/code drift to note:** §A9.4 says variation is ±8%; `scale.ts` ships
`FLORA_JITTER_MIN = 0.95`, `FLORA_JITTER_SPAN = 0.1` — **±5%**. The code is
narrower than the spec claims. Trust the code.

### 1.3 Generation works here TODAY — this corrects the 8 August doc

`local-3d-capability.md` says the Hunyuan3D models are not installed. **They
are now.**

| | state |
|---|---|
| `EmptyLatentHunyuan3Dv2`, `Hunyuan3Dv2Conditioning`, `Hunyuan3Dv2ConditioningMultiView`, `VAEDecodeHunyuan3D` | **registered** (ComfyUI core, pure Python) |
| `VoxelToMesh`, `SaveGLB`, `Preview3D`, `Load3D` | **registered** |
| `hunyuan3d-dit-v2-0-fp16.safetensors` | **installed** |
| ComfyUI | 0.30.0, 798 nodes, reachable |
| TRELLIS.2-4B + FP8 weights | on disk, **pack ABI-dead** (12 missing nodes) — Docker only |
| Blender 5.2.0 + MCP add-on | **add-on not started** — needs Allow Online Access + Start (see §7) |
| UniRig / Kimodo | up, and **irrelevant here** — props don't rig |
| **Z-Image-Turbo** `int8_convrot` + `qwen_3_4b` + `z_image_ae` | **installed, validated** — 8 steps, Apache-2.0 |
| **Z-Image Fun ControlNet Union 2.1** lite-2602 | **installed, validated** — Canny/Depth/Pose/MLSD/Hed/Scribble/Gray in 1.88 GB, Apache-2.0 |
| **Depth-Anything-3** `mono_large` + `small` | **installed, validated** — Apache-2.0 |
| **RealESRGAN_x4plus** | **installed, validated** — 1024²→2048², BSD-3 |
| **SDPose** `wholebody_fp16` | **installed, validated** — MIT |

**The criterion relaxes enormously versus characters.** The candidates doc ranks
models on *"does UniRig produce a clean rig"* — joint topology, limb separation,
weapon separation. None of that applies to a rock. Props need a good silhouette
and a low triangle count, which is the easiest thing these models do.

**Therefore: use Hunyuan3D v2, do not build Docker'd TRELLIS.2 or Pixal3D.** It
is build-free, installed, and the quality gap it loses on (surface detail, PBR)
is thrown away by our own budget and art direction anyway.

---

## 2. The hard part, stated up front

**The input is a 78-pixel-tall pixel-art billboard. Image-to-3D models are
trained on photographs and renders of real objects.**

This is the risk that decides the project, and it is not a tooling risk. Two
things can go wrong and they need different answers:

1. **Mush.** The model may produce a blobby, unreadable mesh from a chunky
   low-res sprite. Mitigation: generate a **clean high-res concept render first**
   (the existing 2D asset is the *art direction reference*, not necessarily the
   literal conditioning image), then image-to-3D from that.
2. **Art-direction mismatch — the deeper one.** §A9.4.5 requires *"the palette,
   the light response and the pixel density stay consistent across the whole
   set."* A crisp low-poly mesh standing next to a chunky pixel character breaks
   pixel density by construction. Generated PBR textures will break palette too.

**So the generated texture is discarded.** Props get the game's own material and
palette, and pixel density is preserved by keeping the mesh's screen-space texel
size matched to the terrain texel at 26 px/unit. This is the thing to validate
visually on prop #1, before any roster work.

### Risk 1 is now measured, and largely answered (10 August)

The mush risk was real and the fix is known. A text-to-image conifer came back as
a **flat vector illustration** — hard outline, flat fills, no self-shading — which
would have produced a cutout, not a mesh. The same subject through
**Depth-Anything-3 → masked depth map → ControlNet at strength 0.80** came back
with overlapping branch tiers, tier-on-tier shading and a modelled trunk: a
genuinely mesh-conditionable render.

**The trap that cost a run, and belongs in every plan that uses depth control:** a
raw depth map's background is black, and so is the subject's far side, so the
generator paints the whole region as one solid mass. Mask the background to 0 and
remap the subject into a band above it (measured: 70–255 worked; the subject
occupied only 134–240 of the raw range under default normalisation). Lowering
ControlNet strength — the intuitive fix — makes it worse, not softer.

Canny needed none of this and worked first try, holding a boulder's silhouette and
facet structure exactly while swapping sandstone for basalt. **That is the §A9.4
capability**: one shape, several materials, one art direction.

---

## 3. Pipeline

Chosen pilot: **vegetation and rocks** (§A9.4). Structures (§A9.5) follow once
the recipe holds.

```
00-reference.png     source art or a Z-Image text-to-image concept
01-control.png       Canny edge map, or MASKED depth map (see §2)
02-concept.png       Z-Image + ControlNet — the mesh-conditionable render
03-raw.glb           Hunyuan3D v2 image-to-3D
04-clean.glb         Blender: decimate to budget, Y-up, origin bottom-centre,
                     single material, generated texture stripped
05-final.glb         palette material applied, LODs
06-manifest.json     kind, variant, tiles height, anchor, tris per LOD, settings
```

Stage 1 is the departure from the character pipeline and the reason it should
work: **condition on a clean render, not on the 78px sprite.**

Measured settings for stages 01–02, taken from the official ComfyUI templates
rather than guessed (the templates are the correct source; `object_info` reports
enums as `COMBO` and will not give you valid values):

| | |
|---|---|
| loaders | `z_image_turbo_int8_convrot` + `CLIPLoader(qwen_3_4b, **lumina2**)` + `z_image_ae` |
| patch | `ZImageFunControlnet` (not `QwenImageDiffsynthControlnet` — that is the older template's route) |
| sampler | 8 steps, cfg 1.0, `res_multistep`/`simple`, `ModelSamplingAuraFlow` shift 3 |
| negative | `ConditioningZeroOut` off the positive |
| Canny | low 0.1 / high 0.32 |
| depth | `DA3Inference(504, lower_bound_resize, mono)` → `DA3Render(depth, **min_max**, sky_clip=false)` |
| strength | **0.80** (documented range 0.65–1.00; 0.60 was measurably worse) |
| hi-res | `ImageUpscaleWithModel(RealESRGAN_x4plus)` → `ImageScaleBy 0.5` → 5 steps `dpmpp_2m_sde`/`beta` at **denoise 0.33** → 2048² |

`DA3Render`'s `output` is a `COMFY_DYNAMICCOMBO_V3` and needs its nested inputs
supplied **three ways at once** in the API format — plain value, plain siblings,
and dotted `output.child` keys. Validation demands the dotted form, `execute()`
demands the plain one; four other encodings failed one side or the other.

---

## 4. Game-side consumption — runtime GLB + manifest

Decided. Two new modules, and the existing billboard path **stays as a
fallback**, exactly as the walk-frame plan made real frames additive:

- `src/render/props/loader.ts` — `GLTFLoader` at startup, reads the manifest.
- `src/render/props/registry.ts` — kind → `InstancedMesh` per (kind, variant,
  LOD). One draw call per bucket; instances filled per map, so **zero per-frame
  allocation** (§D3) is preserved. Loading is startup-time, not frame-time.
- `flora.ts` / `terrain.ts` — `addFlora` picks a mesh variant when the registry
  has one for that kind, else billboards as today. The 9 kinds and the density
  and placement logic are untouched; placement stays deterministic
  (`Rng(seedFromString("<map>:fl:<x>:<y>"))`).

`src/core/` stays frozen. `tools/bakery/` is untouched.

### Budget — stated, per §D3

| class | LOD0 tris | LOD1 tris | variants |
|---|---|---|---|
| big tree (`conifer`, `oak`, `deadTree`) | ≤ 1200 | ≤ 300 | 3 each |
| small tree / `bush` | ≤ 400 | ≤ 120 | 2–3 each |
| `boulder` | ≤ 250 | ≤ 80 | 3 |
| `flowers`, `tuft` | ≤ 80 | billboard | 2 each |

Draw calls: one per (kind, variant, LOD) bucket that has ≥1 instance. **Cap: 24
buckets for all props on any one map.** That is the number to measure against 60
fps at 1440p; if it fails, variants merge into shared atlased materials before
anything else is cut.

---

## 5. The contract, and how to check it without ground truth

`spritekit` could verify itself against art the game already ships. **There are
no shipped meshes**, so there is no ground truth — which is exactly the trap
that made the battle-tier contract wrong the first time.

The substitute: **the existing billboard atlas IS the intended silhouette.**
Render the candidate mesh orthographically at the atlas slot size and compare.

`sprite-tools/propkit.mjs`, sibling of `spritekit.mjs`, reading its numbers from
the game rather than from memory (`FLORA_TILES`, `FLORA_JITTER_*`,
`PX_PER_UNIT`, `FLORA` slots):

| check | rule | source |
|---|---|---|
| still reads as that prop | silhouette IoU vs the atlas billboard ≥ threshold | `textures.ts` `FLORA` |
| **variety** | pairwise silhouette distance between variants, after normalisation — near-identical means one mesh wearing hats | §A9.4.1, verbatim |
| height in band | `FLORA_TILES[kind]` × [0.95, 1.05] | `scale.ts` |
| anchor | origin at bottom-centre of the opaque bbox | atlas anchors |
| orientation | Y-up, faces −Z at rest | three.js |
| material | exactly 1, generated texture stripped | §A9.4.5 |
| budget | tris per LOD within §4 | this doc |
| no animation | 0 clips | props are static |

§A9.4.1 hands us the variety test in measurable form; it is the criterion most
likely to fail quietly, because three trees from the same prompt and model are
cousins — the same identity problem the sprite work hit, in a new coat.

---

## 6. crux-side additions

The ask is "add this into crux". Deliberately small, because the plumbing built
for the game-artist persona already covers most of it.

1. **Extend the `game-artist` persona with a third route** rather than adding a
   persona. It already owns ComfyUI + Blender + the files dir + the GLB orbit
   viewer, and props reuse all four. A new persona would duplicate the lot.
   The route table gains: *static prop geometry → Hunyuan3D v2 + Blender
   cleanup, no rig, no motion.*
2. **New skill `orx-mapprops`** — the stage pipeline (§3), the
   condition-on-a-clean-render rule (§2), the contract (§5), and the budget
   (§4). Registered the same way `orx-gameartist` was: `agent_skills.rs` enum
   entry, `skills_for_persona`, `include_str!`.
3. **`sprite-tools/propkit.mjs`** — the checker in §5.
4. **No new status cards.** ComfyUI already has one and is the only backend that
   generates; Blender already has one and only needs *starting* (§7). UniRig and
   Kimodo are not in this path.

Nothing in the artifact plumbing needs changing: `.glb` already serves as
`model/gltf-binary` and already opens in the orbit viewer, so a delivered prop
is inspectable in chat the moment it is written.

---

## 7. Blocked / dependencies

- **Blender MCP is not reachable**, and stage 03 needs it. Two things, both in
  Blender's UI: Preferences → System → **Allow Online Access** (the add-on hard
  gates on `bpy.app.online_access`), then Preferences → Add-ons → MCP →
  **Start MCP Bridge Server**. Enabling the add-on alone does not open port 9876.
- Decimation could fall back to a headless `blender --background` script if the
  add-on stays down, at the cost of losing interactive inspection.

---

## 8. Pilot, and the stop condition

**One `boulder`, then one `conifer`.** Rock first: it is the shape most forgiving
of a generative model's weaknesses and the fastest way to learn whether the
palette/pixel-density answer in §2 holds. Conifer second: tall, thin, and the
kind most likely to come back as mush.

Then **look at it beside the billboard it replaces**, in the game, orbiting. A
prop that passes every check in §5 can still read worse than the billboard it
replaces — `spritevol.ts` is five phases of work invested in making those
billboards good, and it would be dishonest to assume raw geometry beats it
automatically.

**Stop condition:** if a generated mesh cannot hold art direction beside the
existing pixel characters, the honest outcome is to keep the billboards and say
so. That is why the game-side change is additive and the billboard path stays.

## 9. A closed decision that this work reopens

`image-to-3d-candidates.md` rejected pose control outright: *"pose estimators do
not generalise to sprite proportions — chibi heads and 3-4 px mitten hands are far
outside the human-pose distribution."*

**On this machine that no longer holds at concept scale.** SDPose extracted a full
wholebody skeleton — torso, both arms with individual finger keypoints, legs, feet
and face points — from the 1254² Runeblade *pixel-art* sprite, and the recovered
pose matches the source's mid-guard lunge (weight back, torso pitched forward).

This is irrelevant to props, which have no pose. It matters for **characters**,
because pose control is exactly what the video route is documented as unable to
provide. It deserves its own evaluation rather than being folded in here.

## 10. Out of scope

- Structures (§A9.5) until vegetation and rocks hold.
- Characters — that is the other pipeline, and it needs rigging.
- Terrain itself; this is props standing on it.
- Docker'd TRELLIS.2 / Pixal3D, unless Hunyuan3D v2 demonstrably fails §5.
