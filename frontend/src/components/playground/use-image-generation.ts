import { useCallback, useRef, useState } from "react";
import type { UIMessage } from "ai";
import { normalizePlaygroundImageSize } from "./image-size";

export interface ComposerAttachment {
  id: string;
  file: File;
  /** Data URL used both for previews and for chat-mode file parts. */
  url: string;
}

export interface ImageRequestInput {
  prompt: string;
  model: string;
  size: string;
  group: string;
  apiKey: string | null;
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

export function buildImageGenerationBody(input: ImageRequestInput): string {
  const size = normalizePlaygroundImageSize(input.size);
  return JSON.stringify({
    model: input.model,
    prompt: input.prompt,
    n: 1,
    ...(size ? { size } : {}),
  });
}

export function buildImageEditForm(
  input: ImageRequestInput & { attachment: ComposerAttachment },
): FormData {
  const form = new FormData();
  form.set("model", input.model);
  form.set("prompt", input.prompt);
  form.set("n", "1");
  form.set("image", input.attachment.file);
  const size = normalizePlaygroundImageSize(input.size);
  if (size) form.set("size", size);
  return form;
}

export async function requestImages(
  input: ImageRequestInput,
  signal: AbortSignal,
): Promise<ImageApiDataItem[]> {
  const authHeaders = input.apiKey
    ? { Authorization: `Bearer ${input.apiKey}` }
    : {
        "x-monoize-internal-source": "playground",
        ...(input.group.trim()
          ? { "x-monoize-playground-group": input.group.trim() }
          : {}),
      };
  const internalCredentials = input.apiKey
    ? {}
    : ({ credentials: "include" } as const);
  let response: Response;
  if (input.attachment) {
    const form = buildImageEditForm({ ...input, attachment: input.attachment });
    response = await fetch("/api/v1/images/edits", {
      method: "POST",
      headers: authHeaders,
      ...internalCredentials,
      body: form,
      signal,
    });
  } else {
    response = await fetch("/api/v1/images/generations", {
      method: "POST",
      headers: {
        ...authHeaders,
        "Content-Type": "application/json",
      },
      ...internalCredentials,
      body: buildImageGenerationBody(input),
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
  const completedInputsRef = useRef(new Map<string, ImageRequestInput>());

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
        const message = buildAssistantImageMessage(items);
        completedInputsRef.current.set(message.id, jobState.input);
        appendMessage(message);
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

  const regenerate = useCallback(
    (messageId: string): boolean => {
      const input = completedInputsRef.current.get(messageId);
      if (!input) return false;
      completedInputsRef.current.delete(messageId);
      void run({ id: playgroundMessageId(), status: "pending", input });
      return true;
    },
    [run],
  );

  const abort = useCallback(() => {
    controllerRef.current?.abort();
  }, []);

  const clear = useCallback(() => {
    controllerRef.current?.abort();
    setJob(null);
  }, [setJob]);

  const reset = useCallback(() => {
    controllerRef.current?.abort();
    completedInputsRef.current.clear();
    setJob(null);
  }, [setJob]);

  return { job, generate, retry, regenerate, abort, clear, reset };
}
