// Zero-file audio: tiny WebAudio synth for UI/impact blips. No assets, no CDN.
// Call unlock() on the first user gesture (browsers require it), then blip()/etc.

let ctx: AudioContext | null = null;
function ac(): AudioContext {
  if (!ctx) ctx = new (window.AudioContext || (window as any).webkitAudioContext)();
  return ctx;
}

export function unlock() { if (ac().state === 'suspended') ac().resume(); }

/** A short shaped tone. type: sine|square|triangle|sawtooth. */
export function tone(freq: number, dur = 0.12, type: OscillatorType = 'triangle', gain = 0.2) {
  const c = ac();
  const o = c.createOscillator();
  const g = c.createGain();
  o.type = type;
  o.frequency.value = freq;
  g.gain.setValueAtTime(0.0001, c.currentTime);
  g.gain.exponentialRampToValueAtTime(gain, c.currentTime + 0.008);
  g.gain.exponentialRampToValueAtTime(0.0001, c.currentTime + dur);
  o.connect(g).connect(c.destination);
  o.start();
  o.stop(c.currentTime + dur + 0.02);
}

export const blip = () => tone(660, 0.09, 'triangle', 0.18);
export const confirm = () => { tone(523, 0.09); setTimeout(() => tone(784, 0.12), 60); };
export const deny = () => tone(160, 0.16, 'sawtooth', 0.15);
export const pickup = () => { tone(880, 0.06, 'square', 0.12); setTimeout(() => tone(1320, 0.08, 'square', 0.1), 40); };
