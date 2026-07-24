---
name: orx-game-polish
description: "The house craft standard for web games that actually PLAY: vanilla Three.js + Vite mobile-first, the committed UI starter + toon/juice substrate, game-feel constants, layered VFX, zero-binary assets, a playable-loop-FIRST mandate (the kit is chrome, not the game — no faked meta), and a playability gate that DRIVES the loop, not just a screenshot. Load before any game build."
---

A build that compiles is not a game that feels good. Polished web games share a
specific substrate — and it is **already written for you**. Scaffold from the
committed starter template **before** gameplay, then hold every variant to the
bar below.

## Scaffold from the committed starter FIRST (do not hand-roll the shell)

A polished, mobile-first, vanilla Three.js + Vite starter ships **bundled with
this skill**, in the `starter/` folder next to this `SKILL.md` (the harness writes
it into your skills dir, e.g. `.claude/skills/orx-game-polish/starter/`). It is
the house UI kit + `src/core` substrate, already built and visually verified.
**Copy it into your project root and build on top of it** — do not re-derive the
shell, tokens, buttons, or juice by hand (that is exactly the copy-paste drift
this template exists to end).

```sh
# from your project worktree root:
STARTER=$(find . ~/.claude ~/.config -type d -path '*orx-game-polish/starter' 2>/dev/null | head -1)
cp -R "$STARTER"/. .          # brings index.html, vite.config.ts, tsconfig.json, src/, package.json
npm install
npm run build                 # tsc + vite → dist/  (base:'./' already set)
```

