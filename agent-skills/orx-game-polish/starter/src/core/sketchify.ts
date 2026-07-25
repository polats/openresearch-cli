// Hand-drawn border: trace a wobbly rounded-rect into a <canvas> overlaid on any
// element, re-reading its size via ResizeObserver. Opt in with data-sketch="N"
// (N seeds the wobble so each element differs) and call sketchifyAll() once.
// Purely decorative and pointer-events:none — never blocks input.

function noise(seed: number, i: number): number {
  // cheap layered-sine pseudo-noise, deterministic per (seed,i)
  const a = Math.sin(seed * 12.9898 + i * 78.233) * 43758.5453;
  return (a - Math.floor(a)) * 2 - 1;
}

export function sketchify(el: HTMLElement, seed = 1, color = '#171019', lineWidth = 2.5) {
  const canvas = document.createElement('canvas');
  canvas.className = 'sketch-canvas';
  el.style.position ||= 'relative';
  el.appendChild(canvas);
  const ctx = canvas.getContext('2d')!;

  const draw = () => {
    const w = el.clientWidth, h = el.clientHeight;
    if (!w || !h) return;
    const dpr = Math.min(window.devicePixelRatio, 2);
    canvas.width = w * dpr; canvas.height = h * dpr;
    canvas.style.width = w + 'px'; canvas.style.height = h + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const cs = getComputedStyle(el);
    const r = Math.min(parseFloat(cs.borderTopLeftRadius) || 12, w / 2, h / 2);
    const pad = lineWidth + 1;
    const wob = 1.6; // wobble amplitude in px
    ctx.strokeStyle = color;
    ctx.lineWidth = lineWidth;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';

    // sample the rounded-rect perimeter, jitter each point along its normal
    const pts: [number, number][] = [];
    const N = 120;
    for (let i = 0; i <= N; i++) {
      const t = i / N;
      const [x, y] = roundedRectPoint(t, pad, pad, w - pad * 2, h - pad * 2, r);
      const j = noise(seed, i) * wob;
      pts.push([x + j, y + j]);
    }
    ctx.beginPath();
    ctx.moveTo(pts[0][0], pts[0][1]);
    for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]);
    ctx.stroke();
  };

  const ro = new ResizeObserver(draw);
  ro.observe(el);
  draw();
  return () => { ro.disconnect(); canvas.remove(); };
}

// param t in [0,1] → point on a rounded rect perimeter
function roundedRectPoint(t: number, x: number, y: number, w: number, h: number, r: number): [number, number] {
  const straight = 2 * (w - 2 * r) + 2 * (h - 2 * r);
  const arc = 2 * Math.PI * r;
  const total = straight + arc;
  let d = t * total;
  const seg = (len: number) => { if (d <= len) { const u = d / len; d = -1; return u; } d -= len; return -1; };
  let u: number;
  if ((u = seg(w - 2 * r)) >= 0) return [x + r + u * (w - 2 * r), y];
  if ((u = seg(arc / 4)) >= 0) { const a = -Math.PI / 2 + u * (Math.PI / 2); return [x + w - r + Math.cos(a) * r, y + r + Math.sin(a) * r]; }
  if ((u = seg(h - 2 * r)) >= 0) return [x + w, y + r + u * (h - 2 * r)];
  if ((u = seg(arc / 4)) >= 0) { const a = 0 + u * (Math.PI / 2); return [x + w - r + Math.cos(a) * r, y + h - r + Math.sin(a) * r]; }
  if ((u = seg(w - 2 * r)) >= 0) return [x + w - r - u * (w - 2 * r), y + h];
  if ((u = seg(arc / 4)) >= 0) { const a = Math.PI / 2 + u * (Math.PI / 2); return [x + r + Math.cos(a) * r, y + h - r + Math.sin(a) * r]; }
  if ((u = seg(h - 2 * r)) >= 0) return [x, y + h - r - u * (h - 2 * r)];
  { const a = Math.PI + Math.max(0, d / (arc / 4)) * (Math.PI / 2); return [x + r + Math.cos(a) * r, y + r + Math.sin(a) * r]; }
}

/** Sketch every [data-sketch] element on the page. */
export function sketchifyAll(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>('[data-sketch]').forEach((el) =>
    sketchify(el, Number(el.dataset.sketch) || 1),
  );
}
