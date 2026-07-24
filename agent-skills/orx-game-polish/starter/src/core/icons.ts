// Tintable icons without a font or image files: inline single-colour SVGs turned
// into CSS mask-images so they take `currentColor` and scale with font-size.
// Add glyphs to ICONS (path data on a 24x24 viewBox). Usage:
//   el.appendChild(icon('heart'))   or   `<i class="ic">` styled via iconMask.

const ICONS: Record<string, string> = {
  heart: 'M12 21s-7.5-4.9-10-9.3C.6 8.9 2 5.5 5.2 5.5c1.9 0 3 .9 3.8 2 .8-1.1 1.9-2 3.8-2 3.2 0 4.6 3.4 3.2 6.2C19.5 16.1 12 21 12 21z',
  star: 'M12 2l2.9 6.3 6.9.8-5.1 4.7 1.4 6.8L12 17.9 5.9 21.4l1.4-6.8L2.2 9.1l6.9-.8L12 2z',
  coin: 'M12 2a10 10 0 100 20 10 10 0 000-20zm0 4a6 6 0 110 12 6 6 0 010-12zm-1 2v1.2c-1.2.3-2 1.1-2 2.3 0 1.4 1.1 2 2.6 2.4.9.2 1.4.4 1.4.9s-.5.7-1.2.7c-.8 0-1.4-.3-1.6-1H8c.1 1.2 1 2 2 2.2V18h2v-1.2c1.3-.3 2.1-1.1 2.1-2.4 0-1.5-1.2-2.1-2.7-2.5-.9-.2-1.3-.4-1.3-.8s.4-.7 1-.7c.7 0 1.2.3 1.3.9h1.5c-.1-1.1-.9-1.9-2.1-2.1V8h-1z',
  bolt: 'M13 2L4 14h6l-1 8 9-12h-6l1-8z',
  play: 'M8 5v14l11-7L8 5z',
  bag: 'M6 8V6a6 6 0 0112 0v2h2l1 12H3L4 8h2zm2 0h8V6a4 4 0 00-8 0v2z',
  gear: 'M12 8a4 4 0 100 8 4 4 0 000-8zm9 4l-2-1.5.3-2.5-2.4-.8-1.3-2.2L13 4l-1-2-1 2-2.6.2-1.3 2.2-2.4.8.3 2.5L3 12l2 1.5-.3 2.5 2.4.8 1.3 2.2L11 20l1 2 1-2 2.6-.2 1.3-2.2 2.4-.8-.3-2.5L21 12z',
};

function svgDataUri(path: string): string {
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path d='${path}'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

/** Apply icon-mask styles to an existing element (tints to currentColor). */
export function iconMask(el: HTMLElement, name: string) {
  const uri = svgDataUri(ICONS[name] ?? ICONS.star);
  el.style.display = 'inline-block';
  el.style.width = '1em';
  el.style.height = '1em';
  el.style.verticalAlign = '-0.14em';
  el.style.backgroundColor = 'currentColor';
  el.style.setProperty('-webkit-mask', `${uri} center / contain no-repeat`);
  el.style.setProperty('mask', `${uri} center / contain no-repeat`);
}

/** Make a fresh <i> icon element. */
export function icon(name: string): HTMLElement {
  const i = document.createElement('i');
  iconMask(i, name);
  return i;
}
