#!/usr/bin/env node
// FOUNDRY idea evaluator — Evaluation v2 as an orx `--kind sim`.
//
// NORMAL (keyless) FLOW — no API key, no hardcoded provider:
//   The analyst AGENT (whatever harness the session runs on — Claude Code,
//   Codex, OpenCode, …) reads the thesis + the 35 signal definitions and codes
//   the signals in its own turn, writing theses/<slug>.coding.json. This sim
//   then does the pure, deterministic scoring (35-signal → KPI alignment,
//   comparables, confidence) from that coding and writes:
//     • $ORX_METRICS_PATH (or --metrics)  — {aggregate:{…}, record:{…}}
//     • evaluations/<slug>.md             — the deterministic report
//   The agent adds the qualitative read (archetype, strengths/weaknesses,
//   executive) in its turn. See `--print-signals` for the coding rubric.
//
// HEADLESS mode (--llm) — self-contained, for batch runs with no agent in the
//   loop: the sim itself codes signals + writes archetype/prose via a pinned
//   model (llm.mjs). This is the ONLY path that needs an API key.
//
//   node evaluate.mjs --print-signals            # the rubric the agent codes against
//   node evaluate.mjs --slug skybloom            # keyless: score from the sidecar
//   node evaluate.mjs --slug skybloom --llm      # headless: sim does the LLM steps (needs key)

import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import {
  SIGNALS, KPIS, computeEvaluationCore, codeSignals, buildArchetype, summaryPrompt, openaiJson,
} from "./eval-core.mjs";
import { SIGNAL_DEFINITIONS_MD, ARCHETYPE_REFERENCE_MD, GAME_SIGNAL_PROFILES } from "./eval-data.mjs";
import { PINNED_MODEL } from "./llm.mjs";

const log = (m) => process.stderr.write(`[evaluate] ${m}\n`);

const { values: opt } = parseArgs({
  options: {
    thesis: { type: "string" },
    slug: { type: "string" },
    id: { type: "string" },
    coding: { type: "string" },
    "out-dir": { type: "string", default: "evaluations" },
    metrics: { type: "string" },
    llm: { type: "boolean", default: false },
    "print-signals": { type: "boolean", default: false },
  },
});

// The coding rubric the agent works from — provider/harness-agnostic.
function printSignals() {
  const template = Object.fromEntries(SIGNALS.map((s) => [s, "Unresolved"]));
  process.stdout.write(SIGNAL_DEFINITIONS_MD + "\n");
  process.stdout.write(
    "## How to code\n\n" +
    "For each of the 35 signals output exactly one of Present | Absent | Unresolved:\n" +
    "- Present: the thesis explicitly describes or strongly implies it.\n" +
    "- Absent: the thesis contradicts it or describes a design incompatible with it.\n" +
    "- Unresolved: the thesis neither confirms nor rules it out.\n" +
    "Special rules: free_to_play → Present if pricing is unmentioned; game_age_gte_2yr → always Unresolved (new concept).\n\n" +
    "Write the result to theses/<slug>.coding.json with these exact 35 keys:\n\n" +
    JSON.stringify(template, null, 2) + "\n",
  );
}

function findThesis() {
  if (opt.thesis) return opt.thesis;
  const dir = "theses";
  if (opt.slug && fs.existsSync(path.join(dir, `${opt.slug}.md`))) return path.join(dir, `${opt.slug}.md`);
  if (fs.existsSync(dir)) {
    const md = fs.readdirSync(dir).filter((f) => f.endsWith(".md"));
    if (md.length === 1) return path.join(dir, md[0]);
    if (md.length > 1) throw new Error(`multiple theses in ${dir}/ — pass --thesis or --slug`);
  }
  throw new Error("no thesis found — pass --thesis <path> or --slug <name>");
}

