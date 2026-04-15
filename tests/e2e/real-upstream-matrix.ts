#!/usr/bin/env bun

/**
 * Usage: MONOIZE_BASE_URL=http://127.0.0.1:18080 MONOIZE_API_KEY=sk-... bun run tests/e2e/real-upstream-matrix.ts
 */

import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
type RequestValue = unknown;

interface JsonObject {
  [key: string]: JsonValue;
}

interface RequestObject {
  [key: string]: unknown;
}

type Downstream = "chat" | "responses" | "messages";
interface Config {
  baseUrl: string;
  apiKey: string;
  gptModel: string;
  claudeModel: string;
  reqTimeoutMs: number;
  outDir: string;
  anthropicVersion: string;
}

interface ResponseCapture {
  status: number;
  bodyText: string;
  headersText: string;
  headers: Record<string, string>;
}

interface SseEventFrame {
  event: string;
  data: string;
  raw: string;
  json: JsonValue | null;
}

interface StreamCapture extends ResponseCapture {
  frames: SseEventFrame[];
  doneSeen: boolean;
}

interface ScenarioResult {
  status: number;
  ok: boolean;
  observed?: boolean;
  count?: number;
  details?: string;
}

interface TextExpectationAssessment {
  ok: boolean;
  mode: "exact-answer" | "substring-match" | "missing-required-substrings";
  requiredSubstringHits: number;
  requiredSubstringCount: number;
  normalizedText: string;
  preview: string;
}

interface SuiteResult {
  downstream: Downstream;
  model: string;
  basic: ScenarioResult;
  stream: ScenarioResult;
  reasoning: ScenarioResult;
  encrypted_reasoning: ScenarioResult;
  reasoning_stream: ScenarioResult;
  multimodal: ScenarioResult;
  tool_call: ScenarioResult;
  tool_result_roundtrip: ScenarioResult;
  parallel_tool_call: ScenarioResult;
  parallel_tool_roundtrip: ScenarioResult;
  tool_stream: ScenarioResult;
}

interface ModelMatrixResult {
  label: string;
  model: string;
  downstreams: {
    chat: SuiteResult;
    responses: SuiteResult;
    messages: SuiteResult;
  };
}

interface SummaryResult {
  base_url: string;
  out_dir: string;
  generated_at: string;
  models: ModelMatrixResult[];
  all_critical_passed: boolean;
}

type ChatContentPart =
  | { type: "text"; text: string }
  | { type: "image_url"; image_url: { url: string } };

interface ChatToolCall {
  id: string;
  type: "function";
  function: {
    name: string;
    arguments: string;
  };
}

interface ChatCompletionResponse {
  choices?: Array<{
    message?: {
      content?: string | null;
      tool_calls?: ChatToolCall[];
      reasoning?: string | null;
      reasoning_details?: JsonValue[];
    };
    delta?: {
      content?: string | null;
      tool_calls?: Array<Partial<ChatToolCall>>;
      reasoning?: string | null;
      reasoning_details?: JsonValue[];
    };
    finish_reason?: string | null;
  }>;
}

interface ResponsesOutputTextPart {
  type: "output_text";
  text: string;
}

interface ResponsesMessageItem {
  type: "message";
  content?: ResponsesOutputTextPart[];
}

interface ResponsesFunctionCallItem {
  type: "function_call";
  call_id: string;
  name: string;
  arguments: string;
}

interface ResponsesReasoningItem {
  type: "reasoning";
  summary?: JsonValue;
}

type ResponsesOutputItem = ResponsesMessageItem | ResponsesFunctionCallItem | ResponsesReasoningItem;

interface ResponsesResponse {
  output?: ResponsesOutputItem[];
}

type MessagesContentBlock =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string; signature?: string }
  | { type: "tool_use"; id: string; name: string; input?: JsonValue };

interface MessagesResponse {
  content?: MessagesContentBlock[];
}

interface ChatToolDefinition {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: JsonObject;
  };
}

interface ResponsesToolDefinition {
  type: "function";
  name: string;
  description: string;
  parameters: JsonObject;
}

interface MessagesToolDefinition {
  name: string;
  description: string;
  input_schema: JsonObject;
}

interface MessagesToolUse {
  type: "tool_use";
  id: string;
  name: string;
  input?: JsonValue;
}

interface MessagesToolUseRequest extends RequestObject {
  type: "tool_use";
  id: string;
  name: string;
  input?: unknown;
}

const ROOT_DIR = resolve(import.meta.dir, "..", "..");
const DEFAULT_OUT_DIR = join(ROOT_DIR, "tests", "e2e", ".out", `real-upstream-${timestampSlug()}`);
const PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO7Z0WQAAAAASUVORK5CYII=";
const MULTIMODAL_IMAGE_URL = "https://upload.wikimedia.org/wikipedia/commons/c/ca/1x1.png";
const WEATHER_RESULT_SENTINEL = "WEATHER_RESULT__TAIPEI__SUNNY_25C__MONOIZE_SENTINEL";
const WEBSEARCH_RESULT_SENTINEL = "WEBSEARCH_RESULT__MONOIZE__PROXY__MONOIZE_SENTINEL";
const SINGLE_TOOL_EXPECTED_ANSWER = `FINAL_ANSWER weather=${WEATHER_RESULT_SENTINEL}`;
const PARALLEL_TOOL_EXPECTED_ANSWER = `FINAL_ANSWER weather=${WEATHER_RESULT_SENTINEL} websearch=${WEBSEARCH_RESULT_SENTINEL}`;
const REASONING_STREAM_EXPECTED_ANSWER = "47";
const SINGLE_TOOL_REQUIRED_SUBSTRINGS = ["FINAL_ANSWER", `weather=${WEATHER_RESULT_SENTINEL}`] as const;
const PARALLEL_TOOL_REQUIRED_SUBSTRINGS = [
  "FINAL_ANSWER",
  `weather=${WEATHER_RESULT_SENTINEL}`,
  `websearch=${WEBSEARCH_RESULT_SENTINEL}`,
] as const;

const COLORS = {
  red: "\u001b[31m",
  green: "\u001b[32m",
  yellow: "\u001b[33m",
  blue: "\u001b[34m",
  cyan: "\u001b[36m",
  gray: "\u001b[90m",
  reset: "\u001b[0m",
} as const;

function timestampSlug(): string {
  const date = new Date();
  const year = String(date.getFullYear());
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const seconds = String(date.getSeconds()).padStart(2, "0");
  return `${year}${month}${day}-${hours}${minutes}${seconds}`;
}

function colorize(color: keyof typeof COLORS, text: string): string {
  return `${COLORS[color]}${text}${COLORS.reset}`;
}

function logSection(title: string): void {
  console.log(`\n${colorize("blue", `== ${title} ==`)}`);
}

function logScenario(label: string, result: ScenarioResult): void {
  const statusText = `${result.status}`;
  const verdict = result.ok || result.observed ? colorize("green", "PASS") : colorize("red", "FAIL");
  const detail = result.count !== undefined ? ` count=${result.count}` : result.details ? ` ${result.details}` : "";
  console.log(`  ${verdict} ${label} ${colorize("gray", `status=${statusText}${detail}`)}`);
}

