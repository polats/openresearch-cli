---
name: orx-animation
description: "Produce character animation locally by either route — ComfyUI image-to-video, or the 3D chain (T-pose, mesh, UniRig, Kimodo) — including the endpoint-conditioning rule that makes a generated loop actually loop, prompting for motion, and judging whether motion is real. Use when asked for a walk cycle, idle, attack or flourish, or when a loop does not loop."
---

# Character animation

You make animated character assets on this machine and deliver them as files the
user can see. Two routes exist. **Pick one deliberately; do not run both by default.**

## Choosing the route

| | Video route (ComfyUI) | 3D route (T-pose → mesh → UniRig → Kimodo) |
|---|---|---|
| Best at | per-character performance, gestures, style fidelity | one motion shared across many characters |
| Cost | ~4–5 min GPU per clip, per character | heavy one-off setup, then cheap per character |
| Fails at | **pose control** — it will not put a limb where you ask | **texturing**, and squat proportions vs SMPL-X |
| Output | frames | a rigged, animated GLB |

Default to the **video route** for one character or a specific performance. Reach
for the **3D route** when the same motion has to serve a roster, or when the
deliverable is a mesh rather than frames.

Check the Generative AI tab before starting: ComfyUI, UniRig and Kimodo each have a
status card, and a route whose backend is down is not a route.

**Free the other backend's model cache when you need the memory.** Running the 3D
route does not mean ComfyUI may hold the GPU while you stall — a cache is housekeeping
and reloads on next use, so reclaiming it is always allowed, and a restriction to one
route never implies otherwise. Only killing a process you did not start is off limits;
if reclaiming everything still leaves too little, report the holders with real numbers
rather than stopping at the first shortfall.

## The rule that decides whether a loop loops

**Pin the endpoints on the pose the loop passes THROUGH, not on rest.**

A first-frame/last-frame video model given `rest → rest` is free to return motion
that looks the same played forwards and backwards, and it will: measured, that gave
**0 leg swaps** and a two-pose bob. Pinning both ends on a **mid-stride contact
pose** removed the resting state it could collapse onto and produced a real cycle
with 2 leg swaps.

Prompting does not substitute for this. The failing run's prompt already said
*"legs alternating forward and back"* verbatim.

Corollary: the same symmetry is **correct** for an idle. An idle genuinely is an
out-and-back to rest, so `idle → idle` is the right conditioning there.

## Prompting for motion

Three parts, in order:

1. **The action, with its phases named and their end-states stated.** "The axe head
   is behind the right shoulder, below head height" holds. "A powerful windup" does
   not.
2. **Explicit prohibitions** — never stops, does not step, does not return to rest.
3. **An invariant block, verbatim every time**: same view, no turning, no rotation,
   same size, no zoom, fixed camera, no panning, stays centred, whole subject inside
   the frame, plain flat magenta background.

Two things models in this class reliably refuse, so design around them rather than
re-rolling: **body recoil** (a "jolt on impact" simply will not appear), and
**raising a weapon overhead** when the source image fills its frame.

**Pad the conditioning image.** If the subject fills its canvas, any gesture that
leaves the body's envelope gets cut off — measured at 63 of 73 frames clipped.
Compositing the subject at ~65% onto a larger background took that to 2.

## Contract fitting and delivery live elsewhere

Frames are not sprites until they satisfy the game's contract — **that is the
`orx-sprite` skill**, which covers reading a tier's numbers from the game, fitting a
whole clip at one scale on a common ground line, and pinhole filling. Do not
re-derive it here.

Where artifacts go and how to link them is in your **playbook** (it holds the files
dir as an absolute path). For animation, name the stages:

```
$FILES/sprites/<class>/00-source.png  01-raw/  02-fit/  03-sheet.png  04-manifest.json
```

**Match the per-frame hold, not the cycle length** when a clip sits beside an
existing one. A 2-frame walk at 7 fps is a 143 ms hold, not a 0.29 s cycle; an
8-frame walk reads correctly at 7 fps and is a blur at 28.

## Judging the result

Measure, then look. Both, in that order, and never only the first.

- **Silhouette difference between consecutive frames** tells you whether motion is
  real or the model redrew a cousin.
- **Connected components** catch a dropped limb or a detached weapon — one frame in
  73 had the axe head floating, and it passed every other check.
- **Frame-to-frame extent** (width, height, top row) tells you whether a gesture
  actually left the body's envelope.

Then put it beside the art it replaces and look at both. A frame that passes every
rule can still be visibly worse than what ships, and that is the only test that
matters.
