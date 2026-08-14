// Which components of an upstream file can we actually take?
//
// The rule this encodes: `ui/src/api.ts` is OUR contract of record and does not
// move. Upstream UI that references an api symbol we don't export is not a
// styling change — it is UI for a backend commit we haven't merged (e.g.
// `toolsFound` is 82ea072's rename of `gitFound`, server-side). Taking it
// compiles only if you also merge api.ts, and merging api.ts is what breaks
// App/ChatPanel/DetailDrawer at once AND disables tsc as the detector.
//
// So: keep api.ts frozen, and keep OUR version of any component that trips this
// check. Then tsc passing is real evidence that no unmerged-backend UI slipped
// in — a test that can actually fail.
//
//     node scripts/port-preflight.mjs ui/src/components/SettingsPage.tsx
//
// Prints the components to keep from HEAD, and why. Run it BEFORE resolving
// conflicts, not after tsc complains.

import { execFileSync } from "node:child_process";
import path from "node:path";

const show = (ref, file) =>
  execFileSync("git", ["show", `${ref}:${file}`], { encoding: "utf8" });

/** Split a source file into top-level `function Name(...) { ... }` blocks by
 *  brace matching. Regex on `^function` gets this wrong on nested functions and
 *  arrow consts — which is exactly how the first attempt produced duplicate
 *  definitions and orphaned components. */
function components(src) {
  const out = [];
  const re = /^(?:export\s+)?function\s+([A-Za-z0-9_]+)/gm;
  let m;
  while ((m = re.exec(src))) {
    const start = m.index;
    const open = src.indexOf("{", re.lastIndex);
    if (open === -1) continue;
    let depth = 0, i = open, inStr = null, prev = "";
    for (; i < src.length; i++) {
      const c = src[i];
      if (inStr) {
        if (c === inStr && prev !== "\\") inStr = null;
      } else if (c === '"' || c === "'" || c === "`") inStr = c;
      else if (c === "{") depth++;
      else if (c === "}") { depth--; if (depth === 0) { i++; break; } }
      prev = c;
    }
    out.push({ name: m[1], start, end: i, text: src.slice(start, i) });
  }
  return out;
}

/** Top-level exported names in `api.ts` at a given ref. */
function apiSymbols(ref) {
  const src = show(ref, "ui/src/api.ts");
  return new Set(
    [...src.matchAll(/^export\s+(?:const|function|type|interface)\s+([A-Za-z0-9_]+)/gm)]
      .map((m) => m[1]),
  );
}

/** Interface field names in `api.ts` at a given ref. Tracked separately from
 *  symbols because they need a stricter usage test: a renamed field like
 *  `gitFound`→`toolsFound` is a genuine hazard, but plenty of field names
 *  (`project`, `empty`, `background`) are also ordinary local variables, and
 *  matching those as bare identifiers flags half the file. */
function apiFields(ref) {
  const src = show(ref, "ui/src/api.ts");
  return new Set([...src.matchAll(/^\s{2}([a-zA-Z][A-Za-z0-9_]*)\??:/gm)].map((m) => m[1]));
}

const file = process.argv[2];
if (!file) {
  console.error("usage: node scripts/port-preflight.mjs <path/to/Component.tsx>");
  process.exit(2);
}

const symHazard = [...apiSymbols("upstream/main")].filter((n) => !apiSymbols("HEAD").has(n));
const ourFields = apiFields("HEAD");
const fieldHazard = [...apiFields("upstream/main")].filter((n) => !ourFields.has(n));

/** Symbols are imported, so a bare identifier match is right. Fields are only a
 *  hazard when *read off a value* (`x.toolsFound`) or *written as a key*
 *  (`toolsFound:`) — a local named `project` is not evidence of anything. */
const usesSymbol = (text, n) => new RegExp(`\\b${n}\\b`).test(text);
const usesField = (text, n) => new RegExp(`\\.${n}\\b|\\b${n}:`).test(text);

const keepOurs = [];
for (const c of components(show("upstream/main", file))) {
  const used = [
    ...symHazard.filter((h) => usesSymbol(c.text, h)),
    ...fieldHazard.filter((h) => usesField(c.text, h)),
  ];
  if (used.length) keepOurs.push({ name: c.name, used: [...new Set(used)].slice(0, 6) });
}

// Self-check. Without this the script is one more confident-looking green light:
// it must flag the rename we know is real, and must not flag on a bare local.
const probe = "const x = a.toolsFound; const project = 1;";
const ok =
  fieldHazard.includes("toolsFound") &&
  usesField(probe, "toolsFound") &&
  !usesField(probe, "project");
if (!ok) {
  console.error("SELF-CHECK FAILED — the hazard test is not detecting what it claims.");
  process.exit(3);
}

console.log(`hazards: ${symHazard.length} symbols, ${fieldHazard.length} fields (self-check ok)`);
console.log(`\ncomponents in ${path.basename(file)} to KEEP FROM HEAD (${keepOurs.length}):`);
for (const k of keepOurs) console.log(`  ${k.name.padEnd(26)} uses ${k.used.join(", ")}`);
if (!keepOurs.length) console.log("  (none — this file can be taken wholesale)");