function printHelp(): void {
  console.log(`Usage:
  MONOIZE_BASE_URL=http://127.0.0.1:18080 MONOIZE_API_KEY=sk-... bun run tests/e2e/real-upstream-matrix.ts

Required env:
  MONOIZE_BASE_URL   Base Monoize URL, with or without /v1
  MONOIZE_API_KEY    Forwarding API key

Optional env:
  GPT_MODEL          default: gpt-5.4-mini
  CLAUDE_MODEL       default: claude-sonnet-4.6
  REQ_TIMEOUT        default: 180 (seconds)
  OUT_DIR            default: tests/e2e/.out/real-upstream-<timestamp>
`);
}

function getConfig(): Config {
  const baseInput = process.env.MONOIZE_BASE_URL?.trim() ?? "";
  const apiKey = process.env.MONOIZE_API_KEY?.trim() ?? "";
  if (!baseInput) {
    throw new Error("MONOIZE_BASE_URL is required");
  }
  if (!apiKey) {
    throw new Error("MONOIZE_API_KEY is required");
  }

  const timeoutSeconds = Number(process.env.REQ_TIMEOUT ?? "180");
  if (!Number.isFinite(timeoutSeconds) || timeoutSeconds <= 0) {
    throw new Error("REQ_TIMEOUT must be a positive number of seconds");
  }

  return {
    baseUrl: normalizeBaseUrl(baseInput),
    apiKey,
    gptModel: process.env.GPT_MODEL?.trim() || "gpt-5.4-mini",
    claudeModel: process.env.CLAUDE_MODEL?.trim() || "claude-sonnet-4.6",
    reqTimeoutMs: Math.floor(timeoutSeconds * 1000),
    outDir: resolve(process.env.OUT_DIR?.trim() || DEFAULT_OUT_DIR),
    anthropicVersion: "2023-06-01",
  };
}

function normalizeBaseUrl(input: string): string {
  const trimmed = input.replace(/\/+$/, "");
  if (trimmed.endsWith("/v1")) {
    return trimmed;
  }
  return `${trimmed}/v1`;
}

function slugify(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9._-]/g, "-");
}

function headersToRecord(headers: Headers): Record<string, string> {
  const record: Record<string, string> = {};
  for (const [key, value] of headers.entries()) {
    record[key] = value;
  }
  return record;
}

function headersToText(headers: Headers): string {
  return Array.from(headers.entries())
    .map(([key, value]) => `${key}: ${value}`)
    .join("\n");
}

function safeJsonParse(text: string): JsonValue | null {
  try {
    return JSON.parse(text) as JsonValue;
  } catch {
    return null;
  }
}

function isRecord(value: JsonValue | unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function readArray(value: unknown): unknown[] | null {
  return Array.isArray(value) ? value : null;
}

function stringifyJson<T>(value: T): string {
  return JSON.stringify(value, null, 2);
}

function previewText(text: string, maxLength: number): string {
  return text.length <= maxLength ? text : `${text.slice(0, maxLength)}…`;
}

function unwrapMarkdownCodeFence(text: string): string {
  const trimmed = text.trim();
  const fenced = trimmed.match(/^```(?:json)?\s*([\s\S]*?)\s*```$/i);
  const body = fenced?.[1];
  return body ? body.trim() : trimmed;
}

function normalizeObservedText(text: string): string {
  return unwrapMarkdownCodeFence(text).trim();
}

function assessExpectedAnswerText(
  text: string,
  expectedAnswer: string,
  requiredSubstrings: readonly string[],
): TextExpectationAssessment {
  const normalizedText = normalizeObservedText(text);
  const requiredSubstringHits = requiredSubstrings.filter((value) => normalizedText.includes(value)).length;
  const exactAnswerMatch = normalizedText === expectedAnswer;
  const substringMatch = requiredSubstringHits === requiredSubstrings.length;
  const mode = exactAnswerMatch ? "exact-answer" : substringMatch ? "substring-match" : "missing-required-substrings";
  return {
    ok: exactAnswerMatch || substringMatch,
    mode,
    requiredSubstringHits,
    requiredSubstringCount: requiredSubstrings.length,
    normalizedText,
    preview: previewText(normalizedText, 240),
  };
}

function evaluationDetails(assessment: TextExpectationAssessment): string {
  return `mode=${assessment.mode} substrings=${assessment.requiredSubstringHits}/${assessment.requiredSubstringCount} preview=${JSON.stringify(assessment.preview)}`;
}

function scenarioResultFromAssessment(
  status: number,
  assessment: TextExpectationAssessment | null,
): ScenarioResult {
  if (!assessment) {
    return scenarioResult(status, false, "no-tool-calls");
  }
  return scenarioResult(status, status === 200 && assessment.ok, evaluationDetails(assessment));
}

async function ensureOutDir(dir: string): Promise<void> {
  mkdirSync(dir, { recursive: true });
}

async function writeCaptureFiles(
  outDir: string,
  model: string,
  downstream: Downstream,
  scenario: string,
  capture: ResponseCapture,
): Promise<void> {
  const prefix = `${slugify(model)}-${downstream}-${scenario}`;
  await Bun.write(join(outDir, `${prefix}.body`), capture.bodyText);
  await Bun.write(join(outDir, `${prefix}.headers`), `${capture.headersText}\n`);
}

function artifactPrefix(model: string, downstream: Downstream, scenario: string): string {
  return `${slugify(model)}-${downstream}-${scenario}`;
}

async function writeRequestArtifact(
  config: Config,
  model: string,
  downstream: Downstream,
  scenario: string,
  path: string,
  body: RequestValue,
  extraHeaders?: Record<string, string>,
): Promise<void> {
  const prefix = artifactPrefix(model, downstream, scenario);
  await Bun.write(
    join(config.outDir, `${prefix}.request.json`),
    stringifyJson({
      method: "POST",
      path,
      url: `${config.baseUrl}${path}`,
      headers: buildJsonHeaders(config, extraHeaders),
      body,
    }),
  );
}

async function writeAnalysisArtifact(
  outDir: string,
  model: string,
  downstream: Downstream,
  scenario: string,
  value: unknown,
): Promise<void> {
  const prefix = artifactPrefix(model, downstream, scenario);
  await Bun.write(join(outDir, `${prefix}.analysis.json`), stringifyJson(value));
}

async function captureScenarioRequest(
  config: Config,
  model: string,
  downstream: Downstream,
  scenario: string,
  path: string,
  body: RequestValue,
  extraHeaders?: Record<string, string>,
): Promise<ResponseCapture> {
  await writeRequestArtifact(config, model, downstream, scenario, path, body, extraHeaders);
  const capture = await captureRequest(config, path, body, extraHeaders);
  await writeCaptureFiles(config.outDir, model, downstream, scenario, capture);
  return capture;
}

async function captureScenarioStream(
  config: Config,
  model: string,
  downstream: Downstream,
  scenario: string,
  path: string,
  body: RequestValue,
  extraHeaders?: Record<string, string>,
): Promise<StreamCapture> {
  await writeRequestArtifact(config, model, downstream, scenario, path, body, extraHeaders);
  const capture = await captureStream(config, path, body, extraHeaders);
  await writeCaptureFiles(config.outDir, model, downstream, scenario, capture);
  return capture;
}

function buildSingleToolPrompt(): string {
  return [
    "Call exactly one weather tool for Taipei.",
    "Do not answer before tool results are provided.",
    `After tool results are provided, respond with exactly this text and nothing else: ${SINGLE_TOOL_EXPECTED_ANSWER}`,
    "Use the tool result string verbatim inside that final answer.",
    "No markdown. No JSON. No prose before or after.",
  ].join(" ");
}

function buildParallelToolPrompt(): string {
  return [
    "Return exactly two tool calls in one assistant turn: weather for Taipei and websearch for Monoize.",
    "Do not answer before tool results are provided.",
    `After tool results are provided, respond with exactly this text and nothing else: ${PARALLEL_TOOL_EXPECTED_ANSWER}`,
    "Use each tool result string verbatim inside that final answer.",
    "No markdown. No JSON. No prose before or after.",
  ].join(" ");
}

function buildReasoningStreamPrompt(): string {
  return [
    "Solve this high-school geometry problem carefully.",
    "Triangle ABC has side lengths AB=13, BC=14, and CA=15.",
    "Let D be the foot of the altitude from A to BC.",
    "Compute 7*BD + AD.",
    `Output only the final integer ${REASONING_STREAM_EXPECTED_ANSWER} with no words, no punctuation, no explanation, and no extra text.`,
  ].join(" ");
}

function responsesUserInputMessage(text: string): RequestObject {
  return {
    type: "message",
    role: "user",
    content: [{ type: "input_text", text }],
  };
}

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

function buildJsonHeaders(config: Config, extraHeaders?: Record<string, string>): HeadersInit {
  return {
    Authorization: `Bearer ${config.apiKey}`,
    "Content-Type": "application/json",
    ...extraHeaders,
  };
}

async function captureRequest(
  config: Config,
  path: string,
  body: RequestValue,
  extraHeaders?: Record<string, string>,
): Promise<ResponseCapture> {
  const response = await fetchWithTimeout(
    `${config.baseUrl}${path}`,
    {
      method: "POST",
      headers: buildJsonHeaders(config, extraHeaders),
      body: JSON.stringify(body),
    },
    config.reqTimeoutMs,
  );
  const bodyText = await response.text();
  return {
    status: response.status,
    bodyText,
    headersText: headersToText(response.headers),
    headers: headersToRecord(response.headers),
  };
}

function parseSseBlock(block: string): SseEventFrame | null {
  if (!block.trim()) {
    return null;
  }

  let event = "";
  const dataLines: string[] = [];

  for (const line of block.split("\n")) {
    if (line.startsWith(":")) {
      continue;
    }
    if (line.startsWith("event:")) {
      event = line.slice("event:".length).trim();
      continue;
    }
    if (line.startsWith("data:")) {
      dataLines.push(line.slice("data:".length).trimStart());
    }
  }

  const data = dataLines.join("\n");
  return {
    event,
    data,
    raw: block,
    json: data && data !== "[DONE]" ? safeJsonParse(data) : null,
  };
}

async function captureStream(
  config: Config,
  path: string,
  body: RequestValue,
  extraHeaders?: Record<string, string>,
): Promise<StreamCapture> {
  const response = await fetchWithTimeout(
    `${config.baseUrl}${path}`,
    {
      method: "POST",
      headers: buildJsonHeaders(config, extraHeaders),
      body: JSON.stringify(body),
    },
    config.reqTimeoutMs,
  );

  const headersText = headersToText(response.headers);
  const headers = headersToRecord(response.headers);
  const frames: SseEventFrame[] = [];
  const decoder = new TextDecoder();
  let rawText = "";
  let doneSeen = false;

  if (response.body) {
    const reader = response.body.getReader();
    let buffer = "";

    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      const chunk = decoder.decode(value, { stream: true }).replaceAll("\r", "");
      rawText += chunk;
      buffer += chunk;

      let boundary = buffer.indexOf("\n\n");
      while (boundary !== -1) {
        const block = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const frame = parseSseBlock(block);
        if (frame) {
          if (frame.data === "[DONE]") {
            doneSeen = true;
          }
          frames.push(frame);
        }
        boundary = buffer.indexOf("\n\n");
      }
    }

    buffer += decoder.decode().replaceAll("\r", "");
    rawText += decoder.decode();
    let boundary = buffer.indexOf("\n\n");
    while (boundary !== -1) {
      const block = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      const frame = parseSseBlock(block);
      if (frame) {
        if (frame.data === "[DONE]") {
          doneSeen = true;
        }
        frames.push(frame);
      }
      boundary = buffer.indexOf("\n\n");
    }
    if (buffer.trim()) {
      const frame = parseSseBlock(buffer);
      if (frame) {
        if (frame.data === "[DONE]") {
          doneSeen = true;
        }
        frames.push(frame);
      }
    }
  }

  return {
    status: response.status,
    bodyText: rawText,
    headersText,
    headers,
    frames,
    doneSeen,
  };
}