function loadCodingFixture(p) {
  const raw = JSON.parse(fs.readFileSync(p, "utf8"));
  const codes = {};
  const norm = { present: "P", absent: "A", unresolved: "U", p: "P", a: "A", u: "U" };
  for (const s of SIGNALS) {
    const v = typeof raw[s] === "string" ? raw[s].trim().toLowerCase() : "";
    codes[s] = norm[v] ?? "U";
  }
  codes.game_age_gte_2yr = "U"; // hard rule: new concept
  return codes;
}

function alignmentRow(kpis, kpi) {
  const d = kpis[kpi].discriminating;
  const frac = d.total ? (d.present / d.total).toFixed(3) : "n/a";
  return `| ${kpi} | ${kpis[kpi].bar} | ${frac} | ${d.present}/${d.total} | ${d.present_signals.join(", ") || "—"} |`;
}

function comparablesBlock(kpis, kpi) {
  const c = kpis[kpi].comparables;
  if (!c.length) return "_none_";
  return c.map((x) => `${x.game} (${x.kpi_value ?? "?"}, sim ${x.similarity})`).join(" · ");
}

function renderReport(record, ranLlm) {
  const { kpis, alignment, confidence, archetype, summary, executive } = record;
  const lines = [];
  lines.push(`# Evaluation — ${record.thesis}`);
  lines.push("");
  const scorer = ranLlm ? `headless model \`${PINNED_MODEL}\`` : "agent-coded signals";
  lines.push(`_Evaluation v2 · ${scorer} · ${record.evaluated_at} · confidence ${confidence.resolved}/35 signals resolved (${(confidence.share * 100).toFixed(0)}%)_`);
  lines.push("");
  if (executive) { lines.push(`> ${executive}`); lines.push(""); }
  lines.push("## Signal alignment");
  lines.push("");
  lines.push("| KPI | Bar | Alignment | Discriminating | Present discriminating signals |");
  lines.push("|---|---|---:|---:|---|");
  for (const k of KPIS) lines.push(alignmentRow(kpis, k));
  lines.push("");
  lines.push(`**Mean alignment:** ${alignment.mean ?? "n/a"}`);
  lines.push("");
  lines.push("## Nearest comparables (by weighted signal similarity)");
  lines.push("");
  for (const k of KPIS) lines.push(`- **${k}** — ${comparablesBlock(kpis, k)}`);
  lines.push("");
  if (archetype) {
    lines.push("## Archetype");
    lines.push("");
    lines.push(`- **Matched:** ${archetype.matched}`);
    if (archetype.evidence_games?.length) lines.push(`- **Evidence games:** ${archetype.evidence_games.join(", ")}`);
    lines.push(`- **ARPU path:** ${archetype.arpu_path} · **Tensions:** ${archetype.tension_profile}`);
    if (archetype.tensions?.length) lines.push(`- **Cross-KPI tensions:** ${archetype.tensions.join("; ")}`);
    if (archetype.wedges?.length) lines.push(`- **Wedges:** ${archetype.wedges.join("; ")}`);
    lines.push("");
  }
  if (summary?.strengths?.length || summary?.weaknesses?.length) {
    lines.push("## Read");
    lines.push("");
    for (const s of summary.strengths ?? []) lines.push(`- ✅ ${s}`);
    for (const w of summary.weaknesses ?? []) lines.push(`- ⚠️ ${w}`);
    lines.push("");
  }
  if (!ranLlm) lines.push("_Alignment, comparables, and confidence are fully computed. The archetype and qualitative read are authored by the analyst agent in its turn._");
  return lines.join("\n") + "\n";
}

