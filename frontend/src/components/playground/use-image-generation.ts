import { useCallback, useRef, useState } from "react";
import type { FileUIPart, UIMessage } from "ai";
import { normalizePlaygroundImageSize } from "./image-size";
import { normalizePlaygroundImageQuality } from "./image-quality";
import { loadPlaygroundImage } from "./image-source";

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
  quality: string;
  group: string;
  apiKey: string | null;
  attachments: (ComposerAttachment | FileUIPart)[];
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
  const quality = normalizePlaygroundImageQuality(input.quality);
  return JSON.stringify({
    model: input.model,
    prompt: input.prompt,
    n: 1,
    stream: true,
    ...(size ? { size } : {}),
    ...(quality !== "default" ? { quality } : {}),
  });
}

export function buildImageEditForm(
  input: Omit<ImageRequestInput, "attachments"> & { attachments: ComposerAttachment[] },
): FormData {
  const form = new FormData();
  form.set("model", input.model);
  form.set("prompt", input.prompt);
  form.set("n", "1");
  form.set("stream", "true");
  for (const attachment of input.attachments) {
    form.append("image", attachment.file, attachment.file.name || "reference.png");
  }
  const size = normalizePlaygroundImageSize(input.size);
  if (size) form.set("size", size);
  const quality = normalizePlaygroundImageQuality(input.quality);
  if (quality !== "default") form.set("quality", quality);
  return form;
}

async function readImageStream(response: Response): Promise<ImageApiDataItem[]> {
  if (!response.body || !response.headers.get("content-type")?.includes("text/event-stream")) {
    throw new Error("expected an image event stream");
  }
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  const items: ImageApiDataItem[] = [];
  let buffer = "";
  let completed = false;

  const readFrame = (frame: string) => {
    const data = frame
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).replace(/^ /, ""))
      .join("\n");
    if (!data) return;
    if (data === "[DONE]") {
      completed = true;
      return;
    }
    const event = JSON.parse(data) as ImageApiDataItem & {
      type?: string;
      error?: { message?: string; code?: string };
    };
    if (event.type === "error" || event.error) {
      throw new Error(event.error?.message || event.error?.code || "image generation failed");
    }
    if (event.type === "image_generation.completed" || event.type === "image_edit.completed") {
      if (!event.b64_json && !event.url) {
        throw new Error("image stream completed without image data");
      }
      items.push({
        b64_json: event.b64_json,
        url: event.url,
        revised_prompt: event.revised_prompt,
      });
    }
  };

  try {
    while (!completed) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += value;
      let boundary: RegExpExecArray | null;
      while (!completed && (boundary = /\r?\n\r?\n/.exec(buffer))) {
        const frame = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary[0].length);
        readFrame(frame);
      }
    }
    if (!completed) throw new Error("image stream ended before completion");
    if (items.length === 0) throw new Error("empty image response");
    return items;
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function requestImages(
  input: ImageRequestInput,
  signal: AbortSignal,
): Promise<ImageApiDataItem[]> {
  const authHeaders: Record<string, string> = input.apiKey
    ? { Authorization: `Bearer ${input.apiKey}` }
    : { "x-monoize-internal-source": "playground" };
  if (!input.apiKey && input.group.trim()) {
    authHeaders["x-monoize-playground-group"] = input.group.trim();
  }
  const internalCredentials = input.apiKey
    ? {}
    : ({ credentials: "include" } as const);
  let response: Response;
  if (input.attachments.length > 0) {
    const attachments = await Promise.all(
      input.attachments.map(async (attachment): Promise<ComposerAttachment> => {
        if ("file" in attachment) return attachment;
        const source = await loadPlaygroundImage(attachment.url, signal);
        return {
          id: playgroundMessageId(),
          file: new File(
            [source],
            attachment.filename || "reference.png",
            { type: attachment.mediaType },
          ),
          url: attachment.url,
        };
      }),
    );
    const form = buildImageEditForm({ ...input, attachments });
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

  return readImageStream(response);
}

function buildAssistantImageMessage(items: ImageApiDataItem[]): UIMessage {
  const revised = items
    .map((item) => item.revised_prompt)
    .filter((text): text is string => Boolean(text && text.trim()));
  return {
    id: playgroundMessageId(),
    role: "assistant",
    metadata: { playgroundImage: true },
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
          ...input.attachments.map((attachment): FileUIPart =>
            "file" in attachment
              ? {
                  type: "file",
                  mediaType: attachment.file.type || "image/png",
                  filename: attachment.file.name,
                  url: attachment.url,
                }
              : attachment,
          ),
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

  const rerun = useCallback(
    (input: ImageRequestInput) => {
      void run({ id: playgroundMessageId(), status: "pending", input });
    },
    [run],
  );

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

  return { job, generate, retry, regenerate, rerun, abort, clear, reset };
}
