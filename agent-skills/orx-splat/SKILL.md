---
name: orx-splat
description: "Turn one 2D image into a 3D gaussian splat with sprite-tools/splatkit.py on the local ComfyUI: verify the weights, generate, check the silhouette, compare. Use when a splat is asked for by name, or when a single view has to yield real volume — it measured 0.91 silhouette IoU where single-view mesh reconstruction gave 0.57 and flat sheets."
---

# Image to gaussian splat

`sprite-tools/splatkit.py` does the work. Your job is to give it a source worth
reconstructing, then decide whether the result earns its place.

Everything runs on the **local ComfyUI** through its built-in TripoSplat nodes.
Nothing leaves the machine unless you deliberately use `space` (see the last
section).

## This route is opt-in

A splat is not a drop-in replacement for a mesh prop. It has no triangles, no UVs
and no material, so a game that imports meshes cannot consume one. Reach for it
when:

- someone asks for **a splat**, by name; or
- you need **volume from one view** and the mesh routes have failed; or
- the deliverable is something to **look at and turn around** rather than import.

For a map prop that the game will load, `orx-mapprops` still owns the decision —
this route appears there as a third option, not the default.

## Prerequisite

```bash
python3 <crux>/sprite-tools/splatkit.py models          # verify
python3 <crux>/sprite-tools/splatkit.py models --install # ~3.4 GB, then re-verify
```

`models` asks ComfyUI what its loaders actually list, not what is on disk. A
download can exit 0 having written nothing, and the failure then surfaces
mid-generation as an unhelpful loader error.

**Free the model cache before you generate.** This is housekeeping, always in
bounds, and the graph loads 3.4 GB of its own:

```bash
curl -s -X POST http://127.0.0.1:8188/free \
  -H 'Content-Type: application/json' -d '{"unload_models":true,"free_memory":true}'
```

## The steps

```
1  propkit.py atlas   --game DIR --kind oak -o 00-art.png   # if the source is game art
2  propkit.py upres   00-art.png -o 01-big.png --refine
3  propkit.py isolate 01-big.png -o 02-cut.png
4  splatkit.py splat  02-cut.png -o 03-splat.ply
5  splatkit.py check  03-splat.ply --source 02-cut.png
6  splatkit.py compare 03-splat.ply --source 02-cut.png -o 06-compare.png
```

Steps 1–3 are `orx-mapprops`' own, unchanged and for the same reason: **an oak
sprite is 52×64 pixels, and a splat planned from it is a colourless blob that
still passes every structural check.** `splatkit splat` refuses a source under
384px and points back here.

Step 4 emits everything in one submit — `.ply`, a compact `.spz`, a 75-frame
turntable, a front still and its mask — because one submit loads the weights
once. Measured: **54–58 s** on an RTX 3090 for all of it.

If the source already has alpha (anything out of `propkit isolate`), do not pass
`--remove-bg`; the mask comes from the image. Pass it only for a flat RGB source,
where BiRefNet has to find the subject first.

## Why one view is enough here, and where that stops

On the same isolated oak plate, measured with the same silhouette metric:

| route | views | IoU | note |
|---|---|---|---|
| `propkit mesh` single-view | 1 | 0.57 | flat sheets, no back |
| `propkit meshviews` | 4 | 0.93 | needs a camera rig and four restyled renders |
| `splatkit splat` | 1 | **0.91** | no rig, no views, 57 s |

So the multi-view scaffolding that foliage needs for a *mesh* is not needed for a
*splat*. Do not carry the four-view habit over here.

What this does **not** buy you: a splat's back is inferred, not observed. It is
plausible rather than correct, and no metric in `check` can tell you which. If
the back matters and must be right, four real views into a mesh still beats one
inferred view.

## `check` and `compare` ask different questions

`check` answers **"is this a real splat?"** — gaussian count, bounding box,
near-transparent fraction, and front silhouette IoU against the source. It exits
non-zero and names each failure. The degenerate cases it exists to catch:

