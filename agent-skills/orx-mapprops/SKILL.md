---
name: orx-mapprops
description: "Turn 2D art into 3D props for a game map — conditioning render with Z-Image plus ControlNet, form recovery with Depth-Anything-3, then mesh, poly budget and silhouette-variety checks. Use when asked for map props, trees, rocks or structures, when a render is too flat to become a mesh, or when props must vary without breaking one art direction."
---

# Map props: 2D art to 3D geometry

Props are **not characters**. They never deform, so nothing here rigs, and the
topology rules that dominate character meshes (clean joints, separated limbs) do
not apply. What matters is silhouette, poly budget, and holding one art direction
across a varied set.

Two stages, and the first is where runs are lost:

```
2D art or a prompt → CONCEPT RENDER (readable 3D form) → MESH → contract checks
```

## Stage 1: the concept render must contain form

A flat vector illustration cannot become a mesh. Hard outlines and flat colour
fills give an image-to-3D model nothing to back-project, and it returns a cutout
or mush. Measured: a text-to-image conifer came back as stacked silhouettes with
almost no self-shading; the same subject with depth guidance came back with
overlapping tiers, tier-on-tier shade and a modelled trunk.

**Ask for sculpture, not illustration.** Name self-shading, form shadow and
ambient occlusion; forbid `no hard black outline` and `no flat colour fill`.
Ask for no *ground* shadow — but note the model often adds one anyway, so plan to
remove it before the mesh stage rather than trusting the prompt.

### Depth guidance, and the trap that wastes runs

Depth is the strongest control for recovering form. It fails in one specific way,
and the failure looks like a model problem when it is a data problem.

**A raw depth map's background collides with the subject's far side.** Near is
white, far is black — and an unmasked background is also black, so the subject's
deepest parts are indistinguishable from empty space. The generator paints the
whole region as one solid black mass roughly shaped like the subject.

Fix all three, in this order of actual effect:

1. **Mask the background out** — this is the real fix. Force background to 0 and
   remap the subject's depth into a band that starts well above it (e.g. 70–255),
   so the subject's farthest surface can never equal the background.
2. **Normalise for contrast.** A subject may occupy only a slice of the range;
   stretch it. `min_max` over the default.
3. **Strength.** Least important, and the intuitive direction is wrong: lowering
   it crushes interior shadows and flattens the result back toward illustration.
   Move it **up**, inside the model's documented range.

### Canny vs depth — different jobs

| | Canny | Depth |
|---|---|---|
| Locks | silhouette and internal edges | volume and layer ordering |
| Best for | **variants of a known shape** — same outline, different material | **recovering form** from flat art |
| Setup | none; works first try | needs the masking step above |
| Watch | low thresholds capture surface noise as structure, so texture is inherited too | background collision |

Canny is the reliable one. Reach for it when a prop must keep an established
silhouette; reach for depth when the source is too flat to become geometry.

## Stage 2: mesh, and the contract

Never invent the target's numbers — read them from the game, and state a budget
in the plan rather than discovering one later. Props are consumed by a real-time
renderer, so **triangle count and draw calls are part of the deliverable**, not an
afterthought.

Check, per prop:

- **Silhouette still reads as that prop.** Compare against whatever the game
  ships today — an existing billboard atlas slot is a legitimate ground truth when
  no mesh exists yet.
- **Variety is measured, not asserted.** Normalise each variant's silhouette and
  compare pairwise; near-identical after normalisation means one shape wearing
  hats, which fails a variety requirement even though every prop is "different".
- **Height inside the authored band**, origin at the bottom-centre of the opaque
  bounding box, Y-up, one material, zero animation clips.
- **Generated PBR textures are usually discarded.** A stylised game has its own
  palette and light response; a photoreal texture breaks art direction faster than
  a wrong silhouette does.

## Probe, do not trust a list

Model availability on a machine changes in days. A capability doc in this repo
claimed image-to-3D weights were absent and was wrong within 48 hours.

So: **read the live model list and the node list before planning a graph.** Treat
any model name written in a skill or doc as a hint about shape, never as a fact
about presence.

## Two API traps in this ComfyUI

**Enums report as `COMBO`,** so you cannot read valid option values from
`object_info`. Get them from the official workflow templates instead — the
templates are also the correct source for sampler settings and loader types,
which are easy to get subtly wrong and expensive to debug.

**Dynamic combos (`COMFY_DYNAMICCOMBO_V3`) need their nested inputs specified
three ways at once** in the API format: the plain value, the nested inputs as
plain siblings, *and* the same siblings under dotted `parent.child` keys.
Validation demands the dotted form; `execute()` demands the plain one. Supplying
only one side passes validation and then fails at run time, or the reverse. Four
other encodings were tried and all failed one side.

## Delivering

Stage-numbered, under the project's files dir (absolute path from your playbook,
never a bare relative path):

```
$FILES/props/<kind>/00-source.png  01-control.png  02-concept.png
                    03-mesh.glb    04-final.glb    05-manifest.json
```

Link each artifact relative to the files dir — `[mesh](props/oak-a/04-final.glb)`
— because that is what the viewer resolves against. Images and video render inline
in chat; a `.glb` becomes a chip that opens an orbit viewer, so a delivered mesh is
inspectable immediately.

**Record how each artifact was made** in the manifest: model, control type and
strength, sampler settings, seed, and the poly count per LOD. A prop whose
provenance is unrecorded cannot be regenerated or matched by its siblings, which
is exactly what a varied-but-consistent set requires.

## Judging

Measure, then look. A prop that passes every numeric check can still read worse
than the flat billboard it replaces — faked volume on a billboard can be a lot of
invested work, and raw geometry does not automatically beat it. Put them side by
side, in the game, and orbit the camera before declaring a win.
