---
name: orx-mapprops
description: "Turn a game's 2D sprite art into a textured 3D map prop with sprite-tools/propkit.py: atlas, upres, isolate, then multi-view reconstruction for foliage or single-view for rock and built form, then texture, check and compare. Use for map props: trees, bushes, rocks, walls, houses."
---

# Map props: 2D sprite to 3D prop

`sprite-tools/propkit.py` does the work. Your job is to run the steps in order, pick
the route, and decide whether the result is good enough to keep.

## The steps

Run them in order. Each one's output is the next one's input.

```
1  propkit.py atlas   --game DIR --kind oak -o 00-art.png
2  propkit.py upres   00-art.png -o 01-big.png --refine
3  propkit.py isolate 01-big.png -o 02-cut.png --refine
4  build the geometry — see the route table below
5  propkit.py texture 04-mesh.glb --ref views/cut-0.png -o 05-prop.glb
6  propkit.py check   05-prop.glb --tier oak --game DIR --source 02-cut.png
7  propkit.py compare 05-prop.glb --source 02-cut.png --tier oak --game DIR -o 06-compare.png
```

`propkit.py tiers --game DIR` lists the kinds and what each is allowed to be.
`propkit.py preview 03-prop.glb -o previews/` renders it from four sides.

## Step 2 is not optional

**A shipped sprite is tiny — an oak is 52×64 pixels. Never plan a prop from it.**
Every later step reads the silhouette and the palette from this image, and a prop
planned from 64 rows of chunky pixels comes out a colourless blob that still passes
every structural check. That has happened. The tool now refuses a source under 384px
and points back here.

Two ways to up-res, and the choice matters:

- **`--refine`** adds form the sprite cannot contain (ESRGAN, then a light diffusion
  pass). Use it for anything with volume — a canopy, a boulder.
- **plain** is integer nearest-neighbour: faithful, palette preserved exactly. Use it
  when the prop must keep the shipped colours precisely.

## Step 4: pick the route by subject

**Foliage — trees, bushes.** A canopy has no back in the source art, so ONE view is
never enough: single-view reconstruction returns flat sheets. Give it four views.

```
propkit.py author    02-cut.png --tier conifer --game DIR -o 03-scaffold.glb
propkit.py views     03-scaffold.glb --kind "conifer tree" -o views/
propkit.py meshviews views/ -o 04-mesh.glb
propkit.py clean     04-mesh.glb --tier conifer --game DIR --repair -o 04b.glb
```

`author` here is **only a camera rig** — a rough primitive mesh whose job is to say
where the four cameras stand. `views` renders it from each yaw and restyles each
render into the game's art, so the reconstructor gets real appearance from angles the
2D art never showed. Measured on the same subject: single view 0.57 IoU, four views
**0.93**.

**Rock, wall, house, built form.** Solid shapes reconstruct from one view — a boulder
matched its source to 0.93 that way.

```
propkit.py mesh  02-cut.png -o raw/
propkit.py clean raw/raw-basic.glb --tier boulder --game DIR --repair -o 04b.glb
```

**Gaussian splat, when asked for by name.** A third route, and deliberately not the
default: see `orx-splat`. It reconstructs real volume from ONE view — 0.91 IoU on the
same oak plate where the single-view mesh route gave 0.57 — so it needs no scaffold
and no four-view rig, and it finishes in about a minute.

```
splatkit.py splat 02-cut.png -o 03-splat.ply --mesh
propkit.py  clean 03-splat-mesh.glb --tier oak --game DIR --repair -o 04b.glb --source 02-cut.png
```

Two reasons it stays opt-in. Its **back is inferred, not observed** — plausible
rather than correct, and no check can tell you which, whereas `views` renders sides
that actually derive from the game's art. And its mesh needs the same `clean` budget
work as any other raw output: the splat mesh arrives at 79 MB. Choose it when someone
asks for a splat, or when the routes above have failed on a subject.

## Post each stage as you finish it

Every command prints a ready-to-paste `link` field — markdown relative to the files
dir, which is what the chat viewer resolves. Paste it in a one-line update rather than
saving everything for the end. The stages worth showing, because each is a place the
run can go wrong invisibly:

| after | show | because |
|---|---|---|
| `atlas` | the extracted sprite | proves you pulled the right kind |
| `upres` | the up-resed plate | a tiny or cropped source dooms everything after it |
| `isolate` | the cutout | a surviving background gets modelled as a box |
| `views` | the four-view sheet | if the sides do not look like the same object, stop |
| `meshviews` / `mesh` | the raw mesh | orbit it before spending a clean + texture on it |
| `texture` | the textured prop | grey or torn colour is visible instantly |
| `compare` | source beside prop | the only stage that answers "is this better?" |

A stage you did not show is a stage the reviewer has to take on trust.

## Steps 5 and 6 ask different questions

`check` answers **"is this a legal asset?"** — budget, watertight, single piece,
right height, origin at the base, silhouette close enough. It exits non-zero and
names each failure. Fix them, or say plainly why one is acceptable.

**Texture with `propkit texture`, never by hand.** The shape models emit geometry
only — no UVs, no materials. `texture` UV-unwraps, renders the mesh from six cameras,
paints each view with the Hunyuan3D paint model and bakes one texture, so every side
is coloured. Projecting the source image onto the mesh instead colours the front and
leaves the sides grey.

`compare` answers **"is this better than what we already have?"** — the prop beside
the sprite at the size the game draws it. **`check` passing is not an answer to
this.** A prop can be geometrically perfect and visually useless. Look at the
comparison before delivering; if it reads worse than the sprite, say so. The honest
outcome is sometimes to keep the billboard.

## What you still have to think about

- **Never invent the target's numbers.** `--tier` reads height, slot and pixel
  density from the game's own source. Don't pass remembered values.
- **Colour must survive to the end.** The delivered prop needs more than one
  material, drawn from the sprite's palette. Chaining `clean` after `author` without
  `--source` throws the palette away.
- **Variety is measured, not asserted.** Several props from one shape rescaled is not
  variety. Compare silhouettes pairwise; near-identical after normalisation means one
  shape wearing hats.
- **Model availability changes weekly.** Read the live model list rather than trusting
  any name written down, including here.

## Delivering

Stage-numbered under the project's files dir (the absolute path from your playbook,
never a bare relative path):

```
$FILES/props/<kind>/00-art.png 01-big.png 02-cut.png 03-prop.glb
                    04-compare.png 05-manifest.json
```

Link each artifact **relative to the files dir** — `[prop](props/oak/03-prop.glb)` —
which is what the viewer resolves against. A `.glb` becomes a chip that opens an
orbit viewer, so a delivered prop is inspectable straight from the reply.

Record in the manifest which route you used, the flags you settled on, and `check`'s
output. A prop whose provenance is unrecorded cannot be regenerated or matched by its
siblings.