| signal | meaning |
|---|---|
| `near-transparent` high | the decode mostly produced nothing; the subject was probably not isolated |
| extent degenerate | thinnest axis a fraction of the longest — a flat sheet, not a volume |
| IoU low | the reconstruction is not the thing in the source image |

It uses **propkit's normalisation exactly** — crop to bbox, resize to 512², then
IoU — so numbers from the two tools are comparable. Do not re-derive it.

`compare` answers **"is this better than the art we already have?"** — the source
beside the front render and four orbit views. **`check` passing is not an answer
to this.** Look at the comparison before delivering. If it reads worse than the
sprite, say so; keeping the sprite is a legitimate outcome.

## Post each stage as you finish it

Every command prints a ready-to-paste `link` field, relative to the files dir.
The stages worth showing, because each is a place a run goes wrong invisibly:

| after | show | because |
|---|---|---|
| `upres` | the up-resed plate | a tiny or cropped source dooms everything after it |
| `isolate` | the cutout | surviving background gets reconstructed as geometry |
| `splat` | the turntable | the one artifact that shows the back exists |
| `check` | the IoU line | a number, not an impression |
| `compare` | source beside splat | the only stage that answers "is this better?" |

A `.ply`/`.spz` becomes a chip that opens an orbit viewer, and the turntable
renders inline, so post the turntable *and* link the splat — the video proves it
in the transcript, the chip lets a reviewer turn it themselves.

## The mesh output is a handoff, not a prop

`splat --mesh` also writes a GLB. **It is not deliverable.** Measured: 262k
gaussians meshed at resolution 384 came out a **79 MB** GLB, against propkit tiers
that budget hundreds to a few thousand triangles. It exists to feed:

```
splatkit.py splat 02-cut.png -o 03-splat.ply --mesh
propkit.py  clean 03-splat-mesh.glb --tier oak --game DIR --repair -o 04b.glb --source 02-cut.png
propkit.py  texture 04b.glb --ref 02-cut.png -o 05-prop.glb
propkit.py  check 05-prop.glb --tier oak --game DIR --source 02-cut.png
```

Skip `--mesh` when the splat itself is the deliverable; it costs time and 79 MB
for nothing.

## What you still have to think about

- **Re-run, don't reload.** A saved splat cannot be cheaply loaded back into
  ComfyUI (`Load3D` wants a viewport blob). Generation is deterministic on
  `--seed` and `--decode-seed`, so re-running with the same seeds reproduces the
  file exactly — verified. To change the camera or the turntable, re-run.
- **More gaussians is not more detail.** `--gaussians` oversamples the same
  octree density; 262144 is the model's own default. Raising it costs VRAM and
  time and adds nothing.
- **Ship the `.spz`.** Same content, 3.0 MB against 17.8 MB for the `.ply` on the
  measured oak. Keep the `.ply` as the artifact `check` reads and the viewer
  loads.
- **Never invent the target's numbers.** If the splat is destined for a game via
  the mesh handoff, `--tier` reads height and slot from the game's own source.

## Delivering

Stage-numbered under the project's files dir (the absolute path from your
playbook, never a bare relative path):

```
$FILES/splats/<subject>/00-art.png 01-big.png 02-cut.png 03-splat.ply
                       03-splat.spz 03-splat-turntable.mp4 03-splat-front.png
                       03-splat-manifest.json 06-compare.png
```

`splat` writes the manifest itself: route, seeds, flags and the measured stats. A
splat whose provenance is unrecorded cannot be regenerated or matched by its
siblings.

## The remote fallback

`splatkit.py space IMG -o OUT.ply` drives the `VAST-AI/TripoSplat` HF Space
instead of the local GPU, reading the call contract from the Space's own
`agents.md` rather than from endpoints written down in the tool — so a change
there surfaces as a readable diff instead of a silent break.

Use it only when the local GPU is genuinely unavailable, and **say that you are
doing it**, because it uploads the image to a third-party service. It needs
`$HF_TOKEN` (already synced into `orx up` sessions), it is subject to that Space's
queue and quota, and it returns the splat alone — no turntable, no front mask, so
`check --source` cannot measure IoU on its output. The local route has none of
those limits.
