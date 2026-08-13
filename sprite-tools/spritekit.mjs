#!/usr/bin/env node
/**
 * spritekit — fit a rendered frame to a game's pixel-sprite contract, and check
 * that it holds.
 *
 * The whole point of this tool is that it does **not** reimplement the maths. It
 * imports the target repo's own `pixel.mjs` by absolute path and calls it
 * unmodified, so "passes spritekit" and "passes the game's importer" cannot drift
 * apart. Node resolves that module's own dependencies (pngjs, jpeg-js) from *its*
 * tree rather than ours, which is why this file has no package.json and no
 * install step.
 *
 * Two subcommands:
 *
 *   check <image> --meta <meta.json> [--frame 96] [--figure 82] [--ground 3]
 *   fit <input> <output> --meta <meta.json> [--outline] [...]
 *
 * `--json` makes either machine-readable. Exit status is 0 on pass, 1 on fail,
 * 2 on a usage or IO error — so a caller can branch without parsing anything.
 */

import { pathToFileURL } from "node:url";
import fs from "node:fs";
import path from "node:path";

// --- contract defaults ----------------------------------------------------------
// Mirrors the target game's tools/bakery/map48.mjs. Defaults, not hardcoded truth:
// every one is overridable, because a second game would have different numbers.
const DEFAULTS = {
  frame: 96, // MAP
  figure: 82, // FIGURE_H — round(MAP * 82/96)
  ground: 3, // GROUND_ROWS — empty rows under the feet for the contact-AO decal
  maxColors: 20, // MAX_MAP_COLORS
  // planFit snaps to a clean integer downscale factor, so the figure lands at or
  // just under the target. The shipped roster spans 79-82 against a target of 82.
  figureTolerance: 4,
  paletteKey: "mapPalette",
};

function usage(msg) {
  if (msg) process.stderr.write(`spritekit: ${msg}\n\n`);
  process.stderr.write(
    `usage:\n` +
      `  spritekit check <image> --pixel <pixel.mjs> --meta <meta.json> [options]\n` +
      `  spritekit fit <input> <output> --pixel <pixel.mjs> --meta <meta.json> [options]\n\n` +
      `options:\n` +
      `  --pixel PATH        the target repo's pixel.mjs (required)\n` +
      `  --meta PATH         meta.json holding the palette\n` +
      `  --palette-key KEY   which key to read (default: ${DEFAULTS.paletteKey})\n` +
      `  --palette a,b,c     hex colours instead of --meta\n` +
      `  --frame N           frame size, square (default: ${DEFAULTS.frame})\n` +
      `  --figure N          figure height in px (default: ${DEFAULTS.figure})\n` +
      `  --ground N          empty rows below the feet (default: ${DEFAULTS.ground})\n` +
      `  --max-colors N      distinct colour cap (default: ${DEFAULTS.maxColors})\n` +
      `  --figure-tolerance N  how far under --figure is allowed (default: ${DEFAULTS.figureTolerance})\n` +
      `  --chroma r,g,b      key this colour out first (default: none; alpha is kept)\n` +
      `  --outline           add a 1px palette[0] sel-out border (fit only)\n` +
    `  --allow-detached    skip the single-piece rule (battle tier ships some)\n` +
      `  --json              machine-readable output\n`,
  );
  process.exit(2);
}

const BOOL_FLAGS = new Set(["json", "outline", "allow-detached"]);

function parseArgs(argv) {
  const out = { _: [], flags: {} };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) {
      out._.push(a);
      continue;
    }
    const key = a.slice(2);
    // Bare booleans take no value; everything else consumes the next token. A flag
    // missing from this set silently eats the following argument, so anything new
    // that takes no value must be added here.
    if (BOOL_FLAGS.has(key)) out.flags[key] = true;
    else out.flags[key] = argv[++i];
  }
  return out;
}