function parseChatCompletionResponse(text: string): ChatCompletionResponse | null {
  const parsed = safeJsonParse(text);
  if (!isRecord(parsed)) {
    return null;
  }
  return parsed as unknown as ChatCompletionResponse;
}

function parseResponsesResponse(text: string): ResponsesResponse | null {
  const parsed = safeJsonParse(text);
  if (!isRecord(parsed)) {
    return null;
  }
  return parsed as unknown as ResponsesResponse;
}

function parseMessagesResponse(text: string): MessagesResponse | null {
  const parsed = safeJsonParse(text);
  if (!isRecord(parsed)) {
    return null;
  }
  return parsed as unknown as MessagesResponse;
}

function getChatOutputText(response: ChatCompletionResponse | null): string {
  const choices = response?.choices;
  if (!choices || choices.length === 0) {
    return "";
  }
  const content = choices[0]?.message?.content;
  return typeof content === "string" ? content : "";
}

function getChatToolCalls(response: ChatCompletionResponse | null): ChatToolCall[] {
  const choices = response?.choices;
  if (!choices || choices.length === 0) {
    return [];
  }
  return Array.isArray(choices[0]?.message?.tool_calls) ? choices[0].message.tool_calls ?? [] : [];
}

function chatReasoningPresent(response: ChatCompletionResponse | null): boolean {
  const choices = response?.choices;
  if (!choices || choices.length === 0) {
    return false;
  }
  const message = choices[0]?.message;
  return Boolean((message?.reasoning && message.reasoning.length > 0) || (message?.reasoning_details && message.reasoning_details.length > 0));
}

function getResponsesOutputText(response: ResponsesResponse | null): string {
  const output = response?.output;
  if (!output) {
    return "";
  }
  for (const item of output) {
    if (item.type !== "message" || !item.content) {
      continue;
    }
    for (const part of item.content) {
      if (part.type === "output_text") {
        return part.text;
      }
    }
  }
  return "";
}

function getResponsesFunctionCalls(response: ResponsesResponse | null): ResponsesFunctionCallItem[] {
  const output = response?.output;
  if (!output) {
    return [];
  }
  const calls: ResponsesFunctionCallItem[] = [];
  for (const item of output) {
    if (item.type === "function_call") {
      calls.push(item);
    }
  }
  return calls;
}

function responsesReasoningPresent(response: ResponsesResponse | null): boolean {
  const output = response?.output;
  if (!output) {
    return false;
  }
  return output.some((item) => item.type === "reasoning");
}

function getMessagesText(response: MessagesResponse | null): string {
  const content = response?.content;
  if (!content) {
    return "";
  }
  for (const block of content) {
    if (block.type === "text") {
      return block.text;
    }
  }
  return "";
}

function getMessagesToolUses(response: MessagesResponse | null): MessagesToolUse[] {
  const content = response?.content;
  if (!content) {
    return [];
  }
  const uses: MessagesToolUse[] = [];
  for (const block of content) {
    if (block.type === "tool_use") {
      uses.push(block);
    }
  }
  return uses;
}

