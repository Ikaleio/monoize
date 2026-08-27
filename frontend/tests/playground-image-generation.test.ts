import { describe, expect, test } from "bun:test";
import type { UIMessage } from "ai";
import {
  clampPlaygroundImageDimension,
  normalizePlaygroundImageSize,
  parsePlaygroundImageSize,
} from "../src/components/playground/image-size";
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
  test("accepts custom dimensions within the configured range", () => {
    expect(normalizePlaygroundImageSize("1344x768")).toBe("1344x768");
    expect(normalizePlaygroundImageSize("256x4096")).toBe("256x4096");
    expect(parsePlaygroundImageSize("1023x777")).toEqual({
      width: 1023,
      height: 777,
    });
  });

  test("omits size for auto and invalid values", () => {
    expect(JSON.parse(buildImageGenerationBody(generationInput("")))).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
    });
    expect(
      JSON.parse(buildImageGenerationBody(generationInput("4097x4096"))),
    ).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
    });
    expect(normalizePlaygroundImageSize("255x1024")).toBe("");
    expect(normalizePlaygroundImageSize("1024.5x1024")).toBe("");
  });

  test("clamps numeric input to the supported range", () => {
    expect(clampPlaygroundImageDimension(128)).toBe(256);
    expect(clampPlaygroundImageDimension(768.4)).toBe(768);
    expect(clampPlaygroundImageDimension(8192)).toBe(4096);
  });

  test("adds an explicit size to generation JSON", () => {
    expect(
      JSON.parse(buildImageGenerationBody(generationInput("1344x768"))),
    ).toEqual({
      model: "gpt-image-2",
      prompt: "draw a lighthouse",
      n: 1,
      size: "1344x768",
    });
  });

  test("adds an explicit size to the edit form", () => {
    const attachment: ComposerAttachment = {
      id: "source",
      file: new File(["image"], "source.png", { type: "image/png" }),
      url: "data:image/png;base64,aW1hZ2U=",
    };
    const form = buildImageEditForm({
      ...generationInput("832x1216"),
      attachment,
    });

    expect(form.get("model")).toBe("gpt-image-2");
    expect(form.get("prompt")).toBe("draw a lighthouse");
    expect(form.get("n")).toBe("1");
    expect(form.get("size")).toBe("832x1216");
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
