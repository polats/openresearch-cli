---
name: orx-game-polish
description: "The house craft standard for polished web games: the vanilla Three.js + Vite mobile-first stack, a copyable src/core toon+juice scaffold, the toon look recipe (gradient ramp + inverted-hull outlines), game-feel constants, the layered-VFX minimum bar, zero-binary procedural assets, and visual QA. Load before building any game variant so it reads as crafted, not basic."
---

A build that compiles is not a game that feels good. Polished web games share a
specific substrate — this is it. Scaffold it **before** gameplay, then hold every
variant to the bar below.

## Default stack

**Vanilla Three.js + TypeScript + Vite, mobile-first portrait.** (Not
react-three-fiber — hand-build the scene.) Frame at **390×844**, a
`PerspectiveCamera(70, 9/16)`, `renderer.setPixelRatio(Math.min(2, devicePixelRatio))`,
touch controls (a thumb-zone joystick / tap). Vite `base: './'` so it serves under
`/play/<expId>/`.

## Scaffold `src/core/` first (the copyable substrate)

Build these four before any gameplay — they're what "juicy" is made of:

- **`core/engine.ts`** — renderer (`outputColorSpace = SRGBColorSpace`, PCFSoft
  shadows), a **camera rig with a shake sub-group**, a per-frame `updater`
  registry, and hit-stop time scaling in the loop.
- **`core/juice.ts`** — **trauma-based screen shake** (store `trauma`, apply
  `shake = trauma²`, decay each frame), **hit-stop** (scale dt to ~0 for
  0.05–0.12s on impact), a **`Spring`** class (critically-damped, for hits/pop —
  never `lerp`), and haptics (`navigator.vibrate`).
- **`core/audio.ts`** — WebAudio **synthesis**, zero audio files (osc + gain
  envelopes for hits/pickups; a tiny noise burst for impacts).
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
Pin colours in a **`src/style/palette.ts`** (tokens only — no hex literals in
builders). For characters, procedural primitive/SDF blobs beat hand-modelling.
Only reach for authored art when procedural genuinely can't carry it, and
generate it from **text prompts** (never commit large binaries into an experiment
branch).

## Verify visually, not by types

A variant isn't done because it type-checks. **Screenshot the running build in a
390×844 portrait viewport** (the dashboard Play surface, or headless Chrome) and
look at it before you call it ready — check the toon read, the outlines, that
effects are layered and the feel constants land. Fix what looks flat.
