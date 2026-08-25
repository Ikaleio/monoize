import {
  convertToModelMessages,
  streamText,
  toUIMessageStream,
  type ChatTransport,
  type UIMessage,
  type UIMessageChunk,
} from "ai";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";

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
 * Strips assistant `file` parts before model conversion (PG-CHAT3): generated
 * images cannot be replayed as assistant content on chat-completions upstreams,
 * and an empty assistant message would be rejected, so a literal "[image]"
 * placeholder is substituted when stripping empties the message.
 */
export function sanitizeForModel(messages: UIMessage[]): UIMessage[] {
  return messages.map((message) => {
    if (message.role !== "assistant") return message;
    const parts = message.parts.filter((part) => part.type !== "file");
    if (parts.length === 0) {
      return { ...message, parts: [{ type: "text" as const, text: "[image]" }] };
    }
    if (parts.length === message.parts.length) return message;
    return { ...message, parts };
  });
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
 * ChatTransport that runs the AI SDK OpenAI-compatible provider in the browser
 * against the local Monoize proxy (`POST /api/v1/chat/completions`), bypassing
 * any UI-message server protocol (PG-CHAT1/PG-CHAT2). Config is read at call
 * time so selector changes apply to regenerations too.
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
    const provider = createOpenAICompatible({
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
              fetch(input, { ...init, credentials: "include" }),
          }
        : { apiKey: config.auth.apiKey }),
    });

    const systemPrompt = config.systemPrompt.trim();
    const result = streamText({
      model: provider.chatModel(config.model.trim()),
      messages: await convertToModelMessages(sanitizeForModel(options.messages)),
      ...(systemPrompt ? { system: systemPrompt } : {}),
      ...(parseFinite(config.temperature) !== undefined
        ? { temperature: parseFinite(config.temperature) }
        : {}),
      ...(parsePositiveInt(config.maxTokens) !== undefined
        ? { maxOutputTokens: parsePositiveInt(config.maxTokens) }
        : {}),
      abortSignal: options.abortSignal,
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
