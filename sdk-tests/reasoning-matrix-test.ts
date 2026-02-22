#!/usr/bin/env bun
/**
 * Monoize Reasoning Matrix Test
 *
 * Tests 3 models × 3 API formats × 3 aspects:
 *   - Non-streaming reasoning summary passback
 *   - Streaming reasoning summary passback
 *   - Encrypted reasoning persistence (multi-turn)
 */

const BASE_URL = process.env.BASE_URL ?? "https://mono.ikale.io/v1";
const API_KEY = process.env.API_KEY ?? "sk-4c65b2d5a78a43e1aab5a362f8419667";
const TIMEOUT_MS = 120_000;

const MODELS = ["gpt-5.2", "claude-opus-4.6", "gemini-3.1-pro-preview"] as const;
const FORMATS = ["chat_completions", "responses", "messages"] as const;

type Model = (typeof MODELS)[number];
type Format = (typeof FORMATS)[number];
type Verdict = "PASS" | "FAIL" | "ERROR";

interface TestResult {
  model: Model;
  format: Format;
  nsVerdict: Verdict;
  nsDetail: string;
  sVerdict: Verdict;
  sDetail: string;
  eVerdict: Verdict;
  eDetail: string;
}

const Q1 = "What is 17 × 23? Think step by step, briefly.";
const Q2 = "Now multiply that result by 2.";

// ─── Helpers ────────────────────────────────────────────────────────────────

function truncate(s: string, n = 100): string {
  if (!s) return "(empty)";
  const clean = s.replace(/\n/g, " ").trim();
  return clean.length > n ? clean.slice(0, n) + "…" : clean;
}

async function timedFetch(
  url: string,
  init: RequestInit
): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    return await fetch(url, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
  }
}

function authHeaders(format: Format): Record<string, string> {
  const h: Record<string, string> = {
    "Content-Type": "application/json",
    Authorization: `Bearer ${API_KEY}`,
  };
  if (format === "messages") h["anthropic-version"] = "2023-06-01";
  return h;
}

// ─── SSE Parser ─────────────────────────────────────────────────────────────

interface SSEEvent {
  event: string;
  data: any;
}

async function parseSSEStream(resp: Response): Promise<SSEEvent[]> {
  const raw = await resp.text();
  const events: SSEEvent[] = [];
  let currentEvent = "";

  for (const line of raw.split("\n")) {
    if (line.startsWith("event: ")) {
      currentEvent = line.slice(7).trim();
    } else if (line.startsWith("data: ")) {
      const payload = line.slice(6).trim();
      if (payload === "[DONE]") break;
      try {
        const parsed = JSON.parse(payload);
        const evtName = currentEvent || parsed.type || "";
        events.push({ event: evtName, data: parsed });
      } catch {
        /* skip malformed */
      }
      currentEvent = "";
    }
  }
  return events;
}

// ─── Request Builders ───────────────────────────────────────────────────────

function chatBody(model: string, messages: any[], stream: boolean) {
  return {
    model,
    messages,
    stream,
    reasoning_effort: "high",
    ...(stream ? { stream_options: { include_usage: true } } : {}),
  };
}

function responsesBody(model: string, input: any, stream: boolean) {
  return { model, input, stream, reasoning: { effort: "high" } };
}

function messagesBody(model: string, messages: any[], stream: boolean) {
  return {
    model,
    messages,
    stream,
    max_tokens: 2048,
    thinking: { type: "enabled", budget_tokens: 16384 },
  };
}

// ─── Reasoning extractors ───────────────────────────────────────────────────

interface ReasoningResult {
  reasoning: string;
  encrypted: string;
  content: string;
  rawAssistant?: any; // for multi-turn passback
  rawOutput?: any[];
  rawContent?: any[];
}

// ── Chat Completions (non-stream) ──