const hexToRgb = (h) => {
  const s = String(h).trim().replace(/^#/, "");
  if (!/^[0-9a-fA-F]{6}$/.test(s)) throw new Error(`bad hex colour: ${h}`);
  return [0, 2, 4].map((i) => parseInt(s.slice(i, i + 2), 16));
};
const rgbToHex = ([r, g, b]) =>
  "#" + [r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("");

/**
 * Read the contract from the target repo's own constants instead of retyping them.
 *
 * This exists because retyping them went wrong. The defaults above were set to the
 * STAGING png sizes (96), the committed modules ship 48, and I then guessed the
 * battle tier's numbers and got three of them wrong. Every one of those numbers is
 * already exported by the bakery:
 *
 *   map48.mjs   MAP=96  LOD=48  FIGURE_H  GROUND_ROWS
 *   poses.mjs   BATTLE=160
 *
 * `--tier map|lod|battle` derives frame/figure/ground from those exports, so the
 * tool cannot drift from the game the way I did. Explicit flags still win, but a
 * conflict is reported rather than silently accepted — see checkTierConflict.
 */
async function loadTier(flags) {
  const tier = flags.tier;
  if (!tier) return null;
  if (!flags.bakery) usage("--tier needs --bakery /path/to/tools/bakery");
  const dir = path.resolve(flags.bakery);
  const imp = async (f) => import(pathToFileURL(path.join(dir, f)).href);
  if (tier === "map" || tier === "lod") {
    const m = await imp("map48.mjs");
    // The LOD is defined as an exact halving of the shipping tier, so its numbers
    // are derived from MAP rather than declared — that is what stops them drifting.
    const k = tier === "lod" ? m.LOD / m.MAP : 1;
    return {
      frame: Math.round(m.MAP * k),
      // FIGURE_H is the fit TARGET, and it is also the top of the accept band:
      // planFit's integer-factor snapping lands at or just under it. The tolerance
      // and the ground floor are measured across the 25 shipped sprites of this
      // tier (map 79-82 / 5-6 rows, lod 39-42 / 2-3), not assumed — confusing the
      // target with the band is what made 12 of 25 map sprites "fail" a check they
      // define.
      // The FRAME is structural — LOD is defined as an exact halving of MAP, so it
      // is derived. The FIGURE band is not: halving an 82px bounding box lands on
      // 41 or 42 depending on grid alignment, so round(FIGURE_H/2) rejects half the
      // shipped LOD sprites. Bands below are measured across the 25 shipped files
      // of each tier (map 79-82 / 5-6 rows, lod 39-42 / 2-3 rows). Re-measure if a
      // tier is re-baked; do not re-derive them from FIGURE_H.
      figure: tier === "lod" ? 42 : m.FIGURE_H,
      figureTolerance: 3,
      ground: tier === "lod" ? 2 : 5,
      paletteKey: "mapPalette",
      source: `map48.mjs MAP=${m.MAP} FIGURE_H=${m.FIGURE_H} GROUND_ROWS=${m.GROUND_ROWS}` +
        (tier === "lod" ? ` scaled by LOD/MAP=${m.LOD}/${m.MAP}` : ""),
    };
  }
  if (tier === "battle") {
    const p = await imp("poses.mjs");
    // Battle has no exported figure/ground; these are measured from the 25 shipped
    // battle.png (figure 144-149, ground exactly 3) and are recorded here as
    // measurements, not guesses. Re-derive them if the tier is re-baked.
    return {
      frame: p.BATTLE,
      // Measured across the 25 shipped battle.png: figure 144-149, ground exactly 3.
      figure: 149,
      ground: 3,
      paletteKey: "palette",
      figureTolerance: 5,
      allowDetached: true,
      source: `poses.mjs BATTLE=${p.BATTLE}; figure/ground measured from 25 shipped battle.png`,
    };
  }
  usage(`unknown --tier ${tier} (want map, lod or battle)`);
}

/** An explicit flag that contradicts the tier is a mistake worth stopping for. */
function checkTierConflict(tier, flags) {
  if (!tier) return;
  const conflicts = [];
  for (const [flag, key] of [["frame", "frame"], ["figure", "figure"], ["ground", "ground"]]) {
    if (flags[flag] !== undefined && Number(flags[flag]) !== tier[key]) {
      conflicts.push(`--${flag} ${flags[flag]} but ${tier.source} gives ${tier[key]}`);
    }
  }
  if (conflicts.length) {
    process.stderr.write(
      `spritekit: --tier ${flags.tier} disagrees with explicit flags:\n` +
        conflicts.map((c) => `  ${c}\n`).join("") +
        `drop the flag, or drop --tier if you mean to override the game's own numbers.\n`,
    );
    process.exit(2);
  }
}

/** Load the palette as [r,g,b] triples, from --meta or --palette. */
function loadPalette(flags) {
  if (flags.palette) return String(flags.palette).split(",").map(hexToRgb);
  if (!flags.meta) usage("need --meta or --palette");
  const meta = JSON.parse(fs.readFileSync(flags.meta, "utf8"));
  const key = flags["palette-key"] ?? DEFAULTS.paletteKey;
  const pal = meta[key];
  if (!Array.isArray(pal) || !pal.length) {
    throw new Error(
      `${flags.meta} has no usable "${key}" (keys: ${Object.keys(meta).join(", ")})`,
    );
  }
  return pal.map(hexToRgb);
}

/**
 * Import the target's pixel.mjs.
 *
 * Absolute path → file URL, because a bare relative import would resolve against
 * *this* file and a plain path is not a valid ESM specifier on all platforms.
 */
async function loadPixel(flags) {
  if (!flags.pixel) usage("need --pixel /path/to/pixel.mjs");
  const abs = path.resolve(flags.pixel);
  if (!fs.existsSync(abs)) throw new Error(`no pixel.mjs at ${abs}`);
  return import(pathToFileURL(abs).href);
}

// --- the contract ---------------------------------------------------------------

/**
 * Every rule the game's importer enforces, checked in one pass.
 *
 * Reported as a list rather than a bool because the fixes differ: an off-palette
 * colour is a quantisation problem, a wrong figure height is a framing problem,
 * and partial alpha is a render-settings problem. Collapsing them into "invalid"
 * would throw away the only information a caller can act on.
 */
/** Opaque blobs below this are outline crumbs, not a detached limb. */
const STRAY_MIN = 12;

/** Sizes of every 4-connected opaque component, unsorted. */
function componentSizes(img) {
  const w = img.width;
  const h = img.height;
  const seen = new Uint8Array(w * h);
  const on = (i) => img.data[i * 4 + 3] >= 128;
  const sizes = [];
  const stack = [];
  for (let s = 0; s < w * h; s++) {
    if (seen[s] || !on(s)) continue;
    let n = 0;
    stack.length = 0;
    stack.push(s);
    seen[s] = 1;
    while (stack.length) {
      const p = stack.pop();
      n++;
      const x = p % w;
      const y = (p / w) | 0;
      if (x + 1 < w && !seen[p + 1] && on(p + 1)) { seen[p + 1] = 1; stack.push(p + 1); }
      if (x > 0 && !seen[p - 1] && on(p - 1)) { seen[p - 1] = 1; stack.push(p - 1); }
      if (y + 1 < h && !seen[p + w] && on(p + w)) { seen[p + w] = 1; stack.push(p + w); }
      if (y > 0 && !seen[p - w] && on(p - w)) { seen[p - w] = 1; stack.push(p - w); }
    }
    sizes.push(n);
  }
  return sizes;
}

function checkContract(P, img, palette, opts) {
  const { frame, figure, ground, maxColors, figureTolerance } = opts;
  const rules = [];
  const add = (name, ok, detail) => rules.push({ rule: name, ok, detail });

  add(
    "dimensions",
    img.width === frame && img.height === frame,
    `${img.width}x${img.height}, want ${frame}x${frame}`,
  );

  // Binary alpha: import.mjs throws on any a that is neither 0 nor 255, because
  // the indexed form cannot represent partial coverage.
  let partial = 0;
  for (let i = 3; i < img.data.length; i += 4) {
    const a = img.data[i];
    if (a !== 0 && a !== 255) partial++;
  }
  add("binary-alpha", partial === 0, partial ? `${partial} partial-alpha px` : "");

  // Colour census over opaque pixels only — transparent RGB is arbitrary.
  const seen = new Map();
  for (let i = 0; i < img.data.length; i += 4) {
    if (img.data[i + 3] < 128) continue;
    const rgb = [img.data[i], img.data[i + 1], img.data[i + 2]];
    seen.set(rgbToHex(rgb), (seen.get(rgbToHex(rgb)) ?? 0) + 1);
  }
  add(
    "max-colors",
    seen.size <= maxColors,
    `${seen.size} distinct, cap ${maxColors}`,
  );

  const allowed = new Set(palette.map(rgbToHex));
  const stray = [...seen.keys()].filter((h) => !allowed.has(h));
  add(
    "in-palette",
    stray.length === 0,
    stray.length ? `off-palette: ${stray.slice(0, 6).join(" ")}` : "",
  );

  // Framing. opaqueBounds is the game's own notion of where the figure is, so the
  // measurement matches what the renderer will anchor against.
  // Both of these are BOUNDS, not exact values, and getting that wrong is the
  // first bug this tool had. `planFit` derives maxH = frame - baselinePad, so
  // `ground` is a floor on the rows below the feet — the shipped roster runs 5–6,
  // not 3. And its "prefer a clean integer factor" snapping lands the figure at or
  // just under the target: the roster runs 79–82 against a target of 82. Demanding
  // equality on either would fail every sprite the game itself ships.
  const b = P.opaqueBounds(img);
  const rowsBelow = frame - (b.y + b.h);
  add(
    "figure-height",
    b.h <= figure && b.h >= figure - figureTolerance,
    `${b.h}px, want ${figure - figureTolerance}..${figure}`,
  );
  add("ground-rows", rowsBelow >= ground, `${rowsBelow} rows below feet, need >= ${ground}`);

  // A sprite is one connected blob. A generated frame where the model dropped a
  // limb or let a weapon head float free reads as an obvious defect but passes
  // every rule above — the palette, alpha, colour count and framing are all still
  // correct. Found this the hard way: one frame in 73 had the axe head detached
  // and it went into a sheet unnoticed. Small specks are ignored because the
  // outline pass can legitimately leave a stray pixel at a thin extremity.
  // Not universal: the shipped BATTLE tier legitimately has detached elements
  // (mage's orb, ninja's thrown weapon), so 2 of its 25 sprites fail this rule by
  // design. It holds for all 25 at map scale, where the figure is one silhouette.
  // Default on, because a generated frame that breaks apart is nearly always a
  // defect; --allow-detached turns it off for tiers where it is intentional.
  if (!opts.allowDetached) {
    const blobs = componentSizes(img).sort((a, c) => c - a);
    const detached = blobs.slice(1).filter((n) => n >= STRAY_MIN);
    add(
      "single-piece",
      detached.length === 0,
      detached.length ? `${detached.length} detached blob(s): ${detached.join("+")}px` : "",
    );
  }

  return {
    pass: rules.every((r) => r.ok),
    rules,
    colors: seen.size,
    bounds: b,
  };
}

/** 1px sel-out border grown into the transparent side, in palette[0]. */
function outline(P, img, colour) {
  const { width: w, height: h } = img;
  const out = P.cloneImage(img);
  const opaque = (x, y) =>
    x >= 0 && y >= 0 && x < w && y < h && img.data[(y * w + x) * 4 + 3] > 0;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (img.data[i + 3] > 0) continue;
      if (opaque(x - 1, y) || opaque(x + 1, y) || opaque(x, y - 1) || opaque(x, y + 1)) {
        out.data[i] = colour[0];
        out.data[i + 1] = colour[1];
        out.data[i + 2] = colour[2];
        out.data[i + 3] = 255;
      }
    }
  }
  return out;
}

