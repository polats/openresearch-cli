// Crux wordmark: the Southern Cross (constellation Crux) as a clean geometric
// mark + name, sized by the parent's font-size. The tile recolors with the active
// theme (fill = --primary). Single source for the brand lockup.
const STAR = "M0 -9 L2.4 -2.4 L9 0 L2.4 2.4 L0 9 L-2.4 2.4 L-9 0 L-2.4 -2.4 Z";
// four stars of the cross: [x, y, scale]
const STARS: [number, number, number][] = [
  [52, 23, 0.82], // Gacrux (top)
  [49, 79, 1.06], // Acrux (bottom, brightest)
  [25, 49, 0.72], // Imai (left)
  [77, 55, 0.78], // Mimosa (right)
];

export function Wordmark() {
  return (
    <span className="wordmark inline-flex items-center gap-[0.4em] text-text [&_svg]:w-[1em] [&_svg]:h-[1em] [&_svg]:shrink-0">
      <svg viewBox="0 0 100 100" aria-hidden="true">
        <rect width="100" height="100" rx="24" fill="var(--primary)" />
        {/* the cross, drawn faintly through the stars */}
        <g stroke="#fff" strokeWidth="2.4" strokeLinecap="round" opacity="0.4">
          <line x1="52" y1="23" x2="49" y2="79" />
          <line x1="25" y1="49" x2="77" y2="55" />
        </g>
        <g fill="#fff">
          {STARS.map(([x, y, s], i) => (
            <path key={i} transform={`translate(${x} ${y}) scale(${s})`} d={STAR} />
          ))}
        </g>
      </svg>
      Crux
    </span>
  );
}
