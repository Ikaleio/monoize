#!/usr/bin/env bun

import { createAnthropic } from "@ai-sdk/anthropic";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";
import { createOpenResponses } from "@ai-sdk/open-responses";
import { generateText, stepCountIs, streamText, tool } from "ai";
import { z } from "zod";

type ModelLike = Parameters<typeof generateText>[0]["model"];

type ProtocolCase = {
  protocol: "chat" | "responses" | "messages";
  model: ModelLike;
};

type CheckResult = {
  name: string;
  ok: boolean;
  detail?: string;
};

type StreamSummary = {
  text: string;
  textDeltaCount: number;
  finishCount: number;
  toolCallCount: number;
  toolResultCount: number;
  toolResultOutputs: unknown[];
  rawFunctionCallItemCount: number;
  partTypes: string[];
};

type FullStreamLike = {
  fullStream: AsyncIterable<Record<string, unknown>>;
};

const MAX_OUTPUT_TOKENS = 512;
const REQUEST_TIMEOUT_MS = 120_000;

function prepareLookupWeatherStep({ stepNumber }: { stepNumber: number }) {
  return {
    toolChoice:
      stepNumber === 0
        ? ({ type: "tool", toolName: "lookupWeather" } as const)
        : ("auto" as const),
  };
}

function printUsage(): void {
  console.log("Usage: bun run live-protocol-suite.ts <baseURL> <apiKey> <model>");
  console.log("Example: bun run live-protocol-suite.ts https://mono.example/v1 sk-... gpt-5.5");
}

function parseArgs(argv: string[]): { baseURL: string; apiKey: string; model: string } {
  const args = argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    printUsage();
    process.exit(0);
  }

  if (args.length !== 3) {
    printUsage();
    process.exit(1);
  }

  const [baseURL, apiKey, model] = args;
  if (!baseURL || !apiKey || !model) {
    printUsage();
    process.exit(1);
  }

  return { baseURL, apiKey, model };
}

function normalizeApiBase(input: string): string {
  const url = new URL(input);
  let pathname = url.pathname.replace(/\/+$/, "");

  for (const suffix of ["/responses", "/chat/completions", "/messages"]) {
    if (pathname.endsWith(suffix)) {
      pathname = pathname.slice(0, -suffix.length);
      break;
    }
  }

  if (pathname === "") {
    pathname = "/v1";
  } else if (!pathname.endsWith("/v1")) {
    pathname = `${pathname}/v1`;
  }

  url.pathname = pathname;
  url.search = "";
  url.hash = "";

  return url.toString().replace(/\/$/, "");
}

function responsesEndpoint(apiBase: string): string {
  return `${apiBase}/responses`;
}

function hasOutputArray(body: unknown): boolean {
  return Boolean(
    body &&
      typeof body === "object" &&
      Array.isArray((body as { output?: unknown }).output),
  );
}

function bodyOutputContains(body: unknown, type: string): boolean {
  if (!hasOutputArray(body)) {
    return false;
  }

  return ((body as { output: Array<{ type?: unknown }> }).output ?? []).some(
    item => item?.type === type,
  );
}

function anyStepResponseOutputContains(result: unknown, type: string): boolean {
  const steps = Array.isArray((result as { steps?: unknown })?.steps)
    ? ((result as { steps: unknown[] }).steps)
    : [];

  return steps.some(step =>
    bodyOutputContains((step as { response?: { body?: unknown } })?.response?.body, type),
  );
}

function collectToolCalls(result: unknown): unknown[] {
  const steps = Array.isArray((result as { steps?: unknown })?.steps)
    ? ((result as { steps: unknown[] }).steps)
    : [];

  return steps.flatMap(step => {
    const calls = (step as { toolCalls?: unknown }).toolCalls;
    return Array.isArray(calls) ? calls : [];
  });
}

function collectToolResults(result: unknown): unknown[] {
  const steps = Array.isArray((result as { steps?: unknown })?.steps)
    ? ((result as { steps: unknown[] }).steps)
    : [];

  return steps.flatMap(step => {
    const results = (step as { toolResults?: unknown }).toolResults;
    return Array.isArray(results) ? results : [];
  });
}

function serializedValueContains(value: unknown, expected: string): boolean {
  try {
    return JSON.stringify(value).includes(expected);
  } catch {
    return false;
  }
}

