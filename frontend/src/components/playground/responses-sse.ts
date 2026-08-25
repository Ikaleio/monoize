/**
 * Raw-reasoning SSE adapter for the Monoize Responses stream (PG-CHAT7/PG-CHAT8).
 *
 * `@ai-sdk/openai@4` parses the reasoning-summary event family but drops the
 * official raw-reasoning events `response.reasoning_text.delta` / `.done` that
 * Monoize emits for `Reasoning.content` (open-source CoT models). This adapter
 * rewrites only those two event types into summary-family events whose
 * `summary_index` is offset by a large base, so raw reasoning surfaces as
 * distinct reasoning UI parts while every other frame passes through
 * byte-identical.
 */

/**
 * Summary indexes at or above this base mark rewritten raw-reasoning parts.
 * Real OpenAI summaries use small indexes (single digits), so no collision.
 */
export const RAW_REASONING_SUMMARY_INDEX_BASE = 1000;

export type ReasoningPartKind = "content" | "summary";

/**
 * Classifies a reasoning UI part by its AI SDK id (`<item_id>:<summary_index>`).
 * Parts rewritten by this adapter carry `summary_index >= 1000` and classify as
 * raw reasoning `content`; everything else (including missing ids) is `summary`.
 */
export function reasoningPartKind(partId: string | undefined): ReasoningPartKind {
  if (!partId) return "summary";
  const separator = partId.lastIndexOf(":");
  if (separator < 0) return "summary";
  const index = Number(partId.slice(separator + 1));
  return Number.isInteger(index) && index >= RAW_REASONING_SUMMARY_INDEX_BASE
    ? "content"
    : "summary";
}

interface RawReasoningEvent {
  type: string;
  item_id?: unknown;
  output_index?: unknown;
  content_index?: unknown;
  delta?: unknown;
  [key: string]: unknown;
}

function parseFrameData(frame: string): RawReasoningEvent | null {
  // Monoize emits exactly one single-line `data:` field per frame
  // (long payloads are split into multiple frames, never multi-line data).
  const dataLine = frame
    .split("\n")
    .find((line) => line.startsWith("data:"));
  if (!dataLine) return null;
  try {
    const parsed: unknown = JSON.parse(dataLine.slice(5).trim());
    if (
      parsed &&
      typeof parsed === "object" &&
      !Array.isArray(parsed) &&
      typeof (parsed as { type?: unknown }).type === "string"
    ) {
      return parsed as RawReasoningEvent;
    }
  } catch {
    /* non-JSON data (e.g. [DONE]) passes through */
  }
  return null;
}

function serializeFrame(data: Record<string, unknown>): string {
  return `event: ${String(data.type)}\ndata: ${JSON.stringify(data)}\n\n`;
}

/**
 * Rewrites one SSE frame per PG-CHAT7. Returns the replacement text (possibly
 * with an injected `response.reasoning_summary_part.added` frame), or `null`
 * when the frame must pass through unchanged. `startedParts` tracks
 * (`item_id`, `content_index`) pairs that already received their synthetic
 * part-added frame.
 */
export function rewriteRawReasoningFrame(
  frame: string,
  startedParts: Set<string>,
): string | null {
  const data = parseFrameData(frame);
  if (
    !data ||
    (data.type !== "response.reasoning_text.delta" &&
      data.type !== "response.reasoning_text.done")
  ) {
    return null;
  }

  const contentIndex =
    typeof data.content_index === "number" ? data.content_index : 0;
  const summaryIndex = RAW_REASONING_SUMMARY_INDEX_BASE + contentIndex;
  const partKey = `${String(data.item_id)}#${contentIndex}`;

  let output = "";
  if (!startedParts.has(partKey)) {
    startedParts.add(partKey);
    output += serializeFrame({
      type: "response.reasoning_summary_part.added",
      item_id: data.item_id,
      output_index: data.output_index,
      summary_index: summaryIndex,
    });
  }

  const rest: Record<string, unknown> = { ...data };
  delete rest.content_index;
  output += serializeFrame({
    ...rest,
    type:
      data.type === "response.reasoning_text.delta"
        ? "response.reasoning_summary_text.delta"
        : "response.reasoning_summary_part.done",
    summary_index: summaryIndex,
  });
  return output;
}

/**
 * Splits decoded SSE text into blank-line-delimited frames, applies the
 * raw-reasoning rewrite to each complete frame, and forwards everything else
 * unchanged. Trailing bytes without a terminator flush unmodified on close.
 */
export function createRawReasoningRewriteTransform(): TransformStream<string, string> {
  let buffer = "";
  const startedParts = new Set<string>();

  const processFrame = (frame: string): string =>
    rewriteRawReasoningFrame(frame, startedParts) ?? `${frame}\n\n`;

  return new TransformStream<string, string>({
    transform(chunk, controller) {
      buffer += chunk;
      let boundary = buffer.indexOf("\n\n");
      while (boundary >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        controller.enqueue(processFrame(frame));
        boundary = buffer.indexOf("\n\n");
      }
    },
    flush(controller) {
      if (buffer.length > 0) controller.enqueue(buffer);
    },
  });
}

/**
 * Wraps `fetch` so `text/event-stream` response bodies flow through the
 * raw-reasoning rewrite; all other responses are returned untouched.
 */
export function withRawReasoningRewrite(baseFetch: typeof fetch): typeof fetch {
  return async (input, init) => {
    const response = await baseFetch(input, init);
    const contentType = response.headers.get("content-type") ?? "";
    if (!contentType.includes("text/event-stream") || !response.body) {
      return response;
    }
    const body = response.body
      .pipeThrough(new TextDecoderStream())
      .pipeThrough(createRawReasoningRewriteTransform())
      .pipeThrough(new TextEncoderStream());
    return new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers,
    });
  };
}