function getChatStreamOutputText(frames: SseEventFrame[]): string {
  let text = "";
  for (const frame of frames) {
    const root = frameJsonRecord(frame);
    const choices = readArray(root?.choices);
    if (!choices || choices.length === 0) {
      continue;
    }
    const choice = choices[0];
    if (!isRecord(choice)) {
      continue;
    }
    const delta = isRecord(choice.delta) ? choice.delta : null;
    const chunk = readString(delta?.content);
    if (chunk) {
      text += chunk;
    }
  }
  return text;
}

function getResponsesStreamOutputText(frames: SseEventFrame[]): string {
  let text = "";
  for (const frame of frames) {
    const root = frameJsonRecord(frame);
    if (frame.event === "response.output_text.delta") {
      const chunk = readString(root?.delta);
      if (chunk) {
        text += chunk;
      }
      continue;
    }
    if (frame.event === "response.output_text.done" && text.length === 0) {
      const doneText = readString(root?.text);
      if (doneText) {
        text = doneText;
      }
    }
  }
  return text;
}

function getMessagesStreamOutputText(frames: SseEventFrame[]): string {
  let text = "";
  for (const frame of frames) {
    if (frame.event !== "content_block_delta") {
      continue;
    }
    const root = frameJsonRecord(frame);
    const delta = isRecord(root?.delta) ? root.delta : null;
    if (readString(delta?.type) !== "text_delta") {
      continue;
    }
    const chunk = readString(delta?.text);
    if (chunk) {
      text += chunk;
    }
  }
  return text;
}

function toMessagesToolUseRequests(toolUses: MessagesToolUse[]): MessagesToolUseRequest[] {
  return toolUses.map((toolUse) => ({
    type: toolUse.type,
    id: toolUse.id,
    name: toolUse.name,
    input: toolUse.input,
  }));
}

function messagesReasoningPresent(response: MessagesResponse | null): boolean {
  const content = response?.content;
  if (!content) {
    return false;
  }
  return content.some((block) => block.type === "thinking");
}

function messagesEncryptedReasoningPresent(response: MessagesResponse | null): boolean {
  const content = response?.content;
  if (!content) {
    return false;
  }
  return content.some((block) => block.type === "thinking" && typeof block.signature === "string" && block.signature.length > 0);
}

function scenarioResult(status: number, ok: boolean, details?: string, count?: number): ScenarioResult {
  return { status, ok, details, count };
}

function observedResult(status: number, observed: boolean, details?: string): ScenarioResult {
  return { status, ok: observed, observed, details };
}

function hasJsonEvent(frames: SseEventFrame[], eventName: string): boolean {
  return frames.some((frame) => frame.event === eventName);
}

function hasFrameMatching(frames: SseEventFrame[], predicate: (frame: SseEventFrame) => boolean): boolean {
  return frames.some(predicate);
}

function frameJsonRecord(frame: SseEventFrame): Record<string, unknown> | null {
  return isRecord(frame.json) ? frame.json : null;
}

function chatStreamHasFinishReason(frames: SseEventFrame[]): boolean {
  return hasFrameMatching(frames, (frame) => {
    const root = frameJsonRecord(frame);
    const choices = readArray(root?.choices);
    if (!choices || choices.length === 0) {
      return false;
    }
    const choice = choices[0];
    if (!isRecord(choice)) {
      return false;
    }
    return readString(choice.finish_reason) !== null;
  });
}

function chatStreamHasReasoning(frames: SseEventFrame[]): boolean {
  return hasFrameMatching(frames, (frame) => {
    const root = frameJsonRecord(frame);
    const choices = readArray(root?.choices);
    if (!choices || choices.length === 0) {
      return false;
    }
    const choice = choices[0];
    if (!isRecord(choice)) {
      return false;
    }
    const delta = isRecord(choice.delta) ? choice.delta : null;
    if (!delta) {
      return false;
    }
    return Boolean(readString(delta.reasoning) || (Array.isArray(delta.reasoning_details) && delta.reasoning_details.length > 0));
  });
}

function chatStreamHasEncryptedReasoning(frames: SseEventFrame[]): boolean {
  return hasFrameMatching(frames, (frame) => frame.data.includes("signature") || frame.data.includes("encrypted") || frame.data.includes("reasoning_details"));
}

function chatStreamHasToolCalls(frames: SseEventFrame[]): boolean {
  return hasFrameMatching(frames, (frame) => {
    const root = frameJsonRecord(frame);
    const choices = readArray(root?.choices);
    if (!choices || choices.length === 0) {
      return false;
    }
    const choice = choices[0];
    if (!isRecord(choice)) {
      return false;
    }
    const delta = isRecord(choice.delta) ? choice.delta : null;
    if (!delta) {
      return false;
    }
    const toolCalls = readArray(delta.tool_calls);
    return Boolean(toolCalls && toolCalls.length > 0);
  });
}

function responsesStreamHasEvent(frames: SseEventFrame[], names: string[]): boolean {
  return frames.some((frame) => names.includes(frame.event));
}

function responsesStreamHasReasoning(frames: SseEventFrame[]): boolean {
  return frames.some((frame) => frame.event.includes("reasoning") || frame.data.includes("reasoning"));
}

function responsesStreamHasToolCalls(frames: SseEventFrame[]): boolean {
  return frames.some(
    (frame) =>
      frame.event === "response.function_call_arguments.delta" ||
      frame.event === "response.output_item.added" ||
      frame.data.includes("function_call"),
  );
}

function messagesStreamHasTerminal(frames: SseEventFrame[]): boolean {
  return hasJsonEvent(frames, "message_stop");
}

function messagesStreamHasReasoning(frames: SseEventFrame[]): boolean {
  return frames.some(
    (frame) =>
      frame.event === "content_block_delta" &&
      (() => {
        const root = frameJsonRecord(frame);
        const delta = isRecord(root?.delta) ? root.delta : null;
        return readString(delta?.type) === "thinking_delta" || readString(delta?.type) === "signature_delta";
      })(),
  );
}

function messagesStreamHasToolUse(frames: SseEventFrame[]): boolean {
  return frames.some((frame) => {
    if (frame.event !== "content_block_start" && frame.event !== "content_block_delta") {
      return false;
    }
    const root = frameJsonRecord(frame);
    const contentBlock = isRecord(root?.content_block) ? root.content_block : null;
    const delta = isRecord(root?.delta) ? root.delta : null;
    return readString(contentBlock?.type) === "tool_use" || readString(delta?.type) === "input_json_delta";
  });
}

function getCriticalScenarios(suite: SuiteResult): ScenarioResult[] {
  return [
    suite.basic,
    suite.stream,
    suite.reasoning,
    suite.multimodal,
    suite.tool_call,
    suite.tool_result_roundtrip,
    suite.parallel_tool_call,
    suite.parallel_tool_roundtrip,
    suite.tool_stream,
  ];
}

function chatWeatherTool(): ChatToolDefinition {
  return {
    type: "function",
    function: {
      name: "weather",
      description: "Get weather",
      parameters: {
        type: "object",
        properties: { city: { type: "string" } },
        required: ["city"],
      },
    },
  };
}

function chatWebsearchTool(): ChatToolDefinition {
  return {
    type: "function",
    function: {
      name: "websearch",
      description: "Search web",
      parameters: {
        type: "object",
        properties: { query: { type: "string" } },
        required: ["query"],
      },
    },
  };
}