async function collectFullStream(result: FullStreamLike): Promise<StreamSummary> {
  let text = "";
  let textDeltaCount = 0;
  let finishCount = 0;
  let toolCallCount = 0;
  let toolResultCount = 0;
  const toolResultOutputs: unknown[] = [];
  let rawFunctionCallItemCount = 0;
  const partTypes: string[] = [];

  for await (const part of result.fullStream) {
    const type = String(part.type ?? "unknown");
    partTypes.push(type);

    if (type === "text-delta") {
      textDeltaCount += 1;
      const delta = part.text ?? part.delta;
      if (typeof delta === "string") {
        text += delta;
      }
    } else if (type === "finish") {
      finishCount += 1;
    } else if (type === "tool-call") {
      toolCallCount += 1;
    } else if (type === "tool-result") {
      toolResultCount += 1;
      toolResultOutputs.push(part.output);
    } else if (type === "raw") {
      const rawValue = part.rawValue as { item?: { type?: unknown } } | undefined;
      if (rawValue?.item?.type === "function_call") {
        rawFunctionCallItemCount += 1;
      }
    }
  }

  return {
    text,
    textDeltaCount,
    finishCount,
    toolCallCount,
    toolResultCount,
    toolResultOutputs,
    rawFunctionCallItemCount,
    partTypes,
  };
}

function makeLookupWeatherTool(verificationCode: string) {
  return tool({
    description: "Return deterministic weather data for a city.",
    inputSchema: z.object({
      city: z.string(),
    }),
    execute: async ({ city }) =>
      `Weather for ${city}: sunny, 25 C. Verification code: ${verificationCode}.`,
  });
}

async function runBasicTextCheck(testCase: ProtocolCase): Promise<void> {
  const sentinel = `LIVE_${testCase.protocol.toUpperCase()}_BASIC_OK`;
  const result = await generateText({
    model: testCase.model,
    prompt: `Reply with the exact token ${sentinel}. Do not add code fences.`,
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    timeout: REQUEST_TIMEOUT_MS,
    experimental_include: { responseBody: true },
  });

  if (!result.text.includes(sentinel)) {
    throw new Error(`final text did not contain ${sentinel}; text=${JSON.stringify(result.text)}`);
  }

  if (testCase.protocol === "responses" && !hasOutputArray(result.response.body)) {
    throw new Error("Responses generateText did not expose a response.body.output array");
  }
}

async function runStreamingTextCheck(testCase: ProtocolCase): Promise<void> {
  const sentinel = `LIVE_${testCase.protocol.toUpperCase()}_STREAM_OK`;
  const result = streamText({
    model: testCase.model,
    prompt: `Reply with the exact token ${sentinel}. Do not add code fences.`,
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    timeout: REQUEST_TIMEOUT_MS,
  });
  const stream = await collectFullStream(result);

  if (stream.textDeltaCount < 1) {
    throw new Error(`stream produced no text-delta events; partTypes=${stream.partTypes.join(",")}`);
  }

  if (stream.finishCount !== 1) {
    throw new Error(`stream produced ${stream.finishCount} finish events; expected 1`);
  }

  if (!stream.text.includes(sentinel)) {
    throw new Error(`stream text did not contain ${sentinel}; text=${JSON.stringify(stream.text)}`);
  }
}

async function runToolLoopCheck(testCase: ProtocolCase): Promise<void> {
  const sentinel = `LIVE_${testCase.protocol.toUpperCase()}_TOOL_OK`;
  const expectedToolOutput = `Weather for Taipei: sunny, 25 C. Verification code: ${sentinel}.`;
  const result = await generateText({
    model: testCase.model,
    prompt:
      `Call lookupWeather for city Taipei. ` +
      `After the tool result is available, reply with one sentence that includes its verification code exactly.`,
    tools: {
      lookupWeather: makeLookupWeatherTool(sentinel),
    },
    prepareStep: prepareLookupWeatherStep,
    stopWhen: stepCountIs(4),
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    timeout: REQUEST_TIMEOUT_MS,
    experimental_include: { responseBody: true },
  });

  const calls = collectToolCalls(result);
  if (calls.length < 1) {
    throw new Error("result.steps did not contain a tool call");
  }

  const toolResults = collectToolResults(result);
  if (toolResults.length < 1) {
    throw new Error("result.steps did not contain a tool result");
  }

  if (!serializedValueContains(toolResults, expectedToolOutput)) {
    throw new Error("result.steps did not preserve the deterministic tool result output");
  }

  if (!result.text.includes(sentinel)) {
    throw new Error(
      `final text did not contain ${sentinel}; text=${JSON.stringify(result.text)}`,
    );
  }

  if (testCase.protocol === "responses" && !anyStepResponseOutputContains(result, "function_call")) {
    throw new Error("Responses tool loop did not expose a function_call item in any response.body.output array");
  }
}

