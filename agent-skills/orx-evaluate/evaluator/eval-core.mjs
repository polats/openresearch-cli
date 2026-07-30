// Evaluation v2 core — pure, shared by the dev pipeline
// (src/lib/new-idea-pipeline.mjs) and the deployed Vercel serverless
// functions (game-foundry/api/). All prompts and the mechanical v2
// arithmetic live here; data files (signal definitions, archetype reference,
// game-signal profiles) are passed in by the caller — the dev pipeline reads
// them from the repo, the serverless functions import the embedded copies
// (src/lib/eval-data.mjs).

import { openaiChat } from "./llm.mjs";

// The 35 boolean signals, in the canonical order used by the batch scripts
// (thesis-engine/_work/_evalv2_build_063_080.py).
export const SIGNALS = [
  "short_session", "timer_gating", "collection_drive", "competitive_ranked",
  "limited_time_events", "variable_reward_loop", "social_obligation", "social_spending",
  "strong_ip", "mass_market_genre_ip", "one_touch_input", "accessible_mechanic",
  "universal_player_fantasy", "fantasy_aligned", "multiple_return_triggers",
  "measurable_progression", "daily_content", "streak_mechanic", "free_to_play",
  "game_age_gte_2yr", "non_us_geographic_majority", "low_arpu", "content_refresh_cadence",
  "ad_supported_monetization", "social_multiplayer", "regional_cultural_fit",
  "games_as_social_infra", "network_effects_local_play", "ugc_platform_model",
  "iap_primary", "escalating_spending", "failure_trigger", "pvp_battlepass",
  "meta_narrative", "portrait_orientation",
];

// Per-KPI discrimination classifications, ported verbatim from the skill's
// discrimination-reference.md (same constants as the batch scripts' CLS).
// G = GRADIENT, WG = WEAK_GRADIENT, U = UNIVERSAL, I = INVERSE, A = AMBIGUOUS.
export const CLS = {
  retention: {
    short_session: "U", timer_gating: "U", collection_drive: "U",
    competitive_ranked: "I", limited_time_events: "U", variable_reward_loop: "U",
    social_obligation: "U", social_spending: "U", strong_ip: "G", mass_market_genre_ip: "U",
    one_touch_input: "U", accessible_mechanic: "U", universal_player_fantasy: "U",
    fantasy_aligned: "G", multiple_return_triggers: "U", measurable_progression: "U",
    daily_content: "I", streak_mechanic: "U", free_to_play: "U", game_age_gte_2yr: "U",
    non_us_geographic_majority: "U", low_arpu: "G", content_refresh_cadence: "U",
    ad_supported_monetization: "U", social_multiplayer: "I", regional_cultural_fit: "WG",
    games_as_social_infra: "U", network_effects_local_play: "U", ugc_platform_model: "U",
    iap_primary: "I", escalating_spending: "I", failure_trigger: "I", pvp_battlepass: "I",
    meta_narrative: "I", portrait_orientation: "G",
  },
  mau: {
    short_session: "G", timer_gating: "U", collection_drive: "U",
    competitive_ranked: "U", limited_time_events: "U", variable_reward_loop: "U",
    social_obligation: "I", social_spending: "I", strong_ip: "I", mass_market_genre_ip: "G",
    one_touch_input: "U", accessible_mechanic: "U", universal_player_fantasy: "G",
    fantasy_aligned: "U", multiple_return_triggers: "U", measurable_progression: "U",
    daily_content: "I", streak_mechanic: "I", free_to_play: "U", game_age_gte_2yr: "U",
    non_us_geographic_majority: "G", low_arpu: "G", content_refresh_cadence: "I",
    ad_supported_monetization: "U", social_multiplayer: "U", regional_cultural_fit: "I",
    games_as_social_infra: "U", network_effects_local_play: "U", ugc_platform_model: "U",
    iap_primary: "U", escalating_spending: "I", failure_trigger: "U", pvp_battlepass: "I",
    meta_narrative: "I", portrait_orientation: "G",
  },
  revenue: {
    short_session: "U", timer_gating: "G", collection_drive: "WG",
    competitive_ranked: "G", limited_time_events: "WG", variable_reward_loop: "WG",
    social_obligation: "WG", social_spending: "WG", strong_ip: "U", mass_market_genre_ip: "U",
    one_touch_input: "U", accessible_mechanic: "I", universal_player_fantasy: "G",
    fantasy_aligned: "G", multiple_return_triggers: "WG", measurable_progression: "WG",
    daily_content: "WG", streak_mechanic: "I", free_to_play: "U", game_age_gte_2yr: "U",
    non_us_geographic_majority: "G", low_arpu: "I", content_refresh_cadence: "WG",
    ad_supported_monetization: "U", social_multiplayer: "WG", regional_cultural_fit: "I",
    games_as_social_infra: "U", network_effects_local_play: "G", ugc_platform_model: "U",
    iap_primary: "WG", escalating_spending: "U", failure_trigger: "U", pvp_battlepass: "A",
    meta_narrative: "U", portrait_orientation: "G",
  },
};