async function chatNonStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: authHeaders("chat_completions"),
    body: JSON.stringify(
      chatBody(model, [{ role: "user", content: Q1 }], false)
    ),
  });
  const body = await resp.json();
  if (!resp.ok)
    throw new Error(`HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 300)}`);

  const msg = body.choices?.[0]?.message ?? {};
  const details: any[] = msg.reasoning_details ?? [];
  let reasoning = "";
  let encrypted = "";

  for (const d of details) {
    if (d.type === "reasoning.text") {
      if (d.text) reasoning += d.text;
      if (d.signature) encrypted += d.signature;
    }
    if (d.type === "reasoning.encrypted" && d.data) encrypted += d.data;
    if (d.type === "reasoning.summary" && d.summary) reasoning += d.summary;
  }
  // fallback legacy field
  if (!reasoning && msg.reasoning) reasoning = msg.reasoning;

  return {
    reasoning,
    encrypted,
    content: msg.content ?? "",
    rawAssistant: msg,
  };
}

// ── Chat Completions (stream) ──

async function chatStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: authHeaders("chat_completions"),
    body: JSON.stringify(
      chatBody(model, [{ role: "user", content: Q1 }], true)
    ),
  });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${await resp.text()}`);

  const events = await parseSSEStream(resp);
  let reasoning = "";
  let encrypted = "";
  let content = "";

  for (const evt of events) {
    const delta = evt.data?.choices?.[0]?.delta;
    if (!delta) continue;
    if (typeof delta.content === "string") content += delta.content;
    if (Array.isArray(delta.reasoning_details)) {
      for (const d of delta.reasoning_details) {
        if (d.type === "reasoning.text" && typeof d.text === "string")
          reasoning += d.text;
        if (d.type === "reasoning.text" && typeof d.signature === "string")
          encrypted += d.signature;
        if (d.type === "reasoning.encrypted" && typeof d.data === "string")
          encrypted += d.data;
        if (d.type === "reasoning.summary" && typeof d.summary === "string")
          reasoning += d.summary;
      }
    }
  }
  return { reasoning, encrypted, content };
}

// ── Responses (non-stream) ──

async function responsesNonStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/responses`, {
    method: "POST",
    headers: authHeaders("responses"),
    body: JSON.stringify(responsesBody(model, Q1, false)),
  });
  const body = await resp.json();
  if (!resp.ok)
    throw new Error(`HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 300)}`);

  const output: any[] = body.output ?? [];
  let reasoning = "";
  let encrypted = "";
  let content = "";

  for (const item of output) {
    if (item.type === "reasoning") {
      reasoning += item.text ?? "";
      if (item.signature) encrypted = item.signature;
      if (item.encrypted_content) encrypted = item.encrypted_content;
    }
    if (item.type === "message") {
      for (const p of item.content ?? []) {
        if (p.type === "output_text") content += p.text ?? "";
      }
    }
  }
  return { reasoning, encrypted, content, rawOutput: output };
}

// ── Responses (stream) ──

async function responsesStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/responses`, {
    method: "POST",
    headers: authHeaders("responses"),
    body: JSON.stringify(responsesBody(model, Q1, true)),
  });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${await resp.text()}`);

  const events = await parseSSEStream(resp);
  let reasoning = "";
  let encrypted = "";
  let content = "";

  for (const evt of events) {
    // Monoize wraps in { sequence_number, data } per STR1 — unwrap if needed
    const d = evt.data?.data ?? evt.data;

    if (
      evt.event === "response.output_item.added" ||
      evt.event === "response.output_item.done"
    ) {
      const item = d?.item ?? d;
      if (item?.type === "reasoning") {
        if (item.text) reasoning += item.text;
        if (item.signature) encrypted = item.signature;
        if (item.encrypted_content) encrypted = item.encrypted_content;
      }
    }
    if (evt.event === "response.reasoning_text.delta") {
      reasoning += d?.delta ?? "";
    }
    if (evt.event === "response.reasoning_signature.delta") {
      encrypted += d?.delta ?? "";
    }
    if (evt.event === "response.output_text.delta") {
      content += d?.delta ?? d?.text ?? "";
    }
    // fallback: extract from completed event if nothing streamed
    if (evt.event === "response.completed") {
      const out = d?.response?.output ?? d?.output ?? [];
      for (const item of out) {
        if (item.type === "reasoning") {
          if (!reasoning) reasoning = item.text ?? "";
          if (!encrypted) encrypted = item.signature ?? item.encrypted_content ?? "";
        }
      }
    }
  }
  return { reasoning, encrypted, content };
}