function responsesWeatherTool(): ResponsesToolDefinition {
  return {
    type: "function",
    name: "weather",
    description: "Get weather",
    parameters: {
      type: "object",
      properties: { city: { type: "string" } },
      required: ["city"],
    },
  };
}

function responsesWebsearchTool(): ResponsesToolDefinition {
  return {
    type: "function",
    name: "websearch",
    description: "Search web",
    parameters: {
      type: "object",
      properties: { query: { type: "string" } },
      required: ["query"],
    },
  };
}

function messagesWeatherTool(): MessagesToolDefinition {
  return {
    name: "weather",
    description: "Get weather",
    input_schema: {
      type: "object",
      properties: { city: { type: "string" } },
      required: ["city"],
    },
  };
}

function messagesWebsearchTool(): MessagesToolDefinition {
  return {
    name: "websearch",
    description: "Search web",
    input_schema: {
      type: "object",
      properties: { query: { type: "string" } },
      required: ["query"],
    },
  };
}

async function runChatSuite(config: Config, model: string): Promise<SuiteResult> {
  const downstream: Downstream = "chat";
  const path = "/chat/completions";
  const reasoningStreamPrompt = buildReasoningStreamPrompt();

  const basicReq: RequestObject = {
    model,
    messages: [{ role: "user", content: "Reply with exactly: chat-basic-ok" }],
    max_tokens: 64,
  };
  const basic = await captureRequest(config, path, basicReq);
  await writeCaptureFiles(config.outDir, model, downstream, "basic", basic);
  const basicJson = parseChatCompletionResponse(basic.bodyText);

  const streamReq: RequestObject = {
    model,
    stream: true,
    messages: [{ role: "user", content: "Reply with exactly: chat-stream-ok" }],
    max_tokens: 64,
  };
  const stream = await captureStream(config, path, streamReq);
  await writeCaptureFiles(config.outDir, model, downstream, "stream", stream);

  const reasoningReq: RequestObject = {
    model,
    reasoning_effort: "high",
    messages: [{ role: "user", content: "Think briefly and reply with exactly: chat-reasoning-ok" }],
    max_tokens: 128,
  };
  const reasoning = await captureRequest(config, path, reasoningReq);
  await writeCaptureFiles(config.outDir, model, downstream, "reasoning", reasoning);
  const reasoningJson = parseChatCompletionResponse(reasoning.bodyText);

  const reasoningStreamReq: RequestObject = {
    model,
    stream: true,
    reasoning_effort: "xhigh",
    messages: [{ role: "user", content: reasoningStreamPrompt }],
    max_tokens: 256,
  };
  const reasoningStream = await captureStream(config, path, reasoningStreamReq);
  await writeCaptureFiles(config.outDir, model, downstream, "reasoning_stream", reasoningStream);
  const reasoningStreamText = getChatStreamOutputText(reasoningStream.frames);

  const multimodalReq: RequestObject = {
    model,
    messages: [
      {
        role: "user",
        content: [
          { type: "text", text: "Describe this image in one short sentence." },
          { type: "image_url", image_url: { url: MULTIMODAL_IMAGE_URL } },
        ],
      },
    ],
    max_tokens: 128,
  };
  const multimodal = await captureRequest(config, path, multimodalReq);
  await writeCaptureFiles(config.outDir, model, downstream, "multimodal", multimodal);
  const multimodalJson = parseChatCompletionResponse(multimodal.bodyText);

  const toolReq: RequestObject = {
    model,
    tool_choice: "required",
    parallel_tool_calls: false,
    tools: [chatWeatherTool()],
    messages: [{ role: "user", content: "Call the weather tool for Taipei and do not answer directly." }],
    max_tokens: 128,
  };
  const tool1 = await captureRequest(config, path, toolReq);
  await writeCaptureFiles(config.outDir, model, downstream, "tool_call", tool1);
  const tool1Json = parseChatCompletionResponse(tool1.bodyText);
  const chatToolCalls = getChatToolCalls(tool1Json);

  let tool2Status = 0;
  let tool2Ok = false;
  if (chatToolCalls.length > 0) {
    const tool2Req: RequestObject = {
      model,
      messages: [
        { role: "assistant", content: "", tool_calls: chatToolCalls },
        ...chatToolCalls.map((call) => ({
          role: "tool" as const,
          tool_call_id: call.id,
          content: "WEATHER_RESULT: sunny 25C",
        })),
      ],
      max_tokens: 128,
    };
    const tool2 = await captureRequest(config, path, tool2Req);
    await writeCaptureFiles(config.outDir, model, downstream, "tool_result_roundtrip", tool2);
    tool2Status = tool2.status;
    tool2Ok = tool2.status === 200 && getChatOutputText(parseChatCompletionResponse(tool2.bodyText)).length > 0;
  }

  const parallelReq: RequestObject = {
    model,
    tool_choice: "required",
    parallel_tool_calls: true,
    tools: [chatWeatherTool(), chatWebsearchTool()],
    messages: [
      {
        role: "user",
        content:
          "Return exactly two tool calls in the same assistant turn: weather for Taipei and websearch for Monoize. Do not answer in natural language.",
      },
    ],
    max_tokens: 256,
  };
  const parallel1 = await captureRequest(config, path, parallelReq);
  await writeCaptureFiles(config.outDir, model, downstream, "parallel_tool_call", parallel1);
  const parallel1Json = parseChatCompletionResponse(parallel1.bodyText);
  const parallelCalls = getChatToolCalls(parallel1Json);

  let parallel2Status = 0;
  let parallel2Ok = false;
  if (parallelCalls.length > 0) {
    const parallel2Req: RequestObject = {
      model,
      messages: [
        { role: "assistant", content: "", tool_calls: parallelCalls },
        ...parallelCalls.map((call) => ({
          role: "tool" as const,
          tool_call_id: call.id,
          content: call.function.name === "weather" ? "WEATHER_RESULT: sunny 25C" : "WEB_RESULT: Monoize proxy",
        })),
      ],
      max_tokens: 128,
    };
    const parallel2 = await captureRequest(config, path, parallel2Req);
    await writeCaptureFiles(config.outDir, model, downstream, "parallel_tool_roundtrip", parallel2);
    parallel2Status = parallel2.status;
    parallel2Ok = parallel2.status === 200 && getChatOutputText(parseChatCompletionResponse(parallel2.bodyText)).length > 0;
  }

  const toolStreamReq: RequestObject = {
    ...parallelReq,
    stream: true,
  };
  const toolStream = await captureStream(config, path, toolStreamReq);
  await writeCaptureFiles(config.outDir, model, downstream, "tool_stream", toolStream);

  return {
    downstream,
    model,
    basic: scenarioResult(basic.status, basic.status === 200 && getChatOutputText(basicJson) === "chat-basic-ok"),
    stream: scenarioResult(stream.status, stream.status === 200 && stream.doneSeen && chatStreamHasFinishReason(stream.frames)),
    reasoning: {
      status: reasoning.status,
      ok: reasoning.status === 200 && getChatOutputText(reasoningJson) === "chat-reasoning-ok",
      observed: chatReasoningPresent(reasoningJson),
    },
    encrypted_reasoning: observedResult(reasoningStream.status, chatStreamHasEncryptedReasoning(reasoningStream.frames)),
    reasoning_stream: scenarioResult(
      reasoningStream.status,
      reasoningStream.status === 200 && normalizeObservedText(reasoningStreamText) === REASONING_STREAM_EXPECTED_ANSWER,
      `preview=${JSON.stringify(previewText(normalizeObservedText(reasoningStreamText), 120))}`,
    ),
    multimodal: scenarioResult(multimodal.status, multimodal.status === 200 && getChatOutputText(multimodalJson).length > 0),
    tool_call: scenarioResult(tool1.status, tool1.status === 200 && chatToolCalls.length >= 1, undefined, chatToolCalls.length),
    tool_result_roundtrip: scenarioResult(tool2Status, tool2Ok),
    parallel_tool_call: scenarioResult(parallel1.status, parallel1.status === 200 && parallelCalls.length >= 2, undefined, parallelCalls.length),
    parallel_tool_roundtrip: scenarioResult(parallel2Status, parallel2Ok),
    tool_stream: observedResult(toolStream.status, toolStream.status === 200 && chatStreamHasToolCalls(toolStream.frames)),
  };
}