export const KPIS = ["retention", "mau", "revenue"];
export const BARS = { retention: "D30 >= 20%", mau: "> 25M", revenue: "> $250M/yr" };
// Skill similarity weights (SKILL.md step 4): GRADIENT/ELITE_ONLY 3x,
// WEAK_GRADIENT 2x, everything else 1x.
const SIM_WEIGHT = { G: 3, WG: 2, U: 1, I: 1, A: 1 };

const setOf = (kpi, cls) => SIGNALS.filter((s) => CLS[kpi][s] === cls);

// ------------------------------------------------------------------ helpers

const answerIsValid = (rec) =>
  !!rec && rec.status !== "invalidated" && typeof rec.answer === "string" && rec.answer.trim();

/**
 * `items` is [{checklist_id, question, status, answer}] with status
 * answered|unanswered; `answers` the stored user-answer overlay keyed by
 * checklist id.
 */
export function checklistAnswersBlock(items, answers = {}, slug = "") {
  return items
    .map((it) => {
      let line = `${it.checklist_id}. ${it.question}\n`;
      if (it.status === "answered") line += `   ANSWER: ${it.answer}`;
      else if (answerIsValid(answers[it.checklist_id]))
        line += `   ANSWER (stored user answer, answers/${slug}.answers.json): ${answers[it.checklist_id].answer}`;
      else line += "   (unanswered)";
      return line;
    })
    .join("\n");
}

export function transcriptBlock(transcript) {
  if (!Array.isArray(transcript) || !transcript.length) return "";
  return (
    "\n\nIntake-chat transcript (the designer's own words):\n---\n" +
    transcript.map((m) => `${m.role}: ${m.content}`).join("\n\n") +
    "\n---"
  );
}

/**
 * Gating checklist items = all 12 except c5 (rubric.md completeness gate).
 * A stored, non-invalidated user answer keyed by checklist id closes an
 * item — same semantics as the site loader's unansweredCount.
 */
export function openGatingItems(items, answers = {}) {
  return items.filter(
    (it) =>
      it.checklist_id !== "c5" &&
      it.status !== "answered" &&
      !answerIsValid(answers[it.checklist_id]),
  );
}

export async function openaiJson(messages, label) {
  const raw = await openaiChat(messages, {
    temperature: 0,
    responseFormat: { type: "json_object" },
  });
  try {
    return JSON.parse(raw.replace(/^```json\s*|```\s*$/g, ""));
  } catch (e) {
    throw new Error(`${label} returned unparseable JSON: ${e.message}`);
  }
}

// ------------------------------------------------------- stage 1a: coding