// ── Messages (non-stream) ──

async function messagesNonStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/messages`, {
    method: "POST",
    headers: authHeaders("messages"),
    body: JSON.stringify(
      messagesBody(model, [{ role: "user", content: Q1 }], false)
    ),
  });
  const body = await resp.json();
  if (!resp.ok)
    throw new Error(`HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 300)}`);

  const blocks: any[] = body.content ?? [];
  let reasoning = "";
  let encrypted = "";
  let content = "";

  for (const b of blocks) {
    if (b.type === "thinking") {
      reasoning += b.thinking ?? "";
      if (b.signature) encrypted = b.signature;
    }
    if (b.type === "text") content += b.text ?? "";
  }
  return { reasoning, encrypted, content, rawContent: blocks };
}

// ── Messages (stream) ──

async function messagesStream(model: Model): Promise<ReasoningResult> {
  const resp = await timedFetch(`${BASE_URL}/messages`, {
    method: "POST",
    headers: authHeaders("messages"),
    body: JSON.stringify(
      messagesBody(model, [{ role: "user", content: Q1 }], true)
    ),
  });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}: ${await resp.text()}`);

  const events = await parseSSEStream(resp);
  let reasoning = "";
  let encrypted = "";
  let content = "";

  for (const evt of events) {
    const d = evt.data;
    if (evt.event === "content_block_start" && d?.content_block?.type === "thinking") {
      reasoning += d.content_block.thinking ?? "";
      if (d.content_block.signature) encrypted = d.content_block.signature;
    }
    if (evt.event === "content_block_delta") {
      const delta = d?.delta;
      if (delta?.type === "thinking_delta" && delta.thinking)
        reasoning += delta.thinking;
      if (delta?.type === "signature_delta" && delta.signature)
        encrypted += delta.signature;
      if (delta?.type === "text_delta" && delta.text) content += delta.text;
    }
  }
  return { reasoning, encrypted, content };
}

// ─── Multi-turn encrypted reasoning ────────────────────────────────────────

interface EncResult {
  turn1Encrypted: boolean;
  turn2Ok: boolean;
  detail: string;
}

async function encryptedChat(model: Model): Promise<EncResult> {
  const t1 = await chatNonStream(model);
  if (!t1.encrypted)
    return { turn1Encrypted: false, turn2Ok: false, detail: "T1: no encrypted data" };

  const msgs = [
    { role: "user", content: Q1 },
    t1.rawAssistant,
    { role: "user", content: Q2 },
  ];
  const resp = await timedFetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: authHeaders("chat_completions"),
    body: JSON.stringify(chatBody(model, msgs, false)),
  });
  const body = await resp.json();
  if (!resp.ok)
    return {
      turn1Encrypted: true,
      turn2Ok: false,
      detail: `T2 HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 200)}`,
    };

  const c2 = body.choices?.[0]?.message?.content ?? "";
  return {
    turn1Encrypted: true,
    turn2Ok: c2.length > 0,
    detail: c2 ? `T2: ${truncate(c2, 80)}` : "T2 empty",
  };
}

