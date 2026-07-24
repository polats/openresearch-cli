// DOM HUD helpers over the WebGL canvas. Text stays in the DOM for crisp mobile
// rendering. Floating numbers are projected from world space each spawn. One
// reusable #sheet drives every panel; .hidden toggles the rest.

import * as THREE from 'three';

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

// --- boot loader -----------------------------------------------------------
export function setLoadProgress(p: number) {
  const fill = document.getElementById('loadFill');
  if (fill) fill.style.width = `${Math.round(p * 100)}%`;
}
export function dismissLoader() {
  const l = document.getElementById('loading');
  if (!l) return;
  l.classList.add('leave');
  setTimeout(() => l.remove(), 340);
}

// --- overlay (menu / results) ---------------------------------------------
export function showOverlay(title: string, sub = '', btn = 'PLAY', state = '') {
  const o = $('overlay');
  $('overlayTitle').textContent = title;
  $('overlaySub').textContent = sub;
  const b = $('startBtn'); if (b) b.textContent = btn;
  o.className = state; // e.g. 'game-over'; empty = default
  o.classList.remove('hidden');
}
export function hideOverlay() { $('overlay').classList.add('hidden'); }

// --- reusable bottom sheet -------------------------------------------------
export function showSheet(html: string, opts: { dismissable?: boolean } = {}) {
  const sheet = $('sheet');
  const bd = $('sheet-backdrop');
  sheet.innerHTML = html;
  sheet.classList.remove('hidden');
  bd.classList.toggle('hidden', opts.dismissable === false);
  bd.onclick = opts.dismissable === false ? null : hideSheet;
  return sheet;
}
export function hideSheet() {
  $('sheet').classList.add('hidden');
  $('sheet-backdrop').classList.add('hidden');
}

// --- floating score popup --------------------------------------------------
/** Spawn a rising popup at a screen point. cls: '' | 'gold' | 'good' | 'bad'. */
export function popup(text: string, screenX: number, screenY: number, cls = '') {
  const el = document.createElement('div');
  el.className = `popup ${cls}`.trim();
  el.textContent = text;
  el.style.left = `${screenX}px`;
  el.style.top = `${screenY}px`;
  el.style.fontSize = `${1.3 + Math.random() * 0.2}rem`;
  $('floaters').appendChild(el);
  setTimeout(() => el.remove(), 1150);
}

/** Project a world point to screen px (for world-anchored popups/labels). */
export function worldToScreen(v: THREE.Vector3, cam: THREE.Camera): { x: number; y: number } {
  const p = v.clone().project(cam);
  return { x: (p.x * 0.5 + 0.5) * window.innerWidth, y: (-p.y * 0.5 + 0.5) * window.innerHeight };
}

// --- feedback: shake an element (re-triggerable) ---------------------------
export function shakeEl(el: HTMLElement) {
  el.classList.remove('shake');
  void el.offsetWidth; // force reflow so the animation replays
  el.classList.add('shake');
}

// --- one-shot screen flash (double-rAF so the fade always plays) -----------
let flashEl: HTMLDivElement | null = null;
export function flash(color = '#fff', strength = 0.4) {
  if (!flashEl) {
    flashEl = document.createElement('div');
    Object.assign(flashEl.style, {
      position: 'fixed', inset: '0', zIndex: '90', pointerEvents: 'none', opacity: '0',
    } as CSSStyleDeclaration);
    document.body.appendChild(flashEl);
  }
  flashEl.style.background = color;
  flashEl.style.transition = 'none';
  flashEl.style.opacity = String(strength);
  requestAnimationFrame(() => requestAnimationFrame(() => {
    flashEl!.style.transition = 'opacity 0.5s ease-out';
    flashEl!.style.opacity = '0';
  }));
}