export function codingPrompt(id, slug, thesisBody, items, answers, transcript, signalDefsMd) {
  return `You are the signal-coding step of the FOUNDRY evaluation v2 pipeline
(game-signal-analysis skill, step 2). Code a game CONCEPT against 35
empirical boolean signals.

Signal definitions, verbatim from the skill's data file:
---
${signalDefsMd}
---

Coding criteria (from the skill, follow exactly):
- "Present": the material explicitly describes or strongly implies this signal.
- "Absent": the material explicitly contradicts this signal or describes a
  design incompatible with it.
- "Unresolved": the material neither confirms nor rules it out.
- Code in a single pass; never ask questions; missing info = "Unresolved".
- Special handling:
  - free_to_play: if pricing is not mentioned, code "Present" (standard mobile model).
  - game_age_gte_2yr: ALWAYS "Unresolved" for a new concept (this is one).

Material — thesis document (theses/${id}.md):
---
${thesisBody}
---

Checklist answers (stored user answers are the designer's own design
statements — treat them as authoritative):
${checklistAnswersBlock(items, answers, slug)}${transcriptBlock(transcript)}

Output: STRICT JSON only — one object with EXACTLY these 35 keys, each
"Present" | "Absent" | "Unresolved":
${JSON.stringify(SIGNALS)}`;
}

/** Returns {signal: "P"|"A"|"U"} for all 35 signals. */
export async function codeSignals(id, slug, thesisBody, items, answers, transcript, signalDefsMd, log) {
  const out = await openaiJson(
    [
      { role: "system", content: codingPrompt(id, slug, thesisBody, items, answers, transcript, signalDefsMd) },
      { role: "user", content: "Code all 35 signals now. STRICT JSON only." },
    ],
    "signal coding",
  );
  const codes = {};
  const bad = [];
  for (const s of SIGNALS) {
    const v = typeof out[s] === "string" ? out[s].trim().toLowerCase() : "";
    if (v === "present") codes[s] = "P";
    else if (v === "absent") codes[s] = "A";
    else if (v === "unresolved") codes[s] = "U";
    else {
      codes[s] = "U";
      bad.push(s);
    }
  }
  if (bad.length) log(`evaluation: coding missing/invalid for ${bad.join(", ")} — treated as Unresolved`);
  // Enforce the skill's hard rule regardless of the model's output.
  codes.game_age_gte_2yr = "U";
  return codes;
}

// ---------------------------------------------------- stage 1b: mechanics

function comparablesFor(codes, kpi, profiles) {
  const scored = [];
  for (const p of profiles) {
    if (!Array.isArray(p.kpi_cohorts) || !p.kpi_cohorts.includes(kpi)) continue;
    let w = 0;
    let match = 0;
    for (const s of SIGNALS) {
      if (codes[s] === "U") continue;
      const pv = p[s];
      if (typeof pv !== "boolean") continue;
      const weight = SIM_WEIGHT[CLS[kpi][s]] ?? 1;
      w += weight;
      if ((codes[s] === "P") === pv) match += weight;
    }
    if (w === 0) continue;
    let kpiValue = null;
    if (kpi === "retention" && typeof p.d30 === "number") kpiValue = `D30 ${p.d30}%`;
    else if (kpi === "mau" && typeof p.mau_m === "number") kpiValue = `${p.mau_m}M MAU`;
    else if (kpi === "revenue" && typeof p.revenue_m === "number")
      kpiValue = `$${p.revenue_m}M/yr`;
    scored.push({ game: p.game, similarity: Math.round((match / w) * 100) / 100, kpi_value: kpiValue });
  }
  scored.sort((a, b) => b.similarity - a.similarity || a.game.localeCompare(b.game));
  return scored.slice(0, 5);
}

