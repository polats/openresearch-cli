// Pinned-model chat client — OPTIONAL, headless mode only (`evaluate.mjs --llm`).
//
// The normal flow is keyless: the analyst AGENT codes the signals in its turn
// using whatever harness the session runs on (Claude Code / Codex / OpenCode /
// …), so no API key and no hardcoded provider are involved. This client is used
// ONLY for headless batch runs with no agent in the loop.
//
// When used, the scoring model is PINNED (not per-caller) so a batch of ideas
// stays comparable. Override with EVAL_MODEL / EVAL_PROVIDER; keep it constant
// across a batch.
//
//   EVAL_MODEL     e.g. "gpt-4.1" (default) or "claude-sonnet-5"
//   EVAL_PROVIDER  "openai" | "anthropic" (inferred from the model name if unset)
//   OPENAI_API_KEY / ANTHROPIC_API_KEY   the key for the chosen provider
//
// Exposes the same `openaiChat(messages, opts)` signature eval-core.mjs expects:
// takes an OpenAI-shaped messages array, returns the assistant text string.

export const PINNED_MODEL = process.env.EVAL_MODEL?.trim() || "gpt-4.1";

function provider() {
  const explicit = process.env.EVAL_PROVIDER?.trim().toLowerCase();
  if (explicit === "openai" || explicit === "anthropic") return explicit;
  return /^claude/i.test(PINNED_MODEL) ? "anthropic" : "openai";
}

function fail(code, msg) {
  const err = new Error(msg);
  err.statusCode = code;
  throw err;
}

async function openaiCall(messages, { temperature, responseFormat }) {
  const key = process.env.OPENAI_API_KEY?.trim();
  if (!key) fail(503, "OPENAI_API_KEY not set (pinned provider = openai)");
  let res;
  try {
    res = await fetch("https://api.openai.com/v1/chat/completions", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
      body: JSON.stringify({
        model: PINNED_MODEL,
        temperature,
        messages,
        ...(responseFormat ? { response_format: responseFormat } : {}),
      }),
      signal: AbortSignal.timeout(120_000),
    });
  } catch (e) {
    if (e.name === "TimeoutError") fail(504, "OpenAI request timed out");
    fail(502, `could not reach OpenAI: ${e.message}`);
  }
  if (!res.ok) {
    let detail = "";
    try { detail = (await res.json())?.error?.message ?? ""; } catch {}
    fail(502, `OpenAI API error ${res.status}: ${detail || res.statusText}`.trim());
  }
  const data = await res.json();
  const text = data?.choices?.[0]?.message?.content;
  if (typeof text !== "string" || !text.trim()) fail(502, "OpenAI returned an empty reply");
  return text.trim();
}

async function anthropicCall(messages, { temperature }) {
  const key = process.env.ANTHROPIC_API_KEY?.trim();
  if (!key) fail(503, "ANTHROPIC_API_KEY not set (pinned provider = anthropic)");
  // Split the OpenAI-shaped array into a system string + user/assistant turns.
  const system = messages.filter((m) => m.role === "system").map((m) => m.content).join("\n\n");
  const turns = messages
    .filter((m) => m.role === "user" || m.role === "assistant")
    .map((m) => ({ role: m.role, content: m.content }));
  let res;
  try {
    res = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-api-key": key,
        "anthropic-version": "2023-06-01",
      },
      body: JSON.stringify({
        model: PINNED_MODEL,
        max_tokens: 4096,
        temperature,
        ...(system ? { system } : {}),
        messages: turns,
      }),
      signal: AbortSignal.timeout(120_000),
    });
  } catch (e) {
    if (e.name === "TimeoutError") fail(504, "Anthropic request timed out");
    fail(502, `could not reach Anthropic: ${e.message}`);
  }
  if (!res.ok) {
    let detail = "";
    try { detail = (await res.json())?.error?.message ?? ""; } catch {}
    fail(502, `Anthropic API error ${res.status}: ${detail || res.statusText}`.trim());
  }
  const data = await res.json();
  const text = (data?.content ?? []).filter((b) => b.type === "text").map((b) => b.text).join("").trim();
  if (!text) fail(502, "Anthropic returned an empty reply");
  return text;
}

/** One pinned-model chat call. Same contract as foundry's openaiChat. */
export async function openaiChat(messages, { temperature = 0.4, responseFormat } = {}) {
  return provider() === "anthropic"
    ? anthropicCall(messages, { temperature })
    : openaiCall(messages, { temperature, responseFormat });
}
