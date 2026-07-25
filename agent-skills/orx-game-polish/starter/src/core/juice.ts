// Game-feel toolkit — framework-agnostic (only imports three). Trauma-based
// camera shake, hitstop time-dilation, FOV kick, a critically-damped spring,
// haptics, and the easing set. Drive shake()/hitstop() from gameplay events and
// read cameraOffset each frame. This is the "feel" half; CSS covers UI motion.

import * as THREE from 'three';

// --- easings ----------------------------------------------------------------
export const easeOutCubic = (t: number) => 1 - (1 - t) ** 3;
export const easeInCubic = (t: number) => t ** 3;
export const easeOutBack = (t: number) => {
  const c1 = 1.70158, c3 = c1 + 1;
  return 1 + c3 * (t - 1) ** 3 + c1 * (t - 1) ** 2;
};
export const easeOutElastic = (t: number) => {
  const c4 = (2 * Math.PI) / 3;
  return t === 0 ? 0 : t === 1 ? 1 : 2 ** (-10 * t) * Math.sin((t * 10 - 0.75) * c4) + 1;
};
export const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
export const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
/** Frame-rate-independent smoothing — use instead of a raw lerp for camera/UI follow. */
export const damp = (a: number, b: number, lambda: number, dt: number) =>
  lerp(a, b, 1 - Math.exp(-lambda * dt));

// --- critically-damped spring (no overshoot; great for UI + object motion) --
export class Spring {
  value: number;
  target: number;
  private vel = 0;
  constructor(value = 0, private stiffness = 170, private damping = 18) {
    this.value = value;
    this.target = value;
  }
  step(dt: number) {
    const a = this.stiffness * (this.target - this.value) - this.damping * this.vel;
    this.vel += a * dt;
    this.value += this.vel * dt;
    return this.value;
  }
}

// --- haptics ----------------------------------------------------------------
export function haptic(ms = 12) {
  try { (navigator as any).vibrate?.(ms); } catch { /* unsupported */ }
}

// --- the shared feel bus ----------------------------------------------------
class Feel {
  private trauma = 0;          // 0..1; screen shake magnitude squared
  private hitstopT = 0;        // seconds of remaining time-freeze
  private fov = 0;             // additive FOV kick, decays
  readonly cameraOffset = new THREE.Vector3();
  private t = 0;

  /** Add shake. Keep per-hit <= 0.65; it decays automatically. */
  shake(amount = 0.35) { this.trauma = clamp(this.trauma + amount, 0, 1); }
  /** Freeze time briefly for impact (0.05–0.12s typical). */
  hitstop(seconds = 0.07) { this.hitstopT = Math.max(this.hitstopT, seconds); }
  /** Punch the FOV out then let it decay (1.5–3). */
  fovKick(amount = 2) { this.fov += amount; }

  /**
   * Call once per frame with the real dt. Returns the *scaled* dt to advance
   * gameplay with (0 during hitstop), and updates cameraOffset + fovAdd.
   */
  update(dt: number, cam?: THREE.PerspectiveCamera): number {
    this.t += dt;
    // hitstop: consume real dt, hand gameplay a frozen dt
    let gameplayDt = dt;
    if (this.hitstopT > 0) { this.hitstopT -= dt; gameplayDt = 0; }

    // shake: trauma^2 feels better than linear; sum-of-sines noise
    const s = this.trauma * this.trauma;
    const f = 22;
    this.cameraOffset.set(
      s * 0.22 * Math.sin(this.t * f * 1.1 + 0.3),
      s * 0.22 * Math.sin(this.t * f * 1.7 + 1.7),
      0,
    );
    this.trauma = Math.max(0, this.trauma - dt * 1.4);

    // fov kick decays; apply if a camera was passed
    this.fov = Math.max(0, this.fov - this.fov * dt * 26);
    if (cam) { cam.fov = this.baseFov + this.fov; cam.updateProjectionMatrix(); }
    return gameplayDt;
  }

  baseFov = 55;
  get fovAdd() { return this.fov; }
}

export const feel = new Feel();