async function runResponsesSuite(config: Config, model: string): Promise<SuiteResult> {
  const downstream: Downstream = "responses";
  const path = "/responses";
  const singleToolPrompt = buildSingleToolPrompt();
  const parallelToolPrompt = buildParallelToolPrompt();
  const reasoningStreamPrompt = buildReasoningStreamPrompt();

  const basicReq: RequestObject = { model, input: "Reply with exactly: responses-basic-ok", max_output_tokens: 64 };
  const basic = await captureScenarioRequest(config, model, downstream, "basic", path, basicReq);
  const basicJson = parseResponsesResponse(basic.bodyText);

  const streamReq: RequestObject = { model, stream: true, input: "Reply with exactly: responses-stream-ok", max_output_tokens: 64 };
  const stream = await captureScenarioStream(config, model, downstream, "stream", path, streamReq);

  const reasoningReq: RequestObject = {
    model,
    reasoning: { effort: "high" },
    input: "Think briefly and reply with exactly: responses-reasoning-ok",
    max_output_tokens: 128,
  };
  const reasoning = await captureScenarioRequest(config, model, downstream, "reasoning", path, reasoningReq);
  const reasoningJson = parseResponsesResponse(reasoning.bodyText);

  const reasoningStreamReq: RequestObject = {
    model,
    stream: true,
    reasoning: { effort: "xhigh" },
    input: reasoningStreamPrompt,
    max_output_tokens: 256,
  };
  const reasoningStream = await captureScenarioStream(config, model, downstream, "reasoning_stream", path, reasoningStreamReq);
  const reasoningStreamText = getResponsesStreamOutputText(reasoningStream.frames);

  const multimodalReq: RequestObject = {
    model,
    input: [
      {
          type: "message",
          role: "user",
          content: [
            { type: "input_text", text: "Describe this image in one short sentence." },
            { type: "input_image", image_url: MULTIMODAL_IMAGE_URL },
          ],
        },
      ],
    max_output_tokens: 128,
  };
  const multimodal = await captureScenarioRequest(config, model, downstream, "multimodal", path, multimodalReq);
  const multimodalJson = parseResponsesResponse(multimodal.bodyText);

  const toolReq: RequestObject = {
    model,
    tool_choice: "required",
    parallel_tool_calls: false,
    tools: [responsesWeatherTool()],
    input: singleToolPrompt,
    max_output_tokens: 128,
  };
  const tool1 = await captureScenarioRequest(config, model, downstream, "tool_call", path, toolReq);
  const tool1Json = parseResponsesResponse(tool1.bodyText);
  const functionCalls = getResponsesFunctionCalls(tool1Json);

  let tool2Status = 0;
  let tool2Assessment: TextExpectationAssessment | null = null;
  if (functionCalls.length > 0) {
    const tool2Req: RequestObject = {
      model,
      input: [
        responsesUserInputMessage(singleToolPrompt),
        ...functionCalls.map((call) => ({ type: "function_call", call_id: call.call_id, name: call.name, arguments: call.arguments })),
        ...functionCalls.map((call) => ({ type: "function_call_output", call_id: call.call_id, output: WEATHER_RESULT_SENTINEL })),
      ],
      max_output_tokens: 128,
    };
    const tool2 = await captureScenarioRequest(config, model, downstream, "tool_result_roundtrip", path, tool2Req);
    tool2Status = tool2.status;
    const finalText = getResponsesOutputText(parseResponsesResponse(tool2.bodyText));
    tool2Assessment = assessExpectedAnswerText(finalText, SINGLE_TOOL_EXPECTED_ANSWER, SINGLE_TOOL_REQUIRED_SUBSTRINGS);
    await writeAnalysisArtifact(config.outDir, model, downstream, "tool_result_roundtrip", {
      prompt: singleToolPrompt,
      expected_answer: SINGLE_TOOL_EXPECTED_ANSWER,
      required_substrings: SINGLE_TOOL_REQUIRED_SUBSTRINGS,
      tool_results: [WEATHER_RESULT_SENTINEL],
      final_text: finalText,
      evaluation_mode: tool2Assessment.mode,
      required_substring_hits: tool2Assessment.requiredSubstringHits,
      normalized_text: tool2Assessment.normalizedText,
      preview: tool2Assessment.preview,
    });
  }

  const parallelReq: RequestObject = {
    model,
    tool_choice: "required",
    parallel_tool_calls: true,
    tools: [responsesWeatherTool(), responsesWebsearchTool()],
    input: parallelToolPrompt,
    max_output_tokens: 256,
  };
  const parallel1 = await captureScenarioRequest(config, model, downstream, "parallel_tool_call", path, parallelReq);
  const parallel1Json = parseResponsesResponse(parallel1.bodyText);
  const parallelCalls = getResponsesFunctionCalls(parallel1Json);

  let parallel2Status = 0;
  let parallel2Assessment: TextExpectationAssessment | null = null;
  if (parallelCalls.length > 0) {
    const parallel2Req: RequestObject = {
      model,
      input: [
        responsesUserInputMessage(parallelToolPrompt),
        ...parallelCalls.map((call) => ({ type: "function_call", call_id: call.call_id, name: call.name, arguments: call.arguments })),
        ...parallelCalls.map((call) => ({
          type: "function_call_output",
          call_id: call.call_id,
          output: call.name === "weather" ? WEATHER_RESULT_SENTINEL : WEBSEARCH_RESULT_SENTINEL,
        })),
      ],
      max_output_tokens: 128,
    };
    const parallel2 = await captureScenarioRequest(config, model, downstream, "parallel_tool_roundtrip", path, parallel2Req);
    parallel2Status = parallel2.status;
    const finalText = getResponsesOutputText(parseResponsesResponse(parallel2.bodyText));
    parallel2Assessment = assessExpectedAnswerText(finalText, PARALLEL_TOOL_EXPECTED_ANSWER, PARALLEL_TOOL_REQUIRED_SUBSTRINGS);
    await writeAnalysisArtifact(config.outDir, model, downstream, "parallel_tool_roundtrip", {
      prompt: parallelToolPrompt,
      expected_answer: PARALLEL_TOOL_EXPECTED_ANSWER,
      required_substrings: PARALLEL_TOOL_REQUIRED_SUBSTRINGS,
      tool_results: [WEATHER_RESULT_SENTINEL, WEBSEARCH_RESULT_SENTINEL],
      final_text: finalText,
      evaluation_mode: parallel2Assessment.mode,
      required_substring_hits: parallel2Assessment.requiredSubstringHits,
      normalized_text: parallel2Assessment.normalizedText,
      preview: parallel2Assessment.preview,
    });
  }

  const toolStreamReq: RequestObject = {
    ...parallelReq,
    stream: true,
  };
  const toolStream = await captureScenarioStream(config, model, downstream, "tool_stream", path, toolStreamReq);

  return {
    downstream,
    model,
    basic: scenarioResult(basic.status, basic.status === 200 && getResponsesOutputText(basicJson) === "responses-basic-ok"),
    stream: scenarioResult(
      stream.status,
      stream.status === 200 && stream.doneSeen && responsesStreamHasEvent(stream.frames, ["response.completed", "done"]),
    ),
    reasoning: {
      status: reasoning.status,
      ok: reasoning.status === 200 && getResponsesOutputText(reasoningJson) === "responses-reasoning-ok",
      observed: responsesReasoningPresent(reasoningJson),
    },
    encrypted_reasoning: observedResult(
      reasoningStream.status,
      reasoningStream.frames.some((frame) => frame.data.includes("reasoning_signature") || frame.data.includes("reasoning.encrypted") || frame.data.includes("signature")),
    ),
    reasoning_stream: scenarioResult(
      reasoningStream.status,
      reasoningStream.status === 200 && normalizeObservedText(reasoningStreamText) === REASONING_STREAM_EXPECTED_ANSWER,
      `preview=${JSON.stringify(previewText(normalizeObservedText(reasoningStreamText), 120))}`,
    ),
    multimodal: scenarioResult(multimodal.status, multimodal.status === 200 && getResponsesOutputText(multimodalJson).length > 0),
    tool_call: scenarioResult(tool1.status, tool1.status === 200 && functionCalls.length >= 1, undefined, functionCalls.length),
    tool_result_roundtrip: scenarioResultFromAssessment(tool2Status, tool2Assessment),
    parallel_tool_call: scenarioResult(parallel1.status, parallel1.status === 200 && parallelCalls.length >= 2, undefined, parallelCalls.length),
    parallel_tool_roundtrip: scenarioResultFromAssessment(parallel2Status, parallel2Assessment),
    tool_stream: observedResult(toolStream.status, toolStream.status === 200 && responsesStreamHasToolCalls(toolStream.frames)),
  };
}