/**
 * Render → sprite, using the target's own chain.
 *
 * With `--outline`, the fit targets two pixels short so the border lands the
 * figure back on `figure` exactly — the outline grows the silhouette by 1px per
 * side, and doing this any other way fails the framing rules by construction.
 */
function fit(P, img, palette, opts) {
  const { frame, figure, ground, chroma, withOutline, figureTolerance } = opts;
  let src = img;
  if (chroma) src = P.keyBackground(src, { chroma, tol: 46 });
  src = P.trim(src);

  // `planFit` snaps to a clean integer downscale factor, so the height it
  // returns can overshoot the target — and the outline then adds another pixel
  // per side. A fixed offset therefore misses on some frames. Converge instead:
  // ask for a height, measure what came out, and step the request down until the
  // finished sprite lands in range. Cheap (a handful of resamples) and it makes
  // the result independent of how the source happened to be framed.
  // `planFit` snaps to a clean integer factor srcH/k, so the reachable heights
  // form a ladder with rungs ~srcH/k² apart — at the 128px tier that is a ~10px
  // gap against a 5px tolerance band, and for some source framings no rung falls
  // inside it. Stepping targetH then never converges. Pre-scale the trimmed
  // source to targetH*k so the rung we want lands exactly where we asked, which
  // keeps planFit's integer downscale (the thing that holds the pixel grid) and
  // makes every target reachable.
  const prescale = (im, wantH) => {
    const k = Math.max(1, Math.round(im.height / wantH));
    const h = wantH * k;
    if (h === im.height) return im;
    const w = Math.max(1, Math.round((im.width * h) / im.height));
    const o = P.makeImage(w, h);
    for (let y = 0; y < h; y++) {
      const sy = Math.min(im.height - 1, Math.floor((y * im.height) / h));
      for (let x = 0; x < w; x++) {
        const sx = Math.min(im.width - 1, Math.floor((x * im.width) / w));
        const si = (sy * im.width + sx) * 4;
        const di = (y * w + x) * 4;
        o.data[di] = im.data[si];
        o.data[di + 1] = im.data[si + 1];
        o.data[di + 2] = im.data[si + 2];
        o.data[di + 3] = im.data[si + 3];
      }
    }
    return o;
  };

  const render = (targetH) => {
    const s = prescale(src, targetH);
    const plan = P.planFit(s.width, s.height, frame, frame, {
      heightFrac: targetH / frame,
      baselinePad: withOutline ? ground + 1 : ground,
    });
    let o = P.alphaThreshold(P.renderFit(s, plan, frame, frame).image, 128);
    if (withOutline) o = outline(P, o, palette[0]);
    o = P.remapToPalette(o, palette);
    return P.alphaThreshold(o, 128);
  };

  const lowest = figure - figureTolerance;
  let best = null;
  let targetH = withOutline ? figure - 2 : figure;
  for (let attempt = 0; attempt < 12 && targetH >= lowest - 6; attempt++) {
    const out = render(targetH);
    const h = P.opaqueBounds(out).h;
    if (h <= figure && h >= lowest) return out;
    // Keep the closest-so-far in case nothing lands cleanly, so a caller always
    // gets output plus an honest report rather than nothing.
    const err = h > figure ? h - figure : lowest - h;
    if (best === null || err < best.err) best = { out, err };
    if (h > figure) targetH -= (h - figure);
    else break;              // undershooting: asking for less will not help
  }
  return best ? best.out : render(targetH);
}

