#!/usr/bin/env node
// Offline unit test of the deterministic evaluation core — NO network/LLM.
// Proves the reproducible heart: signal → KPI alignment, comparables, confidence.

import assert from "node:assert/strict";
import fs from "node:fs";
import {
  SIGNALS, KPIS, CLS, computeEvaluationCore,
} from "./eval-core.mjs";
import { GAME_SIGNAL_PROFILES } from "./eval-data.mjs";

let passed = 0;
const ok = (label) => { console.log(`  ✓ ${label}`); passed++; };

// Load the Skybloom coding fixture and normalize to P/A/U.
const raw = JSON.parse(fs.readFileSync(new URL("./fixtures/skybloom.coding.json", import.meta.url)));
const norm = { present: "P", absent: "A", unresolved: "U" };
const codes = {};
for (const s of SIGNALS) codes[s] = norm[String(raw[s]).toLowerCase()] ?? "U";
codes.game_age_gte_2yr = "U";

const core = computeEvaluationCore(codes, GAME_SIGNAL_PROFILES);

// 1. Structure.
for (const k of KPIS) {
  assert.ok(core.kpis[k].discriminating, `${k} has discriminating`);
  assert.ok(core.kpis[k].anti_patterns, `${k} has anti_patterns`);
  assert.ok(core.kpis[k].table_stakes, `${k} has table_stakes`);
  assert.ok(Array.isArray(core.kpis[k].comparables), `${k} comparables is array`);
  assert.ok(core.kpis[k].comparables.length <= 5, `${k} comparables ≤ 5`);
}
ok("record structure for all 3 KPIs");

// 2. Confidence = count of resolved (non-Unresolved) signals / 35.
const resolvedExpected = SIGNALS.filter((s) => codes[s] !== "U").length;
assert.equal(core.confidence.resolved, resolvedExpected, "confidence.resolved matches count");
assert.equal(core.confidence.total, 35, "confidence.total == 35");
assert.equal(core.confidence.share, Math.round((resolvedExpected / 35) * 1000) / 1000, "confidence.share rounded");
ok(`confidence = ${core.confidence.resolved}/35 (${core.confidence.share})`);

// 3. Alignment fraction independently recomputed: present / (present+absent)
//    among the KPI's GRADIENT (discriminating) signals.
for (const kpi of KPIS) {
  const disc = SIGNALS.filter((s) => CLS[kpi][s] === "G");
  const present = disc.filter((s) => codes[s] === "P").length;
  const absent = disc.filter((s) => codes[s] === "A").length;
  const total = present + absent;
  const expected = total ? Math.round((present / total) * 1000) / 1000 : null;
  assert.equal(core.alignment[kpi], expected, `${kpi} alignment = ${expected}`);
  assert.equal(core.kpis[kpi].discriminating.present, present, `${kpi} discriminating.present`);
  assert.equal(core.kpis[kpi].discriminating.total, total, `${kpi} discriminating.total`);
}
ok("alignment fractions match independent recompute");

// 4. Mean = mean of the non-null per-KPI fractions.
const fracs = KPIS.map((k) => core.alignment[k]).filter((v) => v !== null);
const meanExpected = fracs.length ? Math.round((fracs.reduce((a, b) => a + b, 0) / fracs.length) * 1000) / 1000 : null;
assert.equal(core.alignment.mean, meanExpected, "mean alignment matches");
ok(`mean alignment = ${core.alignment.mean}`);

// 5. Comparables are sorted desc by similarity and reference real profile games.
const games = new Set(GAME_SIGNAL_PROFILES.map((p) => p.game));
for (const kpi of KPIS) {
  const c = core.kpis[kpi].comparables;
  for (let i = 1; i < c.length; i++) assert.ok(c[i - 1].similarity >= c[i].similarity, `${kpi} comparables sorted`);
  for (const x of c) assert.ok(games.has(x.game), `${kpi} comparable "${x.game}" is a real profile`);
}
ok("comparables sorted & reference real profiles");

// 6. Determinism — same input, same output.
const again = computeEvaluationCore(codes, GAME_SIGNAL_PROFILES);
assert.deepEqual(again, core, "deterministic: identical output on re-run");
ok("deterministic re-run");

console.log(`\n${passed} checks passed.`);
console.log(`Skybloom alignment → retention=${core.alignment.retention} mau=${core.alignment.mau} revenue=${core.alignment.revenue} mean=${core.alignment.mean}`);
