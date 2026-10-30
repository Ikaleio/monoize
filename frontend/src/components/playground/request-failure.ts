/** One failed chat or image request, as rendered per playground.spec.md PG-CHAT2b. */
export interface PlaygroundRequestFailure {
  /** Upstream error text; empty when the response carried no usable detail. */
  message: string;
  status: number | null;
  requestId: string | null;
}

/** Response header Monoize sets on every response. */
export const REQUEST_ID_HEADER = "x-request-id";

/** Removes the AI SDK retry wrapper and error-class prefix from a message. */
export function cleanErrorMessage(message: string): string {
  return message
    .replace(/^Failed after \d+ attempts?\.\s*Last error:\s*/i, "")
    .replace(/^AI_\w+:\s*/, "")
    .trim();
}