/** Mechanical v2 arithmetic: kpis block, confidence, alignment. */
export function computeEvaluationCore(codes, profiles) {
  const kpis = {};
  const fracs = {};
  for (const kpi of KPIS) {
    const disc = setOf(kpi, "G"); // no ELITE_ONLY signals exist in the reference
    const present = disc.filter((s) => codes[s] === "P");
    const absent = disc.filter((s) => codes[s] === "A");
    const unresolved = disc.filter((s) => codes[s] === "U");
    const inv = setOf(kpi, "I");
    const univ = setOf(kpi, "U");
    const total = present.length + absent.length; // resolved discriminating
    fracs[kpi] = total ? present.length / total : null;
    kpis[kpi] = {
      bar: BARS[kpi],
      discriminating: {
        present: present.length,
        total,
        present_signals: present,
        absent_signals: absent,
        unresolved_signals: unresolved,
      },
      anti_patterns: {
        present: inv.filter((s) => codes[s] === "P").length,
        total: inv.length,
        present_signals: inv.filter((s) => codes[s] === "P"),
      },
      table_stakes: {
        present: univ.filter((s) => codes[s] === "P").length,
        total: univ.length,
        missing_signals: univ.filter((s) => codes[s] === "A").sort(),
      },
      comparables: comparablesFor(codes, kpi, profiles),
    };
  }
  const withData = KPIS.map((k) => fracs[k]).filter((v) => v !== null);
  const mean = withData.length ? withData.reduce((a, b) => a + b, 0) / withData.length : null;
  const alignment = Object.fromEntries(
    KPIS.map((k) => [k, fracs[k] === null ? null : Math.round(fracs[k] * 1000) / 1000]),
  );
  alignment.mean = mean === null ? null : Math.round(mean * 1000) / 1000;
  const resolved = SIGNALS.filter((s) => codes[s] !== "U").length;
  return {
    kpis,
    alignment,
    confidence: { resolved, total: 35, share: Math.round((resolved / 35) * 1000) / 1000 },
  };
}

// --------------------------------------------------- stage 1c: archetype

export function archetypePrompt(id, thesisBody, codes, core, archetypeRefMd) {
  const coded = Object.fromEntries(
    SIGNALS.map((s) => [s, { P: "Present", A: "Absent", U: "Unresolved" }[codes[s]]]),
  );
  return `You are the archetype-alignment step of the FOUNDRY evaluation v2
pipeline. Map a game concept to the market-fit framework's mechanical
archetypes, signal recipes, ARPU paths, tensions, and sentiment wedges.

Reference tables, verbatim from the archetype-alignment skill:
---
${archetypeRefMd}
---

The concept's 35-signal coding (already done — do NOT recode):
${JSON.stringify(coded, null, 2)}

Per-KPI signal alignment already computed:
${JSON.stringify(core.alignment)}

Thesis document (theses/${id}.md):
---
${thesisBody}
---

Produce STRICT JSON:
{
  "archetype": {
    "matched": "<best-matching mechanical archetype name; note a secondary lean or partial match in the same string when appropriate>",
    "evidence_games": ["<the archetype's evidence games from the reference>"],
    "recipe_fit": {
      "must_haves": {"covered": ["<signal>"], "missing": ["<signal>"]},
      "avoid_violations": ["<signal (short parenthetical why, grounded in the thesis)>"]
    },
    "arpu_path": "High-ARPU" | "Volume" | "Mixed",
    "tension_profile": "tension-free" | "single-tension" | "multi-tension",
    "tensions": ["<named cross-KPI tension that applies, with the signal that causes it>"],
    "wedges": ["<sentiment wedge from the reference's 7 that this design explicitly pursues>"]
  },
  "outside_framework": [
    {"mechanic": "<concept mechanic with NO signal coverage in the 35>", "note": "outside framework — no data"}
  ]
}

Rules:
- recipe_fit is against the single best-fitting signal recipe (R-01..R-08)
  for this concept's evident KPI strategy: covered = recipe MUST-HAVE signals
  coded Present; missing = MUST-HAVE signals coded Absent or Unresolved;
  avoid_violations = recipe AVOID signals coded Present.
- tensions: only tensions from the Cross-KPI Tension Patterns table whose
  driving signal is coded Present. tension_profile follows the count.
- wedges: only wedges the thesis explicitly designs for.
- outside_framework: mechanics genuinely uncovered by the 35 signals
  (e.g. AI-generated content, player-to-player trading). Empty list if none.
- Signal names must come from the 35; game names from the reference tables.`;
}