async function runMessagesSuite(config: Config, model: string): Promise<SuiteResult> {
  const downstream: Downstream = "messages";
  const path = "/messages";
  const headers = { "anthropic-version": config.anthropicVersion };
  const singleToolPrompt = buildSingleToolPrompt();
  const parallelToolPrompt = buildParallelToolPrompt();
  const reasoningStreamPrompt = buildReasoningStreamPrompt();

  const basicReq: RequestObject = {
    model,
    max_tokens: 256,
    messages: [{ role: "user", content: "Reply with exactly: messages-basic-ok" }],
  };
  const basic = await captureScenarioRequest(config, model, downstream, "basic", path, basicReq, headers);
  const basicJson = parseMessagesResponse(basic.bodyText);

  const streamReq: RequestObject = {
    model,
    max_tokens: 256,
    stream: true,
    messages: [{ role: "user", content: "Reply with exactly: messages-stream-ok" }],
  };
  const stream = await captureScenarioStream(config, model, downstream, "stream", path, streamReq, headers);

  const reasoningReq: RequestObject = {
    model,
    max_tokens: 256,
    thinking: { type: "enabled", budget_tokens: 1024 },
    messages: [{ role: "user", content: "Think briefly and reply with exactly: messages-reasoning-ok" }],
  };
  const reasoning = await captureScenarioRequest(config, model, downstream, "reasoning", path, reasoningReq, headers);
  const reasoningJson = parseMessagesResponse(reasoning.bodyText);

  const reasoningStreamReq: RequestObject = {
    model,
    max_tokens: 256,
    stream: true,
    thinking: { type: "enabled", budget_tokens: 4096 },
    messages: [{ role: "user", content: reasoningStreamPrompt }],
  };
  const reasoningStream = await captureScenarioStream(config, model, downstream, "reasoning_stream", path, reasoningStreamReq, headers);
  const reasoningStreamText = getMessagesStreamOutputText(reasoningStream.frames);

  const multimodalReq: RequestObject = {
    model,
    max_tokens: 256,
    messages: [
      {
        role: "user",
        content: [
          { type: "text", text: "Describe this image in one short sentence." },
          { type: "image", source: { type: "base64", media_type: "image/png", data: PNG_BASE64 } },
        ],
      },
    ],
  };
  const multimodal = await captureScenarioRequest(config, model, downstream, "multimodal", path, multimodalReq, headers);
  const multimodalJson = parseMessagesResponse(multimodal.bodyText);

  const toolReq: RequestObject = {
    model,
    max_tokens: 256,
    tool_choice: { type: "any" },
    tools: [messagesWeatherTool()],
    messages: [{ role: "user", content: singleToolPrompt }],
  };
  const tool1 = await captureScenarioRequest(config, model, downstream, "tool_call", path, toolReq, headers);
  const tool1Json = parseMessagesResponse(tool1.bodyText);
  const toolUses = getMessagesToolUses(tool1Json);
  const toolUseRequests = toMessagesToolUseRequests(toolUses);

  let tool2Status = 0;
  let tool2Assessment: TextExpectationAssessment | null = null;
  if (toolUses.length > 0) {
    const tool2Req: RequestObject = {
      model,
      max_tokens: 256,
      messages: [
        { role: "user", content: singleToolPrompt },
        { role: "assistant", content: toolUseRequests },
        {
          role: "user",
          content: toolUses.map((use) => ({ type: "tool_result", tool_use_id: use.id, content: WEATHER_RESULT_SENTINEL })),
        },
      ],
    };
    const tool2 = await captureScenarioRequest(config, model, downstream, "tool_result_roundtrip", path, tool2Req, headers);
    tool2Status = tool2.status;
    const finalText = getMessagesText(parseMessagesResponse(tool2.bodyText));
    tool2Assessment = assessExpectedAnswerText(finalText, SINGLE_TOOL_EXPECTED_ANSWER, SINGLE_TOOL_REQUIRED_SUBSTRINGS);
    await writeAnalysisArtifact(config.outDir, model, downstream, "tool_result_roundtrip", {
      prompt: singleToolPrompt,
      expected_answer: SINGLE_TOOL_EXPECTED_ANSWER,
      required_substrings: SINGLE_TOOL_REQUIRED_SUBSTRINGS,
      tool_results: [WEATHER_RESULT_SENTINEL],
      final_text: finalText,
      evaluation_mode: tool2Assessment.mode,
      required_substring_hits: tool2Assessment.requiredSubstringHits,
      normalized_text: tool2Assessment.normalizedText,
      preview: tool2Assessment.preview,
    });
  }

  const parallelReq: RequestObject = {
    model,
    max_tokens: 512,
    tool_choice: { type: "any" },
    tools: [messagesWeatherTool(), messagesWebsearchTool()],
    messages: [
      {
        role: "user",
        content: parallelToolPrompt,
      },
    ],
  };
  const parallel1 = await captureScenarioRequest(config, model, downstream, "parallel_tool_call", path, parallelReq, headers);
  const parallel1Json = parseMessagesResponse(parallel1.bodyText);
  const parallelUses = getMessagesToolUses(parallel1Json);
  const parallelUseRequests = toMessagesToolUseRequests(parallelUses);

  let parallel2Status = 0;
  let parallel2Assessment: TextExpectationAssessment | null = null;
  if (parallelUses.length > 0) {
    const parallel2Req: RequestObject = {
      model,
      max_tokens: 256,
      messages: [
        { role: "user", content: parallelToolPrompt },
        { role: "assistant", content: parallelUseRequests },
        {
          role: "user",
          content: parallelUses.map((use) => ({
            type: "tool_result",
            tool_use_id: use.id,
            content: use.name === "weather" ? WEATHER_RESULT_SENTINEL : WEBSEARCH_RESULT_SENTINEL,
          })),
        },
      ],
    };
    const parallel2 = await captureScenarioRequest(config, model, downstream, "parallel_tool_roundtrip", path, parallel2Req, headers);
    parallel2Status = parallel2.status;
    const finalText = getMessagesText(parseMessagesResponse(parallel2.bodyText));
    parallel2Assessment = assessExpectedAnswerText(finalText, PARALLEL_TOOL_EXPECTED_ANSWER, PARALLEL_TOOL_REQUIRED_SUBSTRINGS);
    await writeAnalysisArtifact(config.outDir, model, downstream, "parallel_tool_roundtrip", {
      prompt: parallelToolPrompt,
      expected_answer: PARALLEL_TOOL_EXPECTED_ANSWER,
      required_substrings: PARALLEL_TOOL_REQUIRED_SUBSTRINGS,
      tool_results: [WEATHER_RESULT_SENTINEL, WEBSEARCH_RESULT_SENTINEL],
      final_text: finalText,
      evaluation_mode: parallel2Assessment.mode,
      required_substring_hits: parallel2Assessment.requiredSubstringHits,
      normalized_text: parallel2Assessment.normalizedText,
      preview: parallel2Assessment.preview,
    });
  }

  const toolStreamReq: RequestObject = {
    ...parallelReq,
    stream: true,
  };
  const toolStream = await captureScenarioStream(config, model, downstream, "tool_stream", path, toolStreamReq, headers);

  return {
    downstream,
    model,
    basic: scenarioResult(basic.status, basic.status === 200 && getMessagesText(basicJson) === "messages-basic-ok"),
    stream: scenarioResult(stream.status, stream.status === 200 && !stream.doneSeen && messagesStreamHasTerminal(stream.frames)),
    reasoning: {
      status: reasoning.status,
      ok: reasoning.status === 200 && getMessagesText(reasoningJson) === "messages-reasoning-ok",
      observed: messagesReasoningPresent(reasoningJson),
    },
    encrypted_reasoning: observedResult(reasoning.status, messagesEncryptedReasoningPresent(reasoningJson)),
    reasoning_stream: scenarioResult(
      reasoningStream.status,
      reasoningStream.status === 200 && normalizeObservedText(reasoningStreamText) === REASONING_STREAM_EXPECTED_ANSWER,
      `preview=${JSON.stringify(previewText(normalizeObservedText(reasoningStreamText), 120))}`,
    ),
    multimodal: scenarioResult(multimodal.status, multimodal.status === 200 && getMessagesText(multimodalJson).length > 0),
    tool_call: scenarioResult(tool1.status, tool1.status === 200 && toolUses.length >= 1, undefined, toolUses.length),
    tool_result_roundtrip: scenarioResultFromAssessment(tool2Status, tool2Assessment),
    parallel_tool_call: scenarioResult(parallel1.status, parallel1.status === 200 && parallelUses.length >= 2, undefined, parallelUses.length),
    parallel_tool_roundtrip: scenarioResultFromAssessment(parallel2Status, parallel2Assessment),
    tool_stream: observedResult(toolStream.status, toolStream.status === 200 && messagesStreamHasToolUse(toolStream.frames)),
  };
}

