---
name: orx-sprite
description: "Fit rendered frames to a pixel-art game's sprite contract and verify they hold, using that game's own pixel maths. Use when frames must match a shipping sprite tier, when a contract check rejects art the game already ships, or when a clip needs one scale and a common ground line rather than per-frame normalisation."
---

# Sprite contract

A pixel-art game sprite is not "a small PNG". It has a contract the game's
importer enforces, and art that violates it fails at import — not at render time,
where you could still have fixed it.

**Never invent the contract's numbers.** Read them from the target repo, then pass
them as flags. They differ per game and per tier.

## Prerequisite

`spritekit` deliberately does not reimplement the resampling maths. It imports the
**target repo's own pixel module** and calls it unmodified, so "passes spritekit"
and "passes the importer" cannot drift apart.

That module must export: `decodeImage`, `encodePng`, `cloneImage`, `keyBackground`,
`trim`, `planFit`, `renderFit`, `remapToPalette`, `alphaThreshold`, `opaqueBounds`.

If the target repo has no such module, this skill does not apply — say so rather
than approximating, because an approximate contract check is worse than none.

## The command

```bash
node <crux>/sprite-tools/spritekit.mjs check <image> \
  --pixel <repo>/path/to/pixel.mjs --meta <repo>/path/to/meta.json \
  [--frame N] [--figure N] [--ground N] [--max-colors N] [--palette-key KEY]

node <crux>/sprite-tools/spritekit.mjs fit <input> <output> \
  --pixel ... --meta ... [--outline] [--chroma r,g,b]
```

`--json` for machine-readable output. Exit status **0 pass / 1 fail / 2
usage-or-IO error** — branch on the status, don't parse text.

`fit` always re-checks what it wrote. A fit that silently emits an invalid sprite
is worse than one that fails, because it fails later and further away.

## What it checks

| Rule | Meaning |
|---|---|
| `dimensions` | exactly the frame size |
| `binary-alpha` | every alpha is 0 or 255 — indexed formats can't hold partial coverage |
| `max-colors` | no more distinct colours than the class's own palette |
| `in-palette` | every colour appears in the palette from `--meta` |
| `figure-height` | figure height within tolerance of the target |
| `ground-rows` | at least N empty rows below the feet |

## Finding the numbers

Look in the game's bakery/importer for constants like frame size, figure fraction,
ground rows and a colour cap, and for what its importer actually throws on. Prefer
reading the constants over reading documentation about them.

**Then check your reading against the shipped art before trusting it.** Run
`check` over every already-shipped sprite. They must all pass — they are the
ground truth. If they don't, your numbers are wrong, not the art.

## Targets are usually bounds, not equalities

This is the failure mode to expect, and it generalises even though the numbers
don't. Constants that *look* exact are often one-sided:

- A "ground rows" reservation is typically a **floor** — fit code derives a max
  height from it, so real output has that many rows *or more*.
- A figure height is typically a **ceiling** — fit code that snaps to clean integer
  downscale factors lands at or just under the target.
- A colour cap is often a **target** that special cases can exceed (forced
  highlight colours, eyes), so prefer the class's actual palette size.

Demanding equality on any of these will reject art the game itself ships. That is
the worst way for a contract checker to be wrong, because it stops good work.

*(Worked example — the target game's map tier: frame 96, target figure 82, ground 3,
cap 20. Its shipped roster actually measures 79–82 figure height, 5–6 ground rows,
and one class carries 21 colours. All 25 pass with the bounds read correctly.)*

## Producing frames that pass

Renderer-side, and mostly universal to 3D-to-pixel-art:

- **Render natively at the target size.** Never render large and downscale — that
  reintroduces anti-aliasing and lets colours drift between frames.
- **Kill anti-aliasing at the renderer.** In Cycles, filter width ~0.01; a box
  filter alone is not enough. In EEVEE, disable TAA/jitter.
- **Toon shading with a Constant-interpolation colour ramp.** This is the single
  biggest difference between "pixel art" and "a small 3D render".
- **One fixed palette applied identically to every frame.** Per-frame adaptive
  quantisation makes the palette crawl across an animation.
- **No dithering on a looping animation** at low colour counts — it twinkles.
- **`--outline` targets two pixels short** so the 1px border lands the figure back
  on the target height. Don't hand-correct for it.

## When a check fails

Read *which* rule failed; the fixes are unrelated.

- `binary-alpha` → renderer setting (anti-aliasing, or alpha blending left on)
- `in-palette` / `max-colors` → quantisation; re-run `fit`, or the source is off-model
- `figure-height` / `ground-rows` → framing; the camera or the fit options
- `dimensions` → you checked the render instead of the fitted output