Then **replace the demo gameplay in `src/main.ts`** with your game; keep the shell
wiring. What you inherit (read the starter's `README.md`):

- **`src/style/tokens.css`** — retheme the whole game here (colour/radius/depth/
  motion tokens, safe-area vars, z-ladder). Never hardcode a hex in a component.
- **`src/style/ui.css`** — the component kit: chunky offset-shadow buttons that
  sink on `:active`, panels/cards, meter, chip, overlay, boot loader, popups, and
  the shared motion keyframes. `prefers-reduced-motion` respected.
- **`src/core/`** — `engine.ts` (loop + resize + `preserveDrawingBuffer`),
  `juice.ts` (`feel` trauma-shake/hitstop/FOV-kick + `Spring` + haptics + easings),
  `toon.ts` (gradient ramp + inverted-hull outline + `toonLights`), `audio.ts`
  (zero-file WebAudio), `sketchify.ts` (hand-drawn borders), `icons.ts` (tintable
  icon masks — no emoji).
- **`src/ui/hud.ts`** — `showOverlay`/`showSheet`, floating `popup()`,
  `worldToScreen()`, `flash()`, `shakeEl()`.

Re-theming an existing prototype? Copy `src/style/*` + `src/core/*` + `src/ui/*`
in and rewire the DOM to the kit's classes — that alone lifts a flat build.

## Build the playable LOOP first — the kit is chrome, not the game

The single most common failure is shipping a **mockup**: a gorgeous home screen
with challenge cards, a festival banner, friend avatars, and a leaderboard — none
of it functional — wrapped around a core mechanic that is thin, broken, or buried
two taps deep. It looks like a game in a screenshot and is not one. Do not do this.

**Order of work, non-negotiable:**

1. **Gray-box the core loop first.** Before any theme, menu, or meta screen, build
   the actual mechanic as ugly boxes: real input → simulation that updates state
   every frame → visible feedback → a **score and an end state** (win/lose/round
   over). It must be **playable for a full session on the first screen**, with no
   menu in the way. Find the fun here. If the gray-box isn't fun, no amount of UI
   saves it — change the mechanic, don't add screens.
2. **Then juice and theme it** (toon look, feel constants, VFX below).
3. **Then add a THIN meta layer.** Retention/organic UI (daily reward, a friend
   challenge, a festival) sits *on top of a loop that already plays* and stays
   small — one real hook, wired to real game state. It is never the bulk of the
   build.

**Honesty rule.** A prototype exists to prove the core loop is fun. **Never fake a
system as if it were real** — no hardcoded "Maya just scored 8,420," no invented
festival progress bar, no fabricated community feed dressed up as functionality.
Faked meta is set-dressing, not a game, and it is the tell of a mockup. If a system
isn't wired to real state, cut it or reduce it to an honest stub.

The starter's `src/main.ts` demo (tap → score → juice) is the **minimum
interactivity bar** — your real loop must be *at least* that responsive. Replace
the demo mechanic with a richer real one; never replace it with menus.

## The UI look (why the starter reads as crafted)

Match these when you add screens; they are already encoded in `ui.css`:

- **Everything is bold** (700–900); uppercase labels get `letter-spacing`;
  counters use `tabular-nums`.
- **Every raised surface** = a top→bottom gradient fill + a **hard, blur-less
  edge shadow** in the darker tone (`box-shadow: 0 6px 0 <edge>`), which
  **collapses on `:active`** while the element `translateY`s down. That pair is
  the entire "physical button" feel — never a flat filled rectangle.
- HUD overlay is `pointer-events:none`; only `.clickable` children take input.
- Anchor every edge element with `env(safe-area-inset-*)`; cap panels with `dvh`.
- **No CDN fonts.** The kit uses a heavy system stack; for a display face,
  self-host a `.woff2` + `@font-face` with a `system-ui` fallback.

## The `src/core/` substrate (what the starter gives you)

You get these already; understand them before extending:

- **`core/engine.ts`** — renderer + camera + fixed-step loop with the `feel` bus
  wired in and `preserveDrawingBuffer` for share-card snapshots.
- **`core/juice.ts`** — **trauma-based screen shake** (`shake = trauma²`, decays),
  **hit-stop** (dt→0 for 0.05–0.12s on impact), a critically-damped **`Spring`**
  (never `lerp` for hits/pops), haptics, and the easing set.
- **`core/audio.ts`** — WebAudio **synthesis**, zero audio files.
- **`core/toon.ts`** — the look (below).

## The toon look recipe (this is the polish)

```ts
// 3-step cel ramp shared by every material
const ramp = new THREE.DataTexture(new Uint8Array([90,90,90, 180,180,180, 255,255,255]), 3,1);
ramp.needsUpdate = true;
const mat = new THREE.MeshToonMaterial({ color, gradientMap: ramp });

// inverted-hull ink outline — clone geo, push verts along normals, BackSide
function addOutline(mesh, color, k = 0.02) {              // k ≈ 2% of bounding radius
  const g = mesh.geometry.clone();
  const o = new THREE.Mesh(g, new THREE.MeshBasicMaterial({ color: darken(color,0.45), side: THREE.BackSide }));
  o.scale.multiplyScalar(1 + k); mesh.add(o);
}
```

- **Lighting rig, fixed:** one warm `DirectionalLight` (PCFSoft shadows) + one
  `HemisphereLight`. **No ambient, no post-processing, no fog.** Fake glow
  *in-scene* — fresnel rim shells + additive glow sprites, not bloom.
- **Draw-call discipline:** merge primitive builders into one mesh
  (`mergeGeometries` / a vertex-colored merged mesh) — many colors, one draw call.

## Game-feel constants (the difference between "works" and "juicy")

- Camera shake ≤ **0.65** trauma per hit; **hit-stop 0.05–0.12s** on impact.
- FOV kick **1.5–3°** on a big action, spring back.
- **Springs, not lerps**, for anything that hits/pops.
- **Squash-and-stretch preserves volume** (scale up one axis → down the others).

## Layered-VFX minimum bar

Never ship a single-mesh effect. Every projectile/impact is **layered**:
`toon core → additive hot inner → 2 glow sprites → spark trail → optional mist`,
and impacts add a **flash + shockwave ring + sparks**. A bare mesh reads as
prototype; the layers read as production.

## Assets: zero-binary, procedural-first

Prefer **no binary assets** — models = merged primitives (box/cyl/sphere/cone),
textures = **canvas painting**, audio = **WebAudio synthesis**. This is the house
style, keeps builds instant, and sidesteps asset-path 404s under `/play/<expId>/`.
Pin CSS colours in **`src/style/tokens.css`** and 3D colours in a small
`src/style/palette.ts` (tokens only — no hex literals in builders). For
characters, procedural primitive/SDF blobs beat hand-modelling.
Only reach for authored art when procedural genuinely can't carry it, and
generate it from **text prompts** (never commit large binaries into an experiment
branch).

## The playability gate — DRIVE it, don't just screenshot it

A screenshot of a pretty menu passes a screenshot check while the game underneath
is a mockup. So a variant is **not done** until you have *exercised the core loop
headlessly and proven state actually moves.* Type-checking and a nice screenshot
are necessary, not sufficient.

Script a headless run (puppeteer/Playwright, 390×844) that:

1. **Reaches the loop** — start a round / get to the first playable screen (no
   human needed). If you can't reach the mechanic in ≤1 automated action, the
   loop is buried — fix that first.
2. **Applies real input** — dispatch the actual gameplay input (pointer drag,
   taps, key) the mechanic listens for, several times.
3. **Asserts state changed** — read the game state before/after and assert it
   *moved*: score increments, entities spawn/despawn, a meter fills, the round
   timer ticks. **A loop where input doesn't change state is a mockup — fail.**
4. **Reaches an end state** — drive it to win/lose/round-over and confirm the
   result screen shows real numbers from play (not static props).
5. **Screenshots MID-PLAY**, not the menu — capture the mechanic in action, and
   check the toon read, outlines, layered VFX, and feel there.

Log the before/after state values in your report so the loop is auditable. If any
of 1–4 fails, it isn't a game yet — fix the mechanic before you touch more UI.
