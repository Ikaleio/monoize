import { useCallback, useRef, useState } from "react";
import type { UIMessage } from "ai";

export interface ComposerAttachment {
  id: string;
  file: File;
  /** Data URL used both for previews and for chat-mode file parts. */
  url: string;
}

export interface ImageRequestInput {
  prompt: string;
  model: string;
  group: string;
  attachment: ComposerAttachment | null;
}

export interface ImageJobState {
  id: string;
  status: "pending" | "error";
  error?: string;
  input: ImageRequestInput;
}

interface ImageApiDataItem {
  b64_json?: string;
  url?: string;
  revised_prompt?: string;
}

let seq = 0;
export function playgroundMessageId(): string {
  return `pg-${Date.now()}-${++seq}`;
}

async function requestImages(
  input: ImageRequestInput,
  signal: AbortSignal,
): Promise<ImageApiDataItem[]> {
  const internalHeaders = {
    "x-monoize-internal-source": "playground",
    ...(input.group.trim()
      ? { "x-monoize-playground-group": input.group.trim() }
      : {}),
  };
  let response: Response;
  if (input.attachment) {
    const form = new FormData();
    form.set("model", input.model);
    form.set("prompt", input.prompt);
    form.set("n", "1");
    form.set("image", input.attachment.file);
    response = await fetch("/api/v1/images/edits", {
      method: "POST",
      headers: internalHeaders,
      credentials: "include",
      body: form,
      signal,
    });
  } else {
    response = await fetch("/api/v1/images/generations", {
      method: "POST",
      headers: {
        ...internalHeaders,
        "Content-Type": "application/json",
      },
      credentials: "include",
      body: JSON.stringify({ model: input.model, prompt: input.prompt, n: 1 }),
      signal,
    });
  }

  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try {
      const body = await response.json();
      message = body.error?.message || body.error?.code || message;
    } catch {
      /* non-JSON error body */
    }
    throw new Error(message);
  }

  const body = (await response.json()) as { data?: ImageApiDataItem[] };
  const items = Array.isArray(body.data) ? body.data : [];
  if (items.length === 0) {
    throw new Error("empty image response");
  }
  return items;
}

function buildAssistantImageMessage(items: ImageApiDataItem[]): UIMessage {
  const revised = items
    .map((item) => item.revised_prompt)
    .filter((text): text is string => Boolean(text && text.trim()));
  return {
    id: playgroundMessageId(),
    role: "assistant",
    parts: [
      ...(revised.length > 0
        ? [{ type: "text" as const, text: revised.join("\n\n") }]
        : []),
      ...items.map((item) => ({
        type: "file" as const,
        mediaType: "image/png",
        url: item.url ?? `data:image/png;base64,${item.b64_json ?? ""}`,
      })),
    ],
  };
}

/**
 * Image generation/edit flow (playground.spec.md §7). The user message is
 * appended synchronously; the assistant result replaces an animated pending
 * placeholder rendered from `job`.
 */
export function usePlaygroundImages(appendMessage: (message: UIMessage) => void) {
  const [job, setJobState] = useState<ImageJobState | null>(null);
  const jobRef = useRef<ImageJobState | null>(null);
  const controllerRef = useRef<AbortController | null>(null);

  const setJob = useCallback((next: ImageJobState | null) => {
    jobRef.current = next;
    setJobState(next);
  }, []);

  const run = useCallback(
    async (jobState: ImageJobState) => {
      const controller = new AbortController();
      controllerRef.current = controller;
      setJob({ ...jobState, status: "pending", error: undefined });
      try {
        const items = await requestImages(jobState.input, controller.signal);
        appendMessage(buildAssistantImageMessage(items));
        setJob(null);
      } catch (error) {
        if ((error as Error).name === "AbortError") {
          // PG-IMG7: aborting removes the placeholder without an error state.
          setJob(null);
          return;
        }
        setJob({
          ...jobState,
          status: "error",
          error: (error as Error).message || "request failed",
        });
      } finally {
        controllerRef.current = null;
      }
    },
    [appendMessage, setJob],
  );

  const generate = useCallback(
    (input: ImageRequestInput) => {
      appendMessage({
        id: playgroundMessageId(),
        role: "user",
        parts: [
          ...(input.attachment
            ? [
                {
                  type: "file" as const,
                  mediaType: input.attachment.file.type || "image/png",
                  filename: input.attachment.file.name,
                  url: input.attachment.url,
                },
              ]
            : []),
          { type: "text" as const, text: input.prompt },
        ],
      });
      void run({ id: playgroundMessageId(), status: "pending", input });
    },
    [appendMessage, run],
  );

  const retry = useCallback(() => {
    const current = jobRef.current;
    if (current && current.status === "error") {
      void run(current);
    }
  }, [run]);

  const abort = useCallback(() => {
    controllerRef.current?.abort();
  }, []);

  const clear = useCallback(() => {
    controllerRef.current?.abort();
    setJob(null);
  }, [setJob]);

  return { job, generate, retry, abort, clear };
}
