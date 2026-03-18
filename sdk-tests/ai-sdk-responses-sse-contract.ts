#!/usr/bin/env bun

import { createOpenResponses } from "@ai-sdk/open-responses";
import { generateText } from "ai";

const BASE_URL = requireEnv("AI_SDK_RESPONSES_BASE_URL");
const API_KEY = requireEnv("AI_SDK_RESPONSES_API_KEY");
const MODEL = process.env.AI_SDK_RESPONSES_MODEL ?? "gpt-5.4";
const TIMEOUT_MS = Number(process.env.AI_SDK_RESPONSES_TIMEOUT_MS ?? "120000");

interface ParsedSSEEvent {
  event: string;
  data: unknown;
}

interface EventSummary {
  index: number;
  event: string;
  outputIndex: number | null;
  sequenceNumber: number | null;
}

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

function redactSecret(_value: string): string {
  return "[redacted-secret]";
}

function redactUrl(_value: string): string {
  return "[redacted-base-url]";
}

function safeJson(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, current) => {
      if (typeof current !== "string") return current;
      if (current === API_KEY) return "[redacted-api-key]";
      if (current === BASE_URL) return "[redacted-base-url]";
      return current;
    },
    2,
  );
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function timedFetch(url: string, init: RequestInit): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    return await fetch(url, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
  }
}

function responsesRequestBody() {
  return {
    model: MODEL,
    stream: true,
    reasoning: { effort: "high" },
    parallel_tool_calls: true,
    input: [
      {
        type: "message",
        role: "user",
        content: [
          {
            type: "input_text",
            text: "Think briefly, call get_weather for tokyo, then answer in one sentence.",
          },
        ],
      },
    ],
    tools: [
      {
        type: "function",
        name: "get_weather",
        description: "Get weather by city.",
        parameters: {
          type: "object",
          properties: {
            city: { type: "string" },
          },
          required: ["city"],
          additionalProperties: false,
        },
      },
    ],
  };
}

async function verifyAISDKReachability() {
  const provider = createOpenResponses({
    name: "monoize",
    url: new URL("responses", BASE_URL.endsWith("/") ? BASE_URL : `${BASE_URL}/`).toString(),
    apiKey: API_KEY,
  });

  const result = await generateText({
    model: provider(MODEL),
    prompt: "Reply with the single word ok.",
    maxOutputTokens: 16,
  });

  assert(result.text.trim().length > 0, "AI SDK generateText returned empty text");
}

async function parseSSE(resp: Response): Promise<ParsedSSEEvent[]> {
  const body = resp.body;
  assert(body, "Responses stream returned no body");

  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const events: ParsedSSEEvent[] = [];

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });

    while (true) {
      const boundary = buffer.indexOf("\n\n");
      if (boundary === -1) break;
      const frame = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      const parsed = parseSSEFrame(frame);
      if (parsed) events.push(parsed);
    }
  }

  if (buffer.trim()) {
    const parsed = parseSSEFrame(buffer);
    if (parsed) events.push(parsed);
  }

  return events;
}

