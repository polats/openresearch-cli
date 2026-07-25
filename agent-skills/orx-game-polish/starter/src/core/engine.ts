// Minimal Three.js substrate: renderer + scene + perspective camera + a fixed-
// step render loop with resize handling and the feel bus wired in. Register
// per-frame updaters with onFrame(). preserveDrawingBuffer is on so you can
// toDataURL() the canvas for share cards.

import * as THREE from 'three';
import { feel } from './juice';

export type Updater = (dt: number, t: number) => void;

export class Engine {
  readonly renderer: THREE.WebGLRenderer;
  readonly scene = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  private updaters: Updater[] = [];
  private last = 0;
  private t = 0;
  private raf = 0;

  constructor(mount: HTMLElement, opts: { fov?: number; bg?: number } = {}) {
    const fov = opts.fov ?? 55;
    feel.baseFov = fov;
    this.renderer = new THREE.WebGLRenderer({
      antialias: true,
      alpha: false,
      preserveDrawingBuffer: true, // enables share-card snapshots
    });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    mount.appendChild(this.renderer.domElement);

    if (opts.bg !== undefined) this.scene.background = new THREE.Color(opts.bg);

    this.camera = new THREE.PerspectiveCamera(fov, 1, 0.1, 200);
    this.camera.position.set(0, 3, 8);
    this.camera.lookAt(0, 0, 0);

    this.resize();
    window.addEventListener('resize', this.resize);
  }

  onFrame(fn: Updater) { this.updaters.push(fn); }

  private resize = () => {
    const w = window.innerWidth, h = window.innerHeight;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  };

  start() {
    this.last = performance.now();
    const tick = (now: number) => {
      this.raf = requestAnimationFrame(tick);
      const dt = Math.min(0.05, (now - this.last) / 1000);
      this.last = now;
      const gdt = feel.update(dt, this.camera); // 0 during hitstop
      this.t += gdt;
      for (const u of this.updaters) u(gdt, this.t);
      // apply shake as a transient camera offset
      this.camera.position.add(feel.cameraOffset);
      this.renderer.render(this.scene, this.camera);
      this.camera.position.sub(feel.cameraOffset);
    };
    this.raf = requestAnimationFrame(tick);
  }

  stop() { cancelAnimationFrame(this.raf); }
}
