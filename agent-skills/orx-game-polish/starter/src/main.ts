// Demo wiring — this is CHROME, not the game. The tap → score loop below is the
// MINIMUM interactivity bar: your real mechanic must be at least this responsive
// (real input → per-frame simulation that moves state → feedback → score → an end
// state). REPLACE the demo mechanic with a richer real loop; do NOT replace it with
// menus. A polished home screen wrapped around a thin/faked loop is a MOCKUP, not a
// game — build the loop first, verify it plays, then theme and add a thin meta layer.
// KEEP the shell wiring (engine, boot loader, overlay, hud helpers).

import * as THREE from 'three';
import { Engine } from './core/engine';
import { toonMaterial, addOutline, toonLights } from './core/toon';
import { feel, haptic } from './core/juice';
import * as sfx from './core/audio';
import { sketchifyAll } from './core/sketchify';
import { icon } from './core/icons';
import {
  setLoadProgress, dismissLoader, showOverlay, hideOverlay,
  popup, worldToScreen, flash, shakeEl,
} from './ui/hud';

const app = document.getElementById('app')!;
const engine = new Engine(app, { fov: 55, bg: 0x0b0e13 });
toonLights(engine.scene);

// --- a demo prop: a toon crystal that bobs and spins ----------------------
const geo = new THREE.IcosahedronGeometry(1.1, 0);
const prop = new THREE.Mesh(geo, toonMaterial(0x6cc4ff));
addOutline(prop, 0.04);
engine.scene.add(prop);

let score = 0;
const scoreEl = document.getElementById('score')!;

engine.onFrame((dt, t) => {
  prop.rotation.y += dt * 0.8;
  prop.rotation.x = Math.sin(t * 0.6) * 0.15;
  prop.position.y = Math.sin(t * 1.4) * 0.15;
});
engine.start();

// --- interaction: tapping the action button scores ------------------------
function tapScore() {
  score += 1;
  scoreEl.textContent = String(score);
  sfx.unlock(); sfx.pickup();
  feel.shake(0.3); feel.hitstop(0.05); feel.fovKick(1.5); haptic(12);
  flash('#6cc4ff', 0.25);
  const s = worldToScreen(prop.position, engine.camera);
  popup('+1', s.x, s.y - 40, 'gold');
  const btn = document.getElementById('actionBtn')!;
  shakeEl(btn);
}
document.getElementById('actionBtn')!.addEventListener('click', tapScore);
document.getElementById('startBtn')!.addEventListener('click', () => { sfx.unlock(); sfx.confirm(); hideOverlay(); });

// mute toggle (demo)
let muted = false;
document.getElementById('muteBtn')!.addEventListener('click', (e) => {
  muted = !muted; (e.currentTarget as HTMLElement).textContent = muted ? '×' : '♪';
});

// decorate the score chip with a coin icon
const coin = icon('coin'); coin.style.marginRight = '4px';
scoreEl.parentElement?.querySelector('.label')?.prepend(coin);

// --- boot: fake a load, then reveal the menu ------------------------------
let p = 0;
const boot = setInterval(() => {
  p = Math.min(1, p + 0.12);
  setLoadProgress(p);
  if (p >= 1) {
    clearInterval(boot);
    setTimeout(() => { dismissLoader(); showOverlay('orx game', 'a polished starting point', 'PLAY'); sketchifyAll(); }, 200);
  }
}, 90);