function parseSSEFrame(frame: string): ParsedSSEEvent | null {
  let event = "message";
  const dataLines: string[] = [];

  for (const rawLine of frame.split("\n")) {
    const line = rawLine.trimEnd();
    if (!line) continue;
    if (line.startsWith("event:")) {
      event = line.slice(6).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice(5).trim());
    }
  }

  if (dataLines.length === 0) return null;
  const dataText = dataLines.join("\n");
  if (dataText === "[DONE]") {
    return { event, data: "[DONE]" };
  }

  try {
    return { event, data: JSON.parse(dataText) };
  } catch {
    throw new Error(`Failed to parse SSE JSON frame for event ${event}`);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function summarizeEvent(event: ParsedSSEEvent, index: number): EventSummary {
  const data = isRecord(event.data) ? event.data : null;
  return {
    index,
    event: event.event,
    outputIndex: typeof data?.output_index === "number" ? data.output_index : null,
    sequenceNumber: typeof data?.sequence_number === "number" ? data.sequence_number : null,
  };
}

function validateResponsesContract(events: ParsedSSEEvent[]) {
  const created = events.find((evt) => evt.event === "response.created");
  const inProgress = events.find((evt) => evt.event === "response.in_progress");
  const outputTextDoneEvents = events.filter((evt) => evt.event === "response.output_text.done");
  const functionDoneEvents = events.filter((evt) => evt.event === "response.function_call_arguments.done");

  assert(created && isRecord(created.data), "Missing response.created event");
  assert(inProgress && isRecord(inProgress.data), "Missing response.in_progress event");
  assert(
    outputTextDoneEvents.length > 0 || functionDoneEvents.length > 0,
    "Missing terminal content completion event (expected response.output_text.done or response.function_call_arguments.done)",
  );

  const createdResponse = created.data.response;
  const inProgressResponse = inProgress.data.response;
  assert(isRecord(createdResponse), "response.created missing nested response wrapper");
  assert(isRecord(inProgressResponse), "response.in_progress missing nested response wrapper");
  assert(typeof createdResponse.created_at === "number", "response.created missing created_at");
  assert(typeof inProgressResponse.created_at === "number", "response.in_progress missing created_at");

  for (const outputTextDone of outputTextDoneEvents) {
    assert(isRecord(outputTextDone.data), "output_text.done payload must be an object");
    assert(typeof outputTextDone.data.output_index === "number", "output_text.done missing output_index");
    assert(typeof outputTextDone.data.content_index === "number", "output_text.done missing content_index");
    assert(typeof outputTextDone.data.item_id === "string", "output_text.done missing item_id");
    assert(typeof outputTextDone.data.text === "string", "output_text.done missing text");
    assert(Object.hasOwn(outputTextDone.data, "logprobs"), "output_text.done missing logprobs");
  }

  for (const functionDone of functionDoneEvents) {
    assert(isRecord(functionDone.data), "function_call_arguments.done payload must be an object");
    assert(typeof functionDone.data.output_index === "number", "function_call_arguments.done missing output_index");
    assert(typeof functionDone.data.item_id === "string", "function_call_arguments.done missing item_id");
    assert(typeof functionDone.data.name === "string", "function_call_arguments.done missing name");
    assert(typeof functionDone.data.call_id === "string", "function_call_arguments.done missing call_id");
    assert(typeof functionDone.data.arguments === "string", "function_call_arguments.done missing arguments");
  }

  const outputDoneIndex = new Map<number, number>();
  for (const [i, evt] of events.entries()) {
    if (evt.event !== "response.output_item.done" || !isRecord(evt.data)) continue;
    const outputIndex = evt.data.output_index;
    if (typeof outputIndex === "number") outputDoneIndex.set(outputIndex, i);
  }

  const childDoneEvents = new Set([
    "response.output_text.done",
    "response.content_part.done",
    "response.function_call_arguments.done",
    "response.reasoning.done",
    "response.reasoning_summary_text.done",
    "response.reasoning_summary_part.done",
  ]);

  const lateEvents: EventSummary[] = [];
  for (const [i, evt] of events.entries()) {
    if (!childDoneEvents.has(evt.event) || !isRecord(evt.data)) continue;
    const outputIndex = evt.data.output_index;
    if (typeof outputIndex !== "number") continue;
    const parentIndex = outputDoneIndex.get(outputIndex);
    if (parentIndex !== undefined && i > parentIndex) {
      lateEvents.push(summarizeEvent(evt, i));
    }
  }

  assert(lateEvents.length === 0, `Child done event arrived after output_item.done: ${safeJson(lateEvents)}`);
}

async function main() {
  await verifyAISDKReachability();

  const streamUrl = new URL("responses", BASE_URL.endsWith("/") ? BASE_URL : `${BASE_URL}/`).toString();
  const resp = await timedFetch(streamUrl, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${API_KEY}`,
      accept: "text/event-stream",
    },
    body: JSON.stringify(responsesRequestBody()),
  });

  if (!resp.ok) {
    throw new Error(
      `Responses SSE request failed: status=${resp.status} baseURL=${redactUrl(BASE_URL)} apiKey=${redactSecret(API_KEY)}`,
    );
  }

  const events = await parseSSE(resp);
  validateResponsesContract(events);

  console.log(
    `AI SDK Responses SSE contract test passed for model=${MODEL} baseURL=${redactUrl(BASE_URL)} apiKey=${redactSecret(API_KEY)}`,
  );
}

main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(message.replaceAll(API_KEY, "[redacted-api-key]").replaceAll(BASE_URL, "[redacted-base-url]"));
  process.exitCode = 1;
});