export async function buildArchetype(id, thesisBody, codes, core, archetypeRefMd) {
  const out = await openaiJson(
    [
      { role: "system", content: archetypePrompt(id, thesisBody, codes, core, archetypeRefMd) },
      { role: "user", content: "Produce the JSON now." },
    ],
    "archetype block",
  );
  const a = out.archetype ?? {};
  const arr = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === "string") : []);
  const archetype = {
    matched: typeof a.matched === "string" ? a.matched : "unmatched",
    evidence_games: arr(a.evidence_games),
    recipe_fit: {
      must_haves: {
        covered: arr(a.recipe_fit?.must_haves?.covered),
        missing: arr(a.recipe_fit?.must_haves?.missing),
      },
      avoid_violations: arr(a.recipe_fit?.avoid_violations),
    },
    arpu_path: ["High-ARPU", "Volume", "Mixed"].includes(a.arpu_path) ? a.arpu_path : "Mixed",
    tension_profile: typeof a.tension_profile === "string" ? a.tension_profile : "tension-free",
    tensions: arr(a.tensions),
    wedges: arr(a.wedges),
  };
  const outside = (Array.isArray(out.outside_framework) ? out.outside_framework : [])
    .filter((o) => o && typeof o.mechanic === "string")
    .map((o) => ({ mechanic: o.mechanic, note: "outside framework — no data" }));
  return { archetype, outside_framework: outside };
}

// ------------------------------------------------ stage 1d: summary prose

export function summaryPrompt(record, thesisBody) {
  return `You write the "summary" and "executive" fields of a FOUNDRY
evaluation v2 record, for a human skimming a site — NOT for the pipeline.
Rules (thesis-engine/rubric.md v2, follow exactly):

- JARGON BAN: no signal names, no statistics, no framework jargon. Banned
  examples: "GRADIENT", "raw subscore", "discriminating similarity",
  "Cramer's V", "INVERSE signals", "R-07", "alignment fraction",
  "anti-pattern", "table stakes", snake_case signal names.
- Each bullet says a concrete thing about THIS game and why it helps or
  hurts. Good: "The catch minigame has real skill: throw precision decides
  whether you keep the creature." Bad: "Retention discriminating fraction
  is 0.667."
- GROUNDING RULE: every bullet must be traceable to a specific fact in the
  record below — a named signal coding, a comparable and its measured KPI
  value, or a recipe fact (must-have covered/missing, avoid violation,
  tension, wedge). Never assert anything the record does not show.
- 3-7 bullets per list, each one plain-English sentence.
- "executive": EXACTLY two sentences. Sentence one names the game's genuine
  strength worth preserving, grounded in a record fact. Sentence two names
  the single change that would most improve signal alignment — grounded in a
  specific absent discriminating signal, missing must-have, or avoid
  violation (described in plain language, not by signal name).

The evaluation record (facts to ground in):
${JSON.stringify(record, null, 2)}

Thesis document (context for plain-language descriptions of mechanics):
---
${thesisBody}
---

Output STRICT JSON:
{"summary": {"strengths": ["..."], "weaknesses": ["..."]}, "executive": "... . ... ."}`;
}

// -------------------------------------------------- full evaluation record

/**
 * Run the full evaluation v2 (coding → mechanics → archetype → prose) for a
 * concept. Purely in-memory: the caller supplies the material and the data
 * files, and persists the returned record. Does NOT apply the completeness
 * gate — the caller does (it owns the checklist-state semantics).
 */
