# orx game starter

A polished, mobile-first, **vanilla Three.js + Vite** starting point. Scaffolded
into every game build so prototypes start *crafted*, not blank. Distilled from
the ai-asylum house patterns (stoneskipping / spellbook / coin-cellar /
purrfectbounce) plus the two gaps those repos never fixed: real safe-area insets,
`dvh`, and offline-safe fonts.

## Use it
```bash
npm install
npm run dev      # local dev
npm run build    # tsc + vite → dist/  (base:'./' so /play/<expId>/ works)
```
Then **replace the demo gameplay in `src/main.ts`** with your game. Keep the
shell wiring (engine, boot loader, overlay). The `dist/` build is subpath-portable
for the orx play sandbox — do not change `base: './'` in `vite.config.ts`.

## What's here
- `index.html` — mobile shell: `viewport-fit=cover`, canvas → vignette → HUD →
  overlay → loader layers, defined z-ladder.
- `src/style/tokens.css` — **retheme the whole game here** (colour/radius/depth/
  motion tokens, safe-area vars, z-ladder). Components never hardcode values.
- `src/style/ui.css` — reset + component kit: chunky offset-shadow buttons that
  sink on `:active`, panels/cards, meter, chip, overlay, boot loader, popup, and
  the shared motion keyframes (`wobble-in`, `popup-rise`, `ctaPulse`, `shake`).
  `prefers-reduced-motion` respected.
- `src/core/engine.ts` — renderer + camera + fixed-step loop, `preserveDrawingBuffer`
  for share-card snapshots, feel bus wired in.
- `src/core/toon.ts` — banded gradient-ramp toon material + inverted-hull outline
  + one-call `toonLights()`.
- `src/core/juice.ts` — `feel` bus (trauma shake / hitstop / FOV kick), critically-
  damped `Spring`, `haptic()`, and the easing set.
- `src/core/audio.ts` — zero-file WebAudio blips.
- `src/core/sketchify.ts` — hand-drawn wobbly borders on `[data-sketch]` elements.
- `src/core/icons.ts` — tintable icon-mask glyphs (no font, no images).
- `src/ui/hud.ts` — overlay/sheet show-hide, floating `popup()`, `worldToScreen()`,
  `flash()`, `shakeEl()`.

## Conventions
- Everything is **bold** (700–900) and labels get `letter-spacing`. Counters use
  `tabular-nums`.
- Every raised surface = a top→bottom gradient + a **hard, blur-less edge shadow**
  in the darker tone; it collapses on `:active` — that pair is the "physical" feel.
- HUD is `pointer-events:none`; only `.clickable` children take input.
- No CDN. If you want a display font, self-host a `.woff2` + `@font-face` and keep
  a `system-ui` fallback — never a Google Fonts `<link>` (404s offline, stripped
  by ad bundlers).