async function main() {
  if (opt["print-signals"]) { printSignals(); return; }

  const thesisPath = findThesis();
  const slug = opt.slug || path.basename(thesisPath).replace(/\.md$/, "");
  const id = opt.id || slug;
  const thesisBody = fs.readFileSync(thesisPath, "utf8");
  log(`thesis: ${thesisPath} (slug=${slug})`);

  // Stage 1a — signal coding. Normal flow: the agent's coding (explicit
  // --coding, or a committed theses/<slug>.coding.json sidecar). Headless
  // opt-in (--llm) lets the sim code signals itself via a pinned model.
  let codes;
  const sidecar = path.join(path.dirname(thesisPath), `${slug}.coding.json`);
  if (opt.coding) {
    codes = loadCodingFixture(opt.coding);
    log(`coding: ${opt.coding}`);
  } else if (fs.existsSync(sidecar)) {
    codes = loadCodingFixture(sidecar);
    log(`coding: committed sidecar ${sidecar}`);
  } else if (opt.llm) {
    log(`coding: headless pinned model ${PINNED_MODEL} (35 signals, temp 0)`);
    codes = await codeSignals(id, slug, thesisBody, [], {}, null, SIGNAL_DEFINITIONS_MD, log);
  } else {
    throw new Error(
      `no signal coding for "${slug}". Normal flow: the analyst agent codes the ` +
      `35 signals (see \`node evaluate.mjs --print-signals\`) and writes ${sidecar}. ` +
      `For headless self-contained scoring, pass --llm (needs a pinned-model key).`,
    );
  }

  // Stage 1b — deterministic mechanics (pure, keyless).
  const core = computeEvaluationCore(codes, GAME_SIGNAL_PROFILES);
  log(`alignment ${KPIS.map((k) => `${k}=${core.alignment[k]}`).join(" ")} mean=${core.alignment.mean} · ${core.confidence.resolved}/35 resolved`);

  const record = {
    thesis: slug, thesis_id: id, evaluation_version: "2.0",
    evaluated_at: new Date().toISOString().slice(0, 10),
    confidence: core.confidence, kpis: core.kpis, alignment: core.alignment,
    archetype: null, summary: { strengths: [], weaknesses: [] }, executive: "", codes,
  };

  // Stage 1c/1d — archetype + prose. Normal flow: the agent authors these in
  // its turn. Only the headless path runs them here.
  if (opt.llm) {
    log("archetype via headless pinned model");
    const { archetype, outside_framework } = await buildArchetype(id, thesisBody, codes, core, ARCHETYPE_REFERENCE_MD);
    record.archetype = archetype;
    record.outside_framework = outside_framework;
    log("summary/executive via headless pinned model");
    const prose = await openaiJson(
      [{ role: "system", content: summaryPrompt(record, thesisBody) },
       { role: "user", content: "Write the JSON now." }],
      "summary prose",
    );
    const strArr = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === "string" && x.trim()) : []);
    record.summary = { strengths: strArr(prose.summary?.strengths), weaknesses: strArr(prose.summary?.weaknesses) };
    record.executive = typeof prose.executive === "string" ? prose.executive.trim() : "";
  }

  // Outputs.
  const outDir = opt["out-dir"];
  fs.mkdirSync(outDir, { recursive: true });
  const reportPath = path.join(outDir, `${slug}.md`);
  fs.writeFileSync(reportPath, renderReport(record, opt.llm));
  log(`report → ${reportPath}`);

  const metricsDoc = {
    aggregate: {
      retention_align: core.alignment.retention,
      mau_align: core.alignment.mau,
      revenue_align: core.alignment.revenue,
      mean_align: core.alignment.mean,
      confidence: core.confidence.share,
    },
    record,
  };
  const metricsPath = opt.metrics || process.env.ORX_METRICS_PATH;
  if (metricsPath) {
    fs.mkdirSync(path.dirname(metricsPath), { recursive: true });
    fs.writeFileSync(metricsPath, JSON.stringify(metricsDoc, null, 2));
    log(`metrics → ${metricsPath}`);
  } else {
    process.stdout.write(JSON.stringify(metricsDoc, null, 2) + "\n");
    log("metrics → stdout (no --metrics / $ORX_METRICS_PATH)");
  }
}

main().catch((e) => { log(`ERROR: ${e.message}`); process.exit(1); });