export async function buildEvaluationRecord(
  { id, slug, thesisBody, items, answers = {}, transcript = null },
  { signalDefsMd, archetypeRefMd, profiles },
  log = () => {},
) {
  log("evaluation: signal coding via OpenAI (35 signals, temp 0)");
  const codes = await codeSignals(id, slug, thesisBody, items, answers, transcript, signalDefsMd, log);
  const core = computeEvaluationCore(codes, profiles);
  log(
    `evaluation: coded — ${core.confidence.resolved}/35 resolved; alignment ` +
      KPIS.map((k) => `${k}=${core.alignment[k]}`).join(" ") +
      ` mean=${core.alignment.mean}`,
  );

  log("evaluation: archetype block via OpenAI");
  const { archetype, outside_framework } = await buildArchetype(id, thesisBody, codes, core, archetypeRefMd);

  const record = {
    thesis: slug,
    thesis_id: id,
    evaluation_version: "2.0",
    evaluated_at: new Date().toISOString().slice(0, 10),
    reports: {
      signal_analysis_md: null,
      archetype_alignment_md: null,
      storage: "path",
      generated: "new-idea-pipeline",
    },
    confidence: core.confidence,
    kpis: core.kpis,
    archetype,
    alignment: core.alignment,
    outside_framework,
    summary: { strengths: [], weaknesses: [] },
    executive: "",
  };

  log("evaluation: summary/executive via OpenAI");
  const prose = await openaiJson(
    [
      { role: "system", content: summaryPrompt(record, thesisBody) },
      { role: "user", content: "Write the JSON now." },
    ],
    "summary prose",
  );
  const strArr = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === "string" && x.trim()) : []);
  record.summary = {
    strengths: strArr(prose.summary?.strengths),
    weaknesses: strArr(prose.summary?.weaknesses),
  };
  record.executive = typeof prose.executive === "string" ? prose.executive.trim() : "";
  return record;
}

// --------------------------------------------------------- market prompt

export function marketPrompt(id, thesisBody, c12, transcript) {
  return `You are the niche/competitor-proposal step of the FOUNDRY market
stage (thesis-engine/agents/market.md). You ONLY propose the niche and the
competitor set — a script fetches every number.

Rules (from the agent spec, follow exactly):
- "niche": one line naming the audience × fantasy × verb this idea competes
  in (e.g. "cozy farming × creature-collection check-in (mobile, no-PvP)").
- "competitors": 3-5 nearest ACTUAL competitors — real, shipped MOBILE games
  a player in this niche is choosing between today. Same audience, same
  fantasy, same core verb. GAMES ONLY, and only app-store-measurable titles
  (iOS/Android). One-line justification each: why a player of that game is
  this idea's player.
- These are audience competitors, NOT mechanical look-alikes: a game may
  share every mechanic and still compete for nobody's fantasy.
- The designer's c12 nearest-comp answer MUST appear among the candidates:
  include it in "competitors", or put it in "considered_but_excluded" with a
  justification.
- A true competitor that is not app-store measurable (PC-only, Roblox
  experience, console) goes in "considered_but_excluded" with the reason.
- No close competitors is legitimate: an empty "competitors" list is
  allowed — never pad with far-fetched comps.

Designer's c12 answer (nearest comp + delta):
${c12 ?? "(none recorded)"}

Thesis document (theses/${id}.md):
---
${thesisBody}
---${transcriptBlock(transcript)}

Output STRICT JSON:
{"niche": "...",
 "competitors": [{"name": "<exact store title>", "search_term": "<Sensor Tower search term>", "justification": "..."}],
 "considered_but_excluded": [{"name": "...", "reason": "..."}]}`;
}

export const normEntityName = (s) =>
  s.normalize("NFKC").toLowerCase().replace(/[®™©]/g, "").replace(/[^\p{L}\p{N}]/gu, "");

/** Pick the search result that best matches the wanted title. */
export function pickCandidate(results, wanted) {
  const target = normEntityName(wanted);
  const exact = results.find((r) => normEntityName(r.name ?? "") === target);
  if (exact) return exact;
  const prefix = results.find((r) => {
    const n = normEntityName(r.name ?? "");
    return n.startsWith(target) || target.startsWith(n);
  });
  return prefix ?? results[0] ?? null;
}
