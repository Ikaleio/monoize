import { describe, expect, test } from "bun:test";
import type { UIMessage } from "ai";
import { filePartsForEditedUserMessage } from "../src/components/playground/message-operations";
import {
  buildImageEditForm,
  buildImageGenerationBody,
  requestImages,
  type ComposerAttachment,
  type ImageRequestInput,
} from "../src/components/playground/use-image-generation";

function generationInput(size: string): ImageRequestInput {
  return {
    prompt: "draw a lighthouse",
    model: "gpt-image-2",
    size,
    group: "",
    apiKey: null,
    attachment: null,
  };
}

describe("Playground image size (PG-SEL6, PG-IMG2, PG-IMG3)", () => {
  test("omits size for auto and unsupported values", () => {
    expect(JSON.parse(buildImageGenerationBody(generationInput("")))).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
    });
    expect(
      JSON.parse(buildImageGenerationBody(generationInput("4096x4096"))),
    ).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
    });
  });

  test("adds an explicit size to generation JSON", () => {
    expect(
      JSON.parse(buildImageGenerationBody(generationInput("1536x1024"))),
    ).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
      size: "1536x1024",
    });
  });

  test("adds an explicit size to the edit form", () => {
    const attachment: ComposerAttachment = {
      id: "source",
      file: new File(["image"], "source.png", { type: "image/png" }),
      url: "data:image/png;base64,aW1hZ2U=",
    };
    const form = buildImageEditForm({
      ...generationInput("1024x1536"),
      attachment,
    });

    expect(form.get("model")).toBe("gpt-image-2");
    expect(form.get("prompt")).toBe("draw a lighthouse");
    expect(form.get("n")).toBe("1");
    expect(form.get("size")).toBe("1024x1536");
    expect(form.get("image")).toBeInstanceOf(File);
  });

  test("reissuing a generation input stays on the image endpoint", async () => {
    const requestedUrls: string[] = [];
    const originalFetch = globalThis.fetch;
    globalThis.fetch = (async (input: string | URL | Request) => {
      requestedUrls.push(String(input));
      return new Response(
        JSON.stringify({ data: [{ b64_json: "aW1hZ2U=" }] }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    }) as typeof fetch;

    try {
      const input = generationInput("1024x1024");
      await requestImages(input, new AbortController().signal);
      await requestImages(input, new AbortController().signal);
    } finally {
      globalThis.fetch = originalFetch;
    }

    expect(requestedUrls).toEqual([
      "/api/v1/images/generations",
      "/api/v1/images/generations",
    ]);
  });
});

describe("Playground user-message editing (PG-MSG3)", () => {
  test("preserves image file parts when the text is edited", () => {
    const imagePart = {
      type: "file" as const,
      mediaType: "image/png",
      filename: "source.png",
      url: "data:image/png;base64,aW1hZ2U=",
    };
    const messages: UIMessage[] = [
      {
        id: "user-image",
        role: "user",
        parts: [imagePart, { type: "text", text: "old prompt" }],
      },
    ];

    const files = filePartsForEditedUserMessage(messages, "user-image");
    expect(files).toEqual([imagePart]);
    expect(files[0]).not.toBe(imagePart);
  });
});