async function runStreamingToolLoopCheck(testCase: ProtocolCase): Promise<void> {
  const sentinel = `LIVE_${testCase.protocol.toUpperCase()}_STREAM_TOOL_OK`;
  const expectedToolOutput = `Weather for Taipei: sunny, 25 C. Verification code: ${sentinel}.`;
  const result = streamText({
    model: testCase.model,
    prompt:
      `Call lookupWeather for city Taipei. ` +
      `After the tool result is available, reply with one sentence that includes its verification code exactly.`,
    tools: {
      lookupWeather: makeLookupWeatherTool(sentinel),
    },
    prepareStep: prepareLookupWeatherStep,
    stopWhen: stepCountIs(4),
    maxOutputTokens: MAX_OUTPUT_TOKENS,
    timeout: REQUEST_TIMEOUT_MS,
    includeRawChunks: testCase.protocol === "responses",
  });
  const stream = await collectFullStream(result);

  if (stream.toolCallCount < 1) {
    throw new Error(`stream produced no tool-call events; partTypes=${stream.partTypes.join(",")}`);
  }

  if (stream.toolResultCount < 1) {
    throw new Error(`stream produced no tool-result events; partTypes=${stream.partTypes.join(",")}`);
  }

  if (!stream.toolResultOutputs.some(output => serializedValueContains(output, expectedToolOutput))) {
    throw new Error("stream tool-result events did not preserve the deterministic tool output");
  }

  if (stream.finishCount !== 1) {
    throw new Error(`stream produced ${stream.finishCount} finish events; expected 1`);
  }

  if (!stream.text.includes(sentinel)) {
    throw new Error(`stream text did not contain ${sentinel}; text=${JSON.stringify(stream.text)}`);
  }

  if (testCase.protocol === "responses" && stream.rawFunctionCallItemCount < 1) {
    throw new Error("Responses streaming tool loop did not expose a raw function_call item event");
  }
}

async function runCheck(name: string, fn: () => Promise<void>): Promise<CheckResult> {
  try {
    await fn();
    console.log(`PASS ${name}`);
    return { name, ok: true };
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    console.error(`FAIL ${name}: ${detail}`);
    return { name, ok: false, detail };
  }
}

async function main(): Promise<void> {
  const { baseURL, apiKey, model } = parseArgs(Bun.argv);
  const apiBase = normalizeApiBase(baseURL);

  console.log(`Live protocol suite target: ${apiBase}`);
  console.log(`Live protocol suite model: ${model}`);

  const chatProvider = createOpenAICompatible({
    name: "monoize-live-chat",
    baseURL: apiBase,
    apiKey,
    includeUsage: true,
  });
  const responsesProvider = createOpenResponses({
    name: "monoize-live-responses",
    url: responsesEndpoint(apiBase),
    apiKey,
  });
  const messagesProvider = createAnthropic({
    name: "monoize-live-messages",
    baseURL: apiBase,
    apiKey,
  });

  const testCases: ProtocolCase[] = [
    { protocol: "chat", model: chatProvider.chatModel(model) },
    { protocol: "responses", model: responsesProvider(model) },
    { protocol: "messages", model: messagesProvider.messages(model as never) },
  ];

  const results: CheckResult[] = [];

  for (const testCase of testCases) {
    results.push(
      await runCheck(`${testCase.protocol}.basic-text`, () => runBasicTextCheck(testCase)),
    );
    results.push(
      await runCheck(`${testCase.protocol}.stream-text`, () => runStreamingTextCheck(testCase)),
    );
    results.push(
      await runCheck(`${testCase.protocol}.tool-loop`, () => runToolLoopCheck(testCase)),
    );
    results.push(
      await runCheck(`${testCase.protocol}.stream-tool-loop`, () =>
        runStreamingToolLoopCheck(testCase),
      ),
    );
  }

  const failed = results.filter(result => !result.ok);
  if (failed.length > 0) {
    console.error(`FAIL live-protocol-suite: ${failed.length}/${results.length} checks failed`);
    process.exit(1);
  }

  console.log(`PASS live-protocol-suite: ${results.length}/${results.length} checks passed`);
}

await main();