async function encryptedResponses(model: Model): Promise<EncResult> {
  const t1 = await responsesNonStream(model);
  if (!t1.encrypted)
    return { turn1Encrypted: false, turn2Ok: false, detail: "T1: no signature" };

  const input = [
    { type: "message", role: "user", content: [{ type: "input_text", text: Q1 }] },
    ...(t1.rawOutput ?? []),
    { type: "message", role: "user", content: [{ type: "input_text", text: Q2 }] },
  ];
  const resp = await timedFetch(`${BASE_URL}/responses`, {
    method: "POST",
    headers: authHeaders("responses"),
    body: JSON.stringify(responsesBody(model, input, false)),
  });
  const body = await resp.json();
  if (!resp.ok)
    return {
      turn1Encrypted: true,
      turn2Ok: false,
      detail: `T2 HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 200)}`,
    };

  let c2 = "";
  for (const item of body.output ?? []) {
    if (item.type === "message")
      for (const p of item.content ?? [])
        if (p.type === "output_text") c2 += p.text ?? "";
  }
  return {
    turn1Encrypted: true,
    turn2Ok: c2.length > 0,
    detail: c2 ? `T2: ${truncate(c2, 80)}` : "T2 empty",
  };
}

async function encryptedMessages(model: Model): Promise<EncResult> {
  const t1 = await messagesNonStream(model);
  if (!t1.encrypted)
    return { turn1Encrypted: false, turn2Ok: false, detail: "T1: no signature" };

  const msgs = [
    { role: "user", content: Q1 },
    { role: "assistant", content: t1.rawContent },
    { role: "user", content: Q2 },
  ];
  const resp = await timedFetch(`${BASE_URL}/messages`, {
    method: "POST",
    headers: authHeaders("messages"),
    body: JSON.stringify(messagesBody(model, msgs, false)),
  });
  const body = await resp.json();
  if (!resp.ok)
    return {
      turn1Encrypted: true,
      turn2Ok: false,
      detail: `T2 HTTP ${resp.status}: ${JSON.stringify(body).slice(0, 200)}`,
    };

  let c2 = "";
  for (const b of body.content ?? [])
    if (b.type === "text") c2 += b.text ?? "";

  return {
    turn1Encrypted: true,
    turn2Ok: c2.length > 0,
    detail: c2 ? `T2: ${truncate(c2, 80)}` : "T2 empty",
  };
}

// ─── Test Runner ────────────────────────────────────────────────────────────

async function runCell(model: Model, format: Format): Promise<TestResult> {
  const r: TestResult = {
    model,
    format,
    nsVerdict: "ERROR",
    nsDetail: "",
    sVerdict: "ERROR",
    sDetail: "",
    eVerdict: "ERROR",
    eDetail: "",
  };

  // ── Non-streaming ──
  try {
    const fn =
      format === "chat_completions"
        ? chatNonStream
        : format === "responses"
          ? responsesNonStream
          : messagesNonStream;
    const res = await fn(model);
    const hasReasoning = !!(res.reasoning || res.encrypted);
    r.nsVerdict = hasReasoning ? "PASS" : "FAIL";
    r.nsDetail = hasReasoning
      ? `reasoning=${truncate(res.reasoning || "(encrypted-only)", 60)} enc=${res.encrypted ? "Y" : "N"}`
      : `no reasoning. content=${truncate(res.content, 60)}`;
  } catch (e: any) {
    r.nsDetail = truncate(e.message, 120);
  }

  // ── Streaming ──
  try {
    const fn =
      format === "chat_completions"
        ? chatStream
        : format === "responses"
          ? responsesStream
          : messagesStream;
    const res = await fn(model);
    const hasReasoning = !!(res.reasoning || res.encrypted);
    r.sVerdict = hasReasoning ? "PASS" : "FAIL";
    r.sDetail = hasReasoning
      ? `reasoning=${truncate(res.reasoning || "(encrypted-only)", 60)} enc=${res.encrypted ? "Y" : "N"}`
      : `no reasoning. content=${truncate(res.content, 60)}`;
  } catch (e: any) {
    r.sDetail = truncate(e.message, 120);
  }

  // ── Encrypted persistence ──
  try {
    const fn =
      format === "chat_completions"
        ? encryptedChat
        : format === "responses"
          ? encryptedResponses
          : encryptedMessages;
    const res = await fn(model);
    if (!res.turn1Encrypted) {
      r.eVerdict = "FAIL";
      r.eDetail = res.detail;
    } else {
      r.eVerdict = res.turn2Ok ? "PASS" : "FAIL";
      r.eDetail = res.detail;
    }
  } catch (e: any) {
    r.eDetail = truncate(e.message, 120);
  }

  return r;
}

