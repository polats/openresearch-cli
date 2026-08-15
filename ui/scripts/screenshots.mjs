// Visual baseline for the Tailwind port (docs/harness-and-upstream-review.md §5).
//
// The port converts ~6k lines of hand-written CSS into utility classes, and that
// class of migration fails SILENTLY: an invalid or forgotten utility emits
// nothing, so `tsc` and `vite build` both pass on a screen that renders wrong.
// Typechecking is not verification here — comparing pixels is.
//
// Driven by scripts/screenshots.sh, which boots the server against a frozen
// snapshot of the data dir so the same screens render the same way every run.
//
//     scripts/screenshots.sh --snapshot            (once) freeze the fixture
//     scripts/screenshots.sh tier2                 after a tier's changes
//     scripts/screenshots.sh --compare tier1 tier2
//
// Each label is a full capture; compare consecutive ones. The label for the last
// verified state is the reference for the next tier — there is no permanent
// "baseline", because the screen list grows as coverage does and a stale label
// with fewer screens compares as a pile of MISSING rather than a real signal.
//
// Add a screen by adding an entry to SCREENS, not by editing the driver. Steps
// run in order against one page and accumulate, so each entry starts wherever
// the previous one finished.

import { chromium } from "playwright";
import { mkdir, readdir, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";

const URL_BASE = process.env.ORX_SCREENSHOT_URL ?? "http://127.0.0.1:3399";
const VIEWPORT = { width: 1440, height: 900 };
const OUT_ROOT = new URL("../.screenshots/", import.meta.url).pathname;

/** Settle time after an interaction. The dashboard streams over SSE, so
 *  `networkidle` never fires — every wait here is explicit and deliberately
 *  generous; a flaky screenshot is worse than a slow one. */
const SETTLE = 1100;

/** Kill anything that differs run to run. Without this the spinners and shimmer
 *  keyframes alone make every diff dirty. Re-applied after each navigation
 *  because React remounts blow the injected tag away. */
const FREEZE = `*, *::before, *::after {
  animation: none !important;
  transition: none !important;
  caret-color: transparent !important;
}`;

const click = (name) => async (p) => {
  const b = p.getByRole("button", { name, exact: false }).first();
  if ((await b.count()) === 0) return false;
  await b.click();
  await p.waitForTimeout(SETTLE);
  return true;
};

/** For controls whose accessible name isn't what you'd guess. The back-to-grid
 *  button is titled "All projects" but its *content* is the project name, and
 *  content wins for the accessible name — so getByRole finds nothing. */
const clickSelector = (selector) => async (p) => {
  const el = p.locator(selector).first();
  if ((await el.count()) === 0) return false;
  await el.click();
  await p.waitForTimeout(SETTLE * 2);
  return true;
};

/** For affordances that aren't buttons — the project cards on the home grid. */
const clickText = (text) => async (p) => {
  const t = p.getByText(text, { exact: false }).first();
  if ((await t.count()) === 0) return false;
  await t.click();
  await p.waitForTimeout(SETTLE * 2);
  return true;
};

const hover = (selector) => async (p) => {
  const el = p.locator(selector).first();
  if ((await el.count()) === 0) return false;
  await el.hover();
  await p.waitForTimeout(SETTLE);
  return true;
};

/** Wait for the things that finish *after* the DOM settles and would otherwise
 *  land on one run and not the next: webfonts, and any image/media the chat and
 *  gallery panes fetch lazily. This is what makes the transcript screens
 *  reproducible rather than merely usually-the-same. */
async function quiesce(page) {
  await page.evaluate(async () => {
    await document.fonts?.ready;
    await Promise.all(
      [...document.images]
        .filter((i) => !i.complete)
        .map((i) => new Promise((r) => {
          i.addEventListener("load", r, { once: true });
          i.addEventListener("error", r, { once: true });
        })),
    );
  }).catch(() => {});
  await page.waitForTimeout(SETTLE);
}

/** Project in the fixture whose tree is worth photographing. */
const TREE_PROJECT = process.env.ORX_SCREENSHOT_TREE_PROJECT ?? "Gambit Arena Mobile";

const SETTINGS_TABS = [
  "Appearance", "Persona", "Harnesses", "Generative AI",
  "Compute", "Instances", "Environment", "Git", "Storage",
];

// `advisory: true` marks a screen that will not reproduce byte-for-byte. The
// chat transcript and the two side panes render async media and settle into one
// of a couple of layouts depending on what finished first; no amount of waiting
// made them stable. They are still worth capturing — you look at them after a
// tier — they just cannot be an automated gate, so `--compare` reports them
// separately instead of failing on them and training you to ignore the output.
const SCREENS = [
  { name: "project-chat", advisory: true, steps: async () => {} },
  { name: "panel-gallery", advisory: true, steps: click("Gallery") },
  { name: "panel-worktree", advisory: true, steps: click("Worktree") },
  // The experiment tree, reached by backing out to the project grid and opening
  // one that actually has experiments — the fixture's landing project has none,
  // which is why the tree had no pixel coverage until now.
  //
  // Order is load-bearing twice over. These come BEFORE settings, because "All
  // projects" only exists on the project view. And they come before `files`,
  // because the left-rail selection is sticky across projects: clicking Files
  // first means the newly opened project lands on its (empty) Files tab and the
  // tree never renders — which is exactly how the first attempt produced an
  // empty Files screenshot named "experiments-tree".
  { name: "projects-home", steps: clickSelector('button[title="All projects"]') },
  // Advisory for the same reason as the panes: the chat composer is on screen
  // here, and its usage pill resolves async — proven by two runs of identical
  // code differing. The determinism check happened to land the same way twice
  // when these were added, which is why they were gated at first; a screen that
  // passes the check once is not the same as a screen that can't drift.
  { name: "experiments-tree", advisory: true, steps: clickText(TREE_PROJECT) },
  { name: "exp-hover-card", advisory: true, steps: hover(".exp-node") },
  { name: "experiments-table", advisory: true, steps: click("Table") },

  { name: "files", steps: click("Files") },
  { name: "settings", steps: click("Settings") },
  ...SETTINGS_TABS.map((tab) => ({
    name: `settings-${tab.toLowerCase().replace(/\s+/g, "-")}`,
    steps: click(tab),
  })),
];

async function capture(label) {
  const outDir = path.join(OUT_ROOT, label);
  await mkdir(outDir, { recursive: true });

  const browser = await chromium.launch({ channel: "chrome" });
  const page = await browser.newPage({ viewport: VIEWPORT });
  await page.goto(URL_BASE, { waitUntil: "load" });
  await page.waitForTimeout(2500);

  let shot = 0, skipped = 0;
  for (const screen of SCREENS) {
    if ((await screen.steps(page)) === false) {
      console.log(`  skip  ${screen.name} (entry point not on screen)`);
      skipped++;
      continue;
    }
    await quiesce(page);
    await page.addStyleTag({ content: FREEZE }).catch(() => {});
    await page.screenshot({
      path: path.join(outDir, `${screen.name}.png`),
      // Video posters and WebGL canvases decode/paint at their own pace, so their
      // pixels are never reproducible. Masking keeps the surrounding layout —
      // which is the part the port can break — under comparison.
      mask: [page.locator("video"), page.locator("canvas")],
      maskColor: "#ff00ff",
    });
    console.log(`  shot  ${screen.name}`);
    shot++;
  }

  await browser.close();
  console.log(`\n${shot} screens -> ui/.screenshots/${label}/${skipped ? `  (${skipped} skipped)` : ""}`);
  if (skipped) process.exitCode = 1;
}

/** Hash-compare two label dirs. Deliberately not a pixel differ: a changed hash
 *  is the signal to open both PNGs and look. A percentage-difference number
 *  still needs a human to judge, so it would only add a dependency. */
async function compare(a, b) {
  const dirs = [path.join(OUT_ROOT, a), path.join(OUT_ROOT, b)];
  const hash = async (f) =>
    createHash("sha256").update(await readFile(f)).digest("hex").slice(0, 12);
  const [filesA, filesB] = await Promise.all(
    dirs.map(async (d) => new Set((await readdir(d)).filter((f) => f.endsWith(".png")))),
  );

  const advisory = new Set(SCREENS.filter((s) => s.advisory).map((s) => `${s.name}.png`));
  let changed = 0, advisoryChanged = 0;

  for (const f of [...new Set([...filesA, ...filesB])].sort()) {
    const isAdvisory = advisory.has(f);
    const flag = (label) => {
      if (isAdvisory) { advisoryChanged++; console.log(`  ${label}~ ${f}  (advisory — inspect, don't gate)`); }
      else { changed++; console.log(`  ${label}  ${f}`); }
    };
    if (!filesA.has(f)) { flag("ADDED  "); continue; }
    if (!filesB.has(f)) { flag("MISSING"); continue; }
    const [ha, hb] = await Promise.all([hash(path.join(dirs[0], f)), hash(path.join(dirs[1], f))]);
    if (ha === hb) console.log(`  same     ${f}`);
    else if (isAdvisory) { advisoryChanged++; console.log(`  differs~ ${f}  (advisory — inspect, don't gate)`); }
    else { changed++; console.log(`  CHANGED  ${f}   ${a}=${ha}  ${b}=${hb}`); }
  }

  const tail = advisoryChanged ? `  (${advisoryChanged} advisory screen(s) also moved — eyeball those)` : "";
  console.log(changed === 0
    ? `\nno gated screen changed: ${a} == ${b}${tail}`
    : `\n${changed} gated screen(s) differ — open both PNGs side by side before calling the port clean${tail}`);
  process.exitCode = changed === 0 ? 0 : 1;
}

// Not covered yet: the experiment tree and runs table. Their Tree/Table segmented
// control lives in a pane that isn't open on this fixture's landing view, so
// clicking it captured the chat screen under a tree-shaped name — a baseline that
// lies about its coverage is worse than one that admits a gap. Worth wiring up
// before Tier 2 touches TreeView; it needs an entry point that opens that pane.

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === "--compare") {
  if (rest.length !== 2) {
    console.error("usage: screenshots.mjs --compare <labelA> <labelB>");
    process.exit(2);
  }
  await compare(rest[0], rest[1]);
} else {
  const label = cmd ?? "baseline";
  console.log(`capturing "${label}" from ${URL_BASE}`);
  await capture(label);
}
