import {
  convertToModelMessages,
  streamText,
  toUIMessageStream,
  type ChatTransport,
  type UIMessage,
  type UIMessageChunk,
} from "ai";
import { createOpenAI } from "@ai-sdk/openai";
import { withRawReasoningRewrite } from "@/components/playground/responses-sse";

export type ChatRequestAuth =
  | { mode: "internal"; group: string }
  | { mode: "api-key"; apiKey: string }
  | { mode: "invalid"; message: string };

export interface ChatRequestConfig {
  model: string;
  auth: ChatRequestAuth;
  systemPrompt: string;
  temperature: string;
  maxTokens: string;
}

/**
 * Strips assistant `file` and `reasoning` parts before model conversion
 * (PG-CHAT3): generated images cannot be replayed as assistant content, and
 * reasoning is never replayed because Monoize may route each request to a
 * different upstream. When stripping empties a message, a literal "[image]"
 * text part is substituted if a file part was removed (the message showed an
 * image); otherwise the message is dropped from the outgoing conversation.
 */
export function sanitizeForModel(messages: UIMessage[]): UIMessage[] {
  const result: UIMessage[] = [];
  for (const message of messages) {
    if (message.role !== "assistant") {
      result.push(message);
      continue;
    }
    const parts = message.parts.filter(
      (part) => part.type !== "file" && part.type !== "reasoning",
    );
    if (parts.length === 0) {
      const hadFile = message.parts.some((part) => part.type === "file");
      if (hadFile) {
        result.push({
          ...message,
          parts: [{ type: "text" as const, text: "[image]" }],
        });
      }
      continue;
    }
    result.push(
      parts.length === message.parts.length ? message : { ...message, parts },
    );
  }
  return result;
}

/**
 * Maps a stream failure to human-readable text (PG-CHAT2 step 6). Provider
 * error chunks arrive as plain objects (not Error instances), so the `message`
 * field is extracted before falling back to JSON serialization.
 */
export function describeStreamError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const record = error as { message?: unknown; error?: { message?: unknown } };
    if (typeof record.message === "string") return record.message;
    if (record.error && typeof record.error.message === "string") {
      return record.error.message;
    }
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error);
}

function parseFinite(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function parsePositiveInt(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

/**
 * ChatTransport that runs the AI SDK OpenAI Responses model in the browser
 * against the local Monoize proxy (`POST /api/v1/responses`), bypassing any
 * UI-message server protocol (PG-CHAT1/PG-CHAT2). Config is read at call time
 * so selector changes apply to regenerations too. The provider fetch is
 * wrapped with the PG-CHAT7 raw-reasoning SSE adapter so open-source CoT
 * (`response.reasoning_text.*`) surfaces as reasoning UI parts.
 */
export class MonoizeChatTransport implements ChatTransport<UIMessage> {
  private config: ChatRequestConfig;
  private missingModelMessage: string;

  constructor(config: ChatRequestConfig, missingModelMessage: string) {
    this.config = config;
    this.missingModelMessage = missingModelMessage;
  }

  updateConfig(config: ChatRequestConfig, missingModelMessage: string) {
    this.config = config;
    this.missingModelMessage = missingModelMessage;
  }

  async sendMessages(
    options: Parameters<ChatTransport<UIMessage>["sendMessages"]>[0],
  ): Promise<ReadableStream<UIMessageChunk>> {
    const config = this.config;
    if (!config.model.trim()) {
      throw new Error(this.missingModelMessage);
    }
    if (config.auth.mode === "invalid") {
      throw new Error(config.auth.message);
    }

    const internal = config.auth.mode === "internal";
    const rewriteFetch = withRawReasoningRewrite(globalThis.fetch.bind(globalThis));
    const provider = createOpenAI({
      name: "monoize",
      baseURL: `${window.location.origin}/api/v1`,
      ...(internal
        ? {
            headers: {
              "x-monoize-internal-source": "playground",
              ...(config.auth.group.trim()
                ? { "x-monoize-playground-group": config.auth.group.trim() }
                : {}),
            },
            fetch: (input: RequestInfo | URL, init?: RequestInit) =>
              rewriteFetch(input, { ...init, credentials: "include" }),
          }
        : {
            apiKey: config.auth.apiKey,
            fetch: rewriteFetch,
          }),
    });

    const systemPrompt = config.systemPrompt.trim();
    const result = streamText({
      model: provider.responses(config.model.trim()),
      messages: await convertToModelMessages(sanitizeForModel(options.messages)),
      ...(systemPrompt ? { system: systemPrompt } : {}),
      ...(parseFinite(config.temperature) !== undefined
        ? { temperature: parseFinite(config.temperature) }
        : {}),
      ...(parsePositiveInt(config.maxTokens) !== undefined
        ? { maxOutputTokens: parsePositiveInt(config.maxTokens) }
        : {}),
      abortSignal: options.abortSignal,
      providerOptions: {
        openai: {
          // Reasoning summaries are opt-in on the Responses API; "auto" makes
          // reasoning-capable models return them without a user toggle
          // (PG-CHAT2a). The playground is ephemeral, so responses are never
          // stored server-side.
          reasoningSummary: "auto",
          store: false,
        },
      },
    });

    return toUIMessageStream({
      stream: result.fullStream,
      // Default onError masks messages; the playground surfaces upstream error text.
      onError: describeStreamError,
    });
  }

  async reconnectToStream(): Promise<ReadableStream<UIMessageChunk> | null> {
    return null;
  }
}