async function runModelMatrix(config: Config, label: string, model: string): Promise<ModelMatrixResult> {
  logSection(`${label} (${model})`);
  const chat = await runChatSuite(config, model);
  logScenario("chat.basic", chat.basic);
  logScenario("chat.stream", chat.stream);
  logScenario("chat.reasoning", chat.reasoning);
  logScenario("chat.reasoning_stream", chat.reasoning_stream);
  logScenario("chat.multimodal", chat.multimodal);
  logScenario("chat.tool_call", chat.tool_call);
  logScenario("chat.tool_result_roundtrip", chat.tool_result_roundtrip);
  logScenario("chat.parallel_tool_call", chat.parallel_tool_call);
  logScenario("chat.parallel_tool_roundtrip", chat.parallel_tool_roundtrip);
  logScenario("chat.tool_stream", chat.tool_stream);

  const responses = await runResponsesSuite(config, model);
  logScenario("responses.basic", responses.basic);
  logScenario("responses.stream", responses.stream);
  logScenario("responses.reasoning", responses.reasoning);
  logScenario("responses.reasoning_stream", responses.reasoning_stream);
  logScenario("responses.multimodal", responses.multimodal);
  logScenario("responses.tool_call", responses.tool_call);
  logScenario("responses.tool_result_roundtrip", responses.tool_result_roundtrip);
  logScenario("responses.parallel_tool_call", responses.parallel_tool_call);
  logScenario("responses.parallel_tool_roundtrip", responses.parallel_tool_roundtrip);
  logScenario("responses.tool_stream", responses.tool_stream);

  const messages = await runMessagesSuite(config, model);
  logScenario("messages.basic", messages.basic);
  logScenario("messages.stream", messages.stream);
  logScenario("messages.reasoning", messages.reasoning);
  logScenario("messages.reasoning_stream", messages.reasoning_stream);
  logScenario("messages.multimodal", messages.multimodal);
  logScenario("messages.tool_call", messages.tool_call);
  logScenario("messages.tool_result_roundtrip", messages.tool_result_roundtrip);
  logScenario("messages.parallel_tool_call", messages.parallel_tool_call);
  logScenario("messages.parallel_tool_roundtrip", messages.parallel_tool_roundtrip);
  logScenario("messages.tool_stream", messages.tool_stream);

  return {
    label,
    model,
    downstreams: { chat, responses, messages },
  };
}

function allCriticalPassed(models: ModelMatrixResult[]): boolean {
  return models.every((model) => {
    const suites = [model.downstreams.chat, model.downstreams.responses, model.downstreams.messages];
    return suites.every((suite) => getCriticalScenarios(suite).every((result) => result.ok));
  });
}

async function main(): Promise<void> {
  if (process.argv.includes("--help") || process.argv.includes("-h")) {
    printHelp();
    return;
  }

  const config = getConfig();
  await ensureOutDir(config.outDir);

  console.log(colorize("cyan", `Writing captures to ${config.outDir}`));
  console.log(colorize("gray", `Base URL: ${config.baseUrl}`));

  const gpt = await runModelMatrix(config, "gpt", config.gptModel);
  const claude = await runModelMatrix(config, "claude", config.claudeModel);
  const models = [gpt, claude];

  const summary: SummaryResult = {
    base_url: config.baseUrl,
    out_dir: config.outDir,
    generated_at: new Date().toISOString(),
    models,
    all_critical_passed: allCriticalPassed(models),
  };

  await Bun.write(join(config.outDir, "summary.json"), stringifyJson(summary));

  console.log(`\n${summary.all_critical_passed ? colorize("green", "All critical scenarios passed.") : colorize("red", "One or more critical scenarios failed.")}`);
  if (!summary.all_critical_passed) {
    process.exitCode = 1;
  }
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.stack ?? error.message : String(error);
  console.error(colorize("red", message));
  process.exitCode = 1;
});