// --- entry ----------------------------------------------------------------------

async function main() {
  const { _: pos, flags } = parseArgs(process.argv.slice(2));
  const cmd = pos[0];
  if (!cmd || (cmd !== "check" && cmd !== "fit")) usage(cmd ? `unknown command ${cmd}` : null);

  const P = await loadPixel(flags);
  // The tier must resolve before the palette: map and battle read different keys
  // from meta.json, and getting that wrong is what made 25/25 battle sprites look
  // off-palette when they were not.
  const tier = await loadTier(flags);
  checkTierConflict(tier, flags);
  if (tier && !flags["palette-key"]) flags["palette-key"] = tier.paletteKey;
  const palette = loadPalette(flags);
  const num = (k, d) => (flags[k] === undefined ? d : Number(flags[k]));
  const opts = {
    frame: num("frame", tier ? tier.frame : DEFAULTS.frame),
    figure: num("figure", tier ? tier.figure : DEFAULTS.figure),
    ground: num("ground", tier ? tier.ground : DEFAULTS.ground),
    // Defaults to THIS class's palette size, not the design guideline of 20.
    // `selectMapPalette` treats 20 as a target and `irisTargets` can force extras
    // (see meta.mapAddedColors) — `thief` ships 21. A fixed 20 would fail art the
    // game itself ships, which is the wrong way for a contract checker to be wrong.
    maxColors: num("max-colors", palette.length),
    figureTolerance: num("figure-tolerance", tier?.figureTolerance ?? DEFAULTS.figureTolerance),
    chroma: flags.chroma ? String(flags.chroma).split(",").map(Number) : null,
    withOutline: !!flags.outline,
    allowDetached: !!flags["allow-detached"] || !!tier?.allowDetached,
  };

  if (cmd === "check") {
    if (!pos[1]) usage("check needs an image");
    const img = P.decodeImage(fs.readFileSync(pos[1]));
    const report = checkContract(P, img, palette, opts);
    if (flags.json) {
      process.stdout.write(JSON.stringify({ file: pos[1], ...report }, null, 2) + "\n");
    } else {
      process.stdout.write(`${report.pass ? "PASS" : "FAIL"}  ${pos[1]}\n`);
      for (const r of report.rules) {
        process.stdout.write(
          `  ${r.ok ? "ok  " : "FAIL"} ${r.rule}${r.detail ? `  — ${r.detail}` : ""}\n`,
        );
      }
    }
    process.exit(report.pass ? 0 : 1);
  }

  if (!pos[1] || !pos[2]) usage("fit needs <input> <output>");
  const src = P.decodeImage(fs.readFileSync(pos[1]));
  const out = fit(P, src, palette, opts);
  fs.mkdirSync(path.dirname(path.resolve(pos[2])), { recursive: true });
  fs.writeFileSync(pos[2], P.encodePng(out));

  // Always verify what we just wrote. A fit that silently produces an invalid
  // sprite is worse than one that fails, because it fails later, in the importer.
  const report = checkContract(P, out, palette, opts);
  if (flags.json) {
    process.stdout.write(
      JSON.stringify({ input: pos[1], output: pos[2], ...report }, null, 2) + "\n",
    );
  } else {
    process.stdout.write(`wrote ${pos[2]}  (${report.pass ? "PASS" : "FAIL"})\n`);
    for (const r of report.rules) {
      if (!r.ok) process.stdout.write(`  FAIL ${r.rule} — ${r.detail}\n`);
    }
  }
  process.exit(report.pass ? 0 : 1);
}

main().catch((e) => {
  process.stderr.write(`spritekit: ${e.message}\n`);
  process.exit(2);
});