// ─── Output ─────────────────────────────────────────────────────────────────

const FMT_LABELS: Record<Format, string> = {
  chat_completions: "chat/comp",
  responses: "responses",
  messages: "messages",
};

function icon(v: Verdict): string {
  return v === "PASS" ? "✅" : v === "FAIL" ? "❌" : "💥";
}

function printResults(results: TestResult[]) {
  console.log("\n" + "═".repeat(110));
  console.log("  MONOIZE REASONING MATRIX — RESULTS");
  console.log("═".repeat(110));

  // ── Summary table ──
  console.log("\n  Legend: NS=Non-Stream  S=Stream  E=Encrypted-Persistence\n");

  const colW = 30;
  const hdr = "Model".padEnd(20) + FORMATS.map((f) => FMT_LABELS[f].padEnd(colW)).join("");
  console.log("  " + hdr);
  console.log("  " + " ".repeat(20) + FORMATS.map(() => "NS  S   E".padEnd(colW)).join(""));
  console.log("  " + "─".repeat(20 + colW * 3));

  for (const model of MODELS) {
    let line = model.padEnd(20);
    for (const format of FORMATS) {
      const r = results.find((x) => x.model === model && x.format === format)!;
      line += `${icon(r.nsVerdict)}  ${icon(r.sVerdict)}  ${icon(r.eVerdict)}`.padEnd(colW);
    }
    console.log("  " + line);
  }

  // ── Detail dump ──
  console.log("\n" + "─".repeat(110));
  console.log("  DETAILED RESULTS");
  console.log("─".repeat(110));

  for (const r of results) {
    console.log(`\n  ■ ${r.model} × ${FMT_LABELS[r.format]}`);
    console.log(`    ${icon(r.nsVerdict)} Non-Stream : ${r.nsDetail}`);
    console.log(`    ${icon(r.sVerdict)} Stream     : ${r.sDetail}`);
    console.log(`    ${icon(r.eVerdict)} Encrypted  : ${r.eDetail}`);
  }

  // ── Stats ──
  const total = results.length * 3;
  const pass = results.reduce(
    (n, r) =>
      n +
      (r.nsVerdict === "PASS" ? 1 : 0) +
      (r.sVerdict === "PASS" ? 1 : 0) +
      (r.eVerdict === "PASS" ? 1 : 0),
    0
  );
  const fail = results.reduce(
    (n, r) =>
      n +
      (r.nsVerdict === "FAIL" ? 1 : 0) +
      (r.sVerdict === "FAIL" ? 1 : 0) +
      (r.eVerdict === "FAIL" ? 1 : 0),
    0
  );
  const err = total - pass - fail;

  console.log(`\n  ═══ ${pass}/${total} PASS · ${fail} FAIL · ${err} ERROR ═══\n`);
}

// ─── Main ───────────────────────────────────────────────────────────────────

async function main() {
  console.log(`Monoize Reasoning Matrix Test`);
  console.log(`  endpoint : ${BASE_URL}`);
  console.log(`  models   : ${MODELS.join(", ")}`);
  console.log(`  formats  : ${FORMATS.join(", ")}`);
  console.log(`  tests    : ${MODELS.length * FORMATS.length * 3} total\n`);

  const results: TestResult[] = [];

  for (const model of MODELS) {
    // Run all 3 formats for this model in parallel
    const batch = await Promise.all(
      FORMATS.map(async (format) => {
        process.stdout.write(`  ⏳ ${model} × ${FMT_LABELS[format]}…`);
        const r = await runCell(model, format);
        console.log(
          ` ${icon(r.nsVerdict)}${icon(r.sVerdict)}${icon(r.eVerdict)}`
        );
        return r;
      })
    );
    results.push(...batch);
  }

  printResults(results);
}

main().catch((e) => {
  console.error("Fatal:", e);
  process.exitCode = 1;
});
