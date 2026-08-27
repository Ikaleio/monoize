declare const process: {
  env: Record<string, string | undefined>;
};

declare const Bun: {
  serve(options: { port: number; fetch: (req: Request) => Response | Promise<Response> }): void;
};

const port = Number(process.env.PORT ?? 4010);

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function sseResponse(chunks: string[], delayMs = 0) {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      async start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(encoder.encode(chunk));
          if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
        }
        controller.close();
      },
    }),
    {
      status: 200,
      headers: {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      },
    },
  );
}

function collectResponsesText(input: any): string {
  if (typeof input === "string") return input;
  if (!Array.isArray(input)) return "";
  let out = "";
  for (const item of input) {
    if (typeof item === "string") {
      out += item;
      continue;
    }
    if (item?.type === "message" && Array.isArray(item.content)) {
      for (const part of item.content) {
        if (typeof part?.text === "string") out += part.text;
        if (typeof part?.input_text === "string") out += part.input_text;
      }
    }
  }
  return out;
}

function collectChatText(messages: any[]): string {
  let out = "";
  for (const msg of messages) {
    if (typeof msg?.content === "string") out += msg.content;
  }
  return out;
}

function collectToolMessages(messages: any[]): Array<{ toolCallId: string; content: string }> {
  const toolMessages: Array<{ toolCallId: string; content: string }> = [];
  for (const msg of messages) {
    if (msg?.role !== "tool") {
      continue;
    }
    toolMessages.push({
      toolCallId: typeof msg?.tool_call_id === "string" ? msg.tool_call_id : "",
      content: typeof msg?.content === "string" ? msg.content : "",
    });
  }
  return toolMessages;
}

function chatToolCall(id: string, name: string, args: Record<string, unknown>) {
  return {
    id,
    type: "function",
    function: {
      name,
      arguments: JSON.stringify(args),
    },
  };
}

function toolAwareChatResponse(model: string, messages: any[], body: any) {
  const toolMessages = collectToolMessages(messages);
  const toolNames = Array.isArray(body.tools)
    ? body.tools
        .map((tool: any) => tool?.function?.name)
        .filter((name: unknown): name is string => typeof name === "string")
    : [];

  if (toolNames.includes("weather") && toolNames.includes("websearch")) {
    if (toolMessages.length === 0) {
      return {
        id: `chatcmpl_mock_${Date.now()}`,
        object: "chat.completion",
        created: Math.floor(Date.now() / 1000),
        model,
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: "",
              tool_calls: [chatToolCall("call_weather_1", "weather", { city: "Taipei" })],
            },
            finish_reason: "tool_calls",
          },
        ],
      };
    }

    if (toolMessages.length === 1) {
      return {
        id: `chatcmpl_mock_${Date.now()}`,
        object: "chat.completion",
        created: Math.floor(Date.now() / 1000),
        model,
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: "",
              tool_calls: [chatToolCall("call_websearch_1", "websearch", { query: "Monoize" })],
            },
            finish_reason: "tool_calls",
          },
        ],
      };
    }

    const weatherResult = toolMessages.find((message) => message.toolCallId === "call_weather_1")?.content ?? "missing-weather";
    const websearchResult = toolMessages.find((message) => message.toolCallId === "call_websearch_1")?.content ?? "missing-websearch";

    return {
      id: `chatcmpl_mock_${Date.now()}`,
      object: "chat.completion",
      created: Math.floor(Date.now() / 1000),
      model,
      choices: [
        {
          index: 0,
          message: {
            role: "assistant",
            content: `PASS weather=${weatherResult}; websearch=${websearchResult}`,
          },
          finish_reason: "stop",
        },
      ],
    };
  }

  const text = `${collectChatText(messages)}${echoSuffix(body)}`;
  return {
    id: `chatcmpl_mock_${Date.now()}`,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: { role: "assistant", content: text },
        finish_reason: "stop",
      },
    ],
  };
}

function collectAnthropicText(messages: any[]): string {
  let out = "";
  for (const msg of messages) {
    const content = msg?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block?.type === "text" && typeof block?.text === "string") out += block.text;
    }
  }
  return out;
}

function echoSuffix(body: any): string {
  if (body && typeof body.extra_echo === "string" && body.extra_echo.length > 0) {
    return `|extra_echo=${body.extra_echo}`;
  }
  if (body && typeof body.unparsed_field === "string" && body.unparsed_field.length > 0) {
    return `|unparsed_field=${body.unparsed_field}`;
  }
  return "";
}

// 256x256 solid-color PNGs so image responses are visible in UI walkthroughs:
// orange marks /v1/images/generations output, teal marks /v1/images/edits output.
const GENERATION_PNG_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAQAAAAEACAIAAADTED8xAAAB/klEQVR42u3TsQkAAAzDsPy/995m7wsV6AKDsxN4SwIMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA2AAFTAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAADCABBgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAEwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAA4Cvt/F62yjg5YAAAAAElFTkSuQmCC";
const EDIT_PNG_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAQAAAAEACAIAAADTED8xAAACAElEQVR42u3TQQ0AAAjEsJOEdFxgizcaaFIFS5bqgbciAQYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABMIAKGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAYAA4ABwABgADAAGAAMAAbAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAGUAEDgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAAGAAMAAYAAwABgADgAHAABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwABgADAAGAAOAAcAAYAAwAFwLC1gYymvLfH8AAAAASUVORK5CYII=";

// Monoize rejects image responses without billable usage.
const IMAGE_USAGE = {
  total_tokens: 30,
  input_tokens: 10,
  output_tokens: 20,
  input_tokens_details: { text_tokens: 10, image_tokens: 0 },
};

function imageResponse(prompt: string, b64: string) {
  return {
    created: Math.floor(Date.now() / 1000),
    data: [
      {
        b64_json: b64,
        revised_prompt: `mock render of: ${prompt}`,
      },
    ],
    usage: IMAGE_USAGE,
  };
}

// MU14a/MU15a: partial frames per requested partial_images (capped at 3),
// then one completed frame carrying the PNG and the MU14 usage object.
function imageStreamChunks(family: "image_generation" | "image_edit", b64: string, partialImages: number): string[] {
  const createdAt = Math.floor(Date.now() / 1000);
  const partials = Math.max(0, Math.min(3, Math.trunc(partialImages)));
  const chunks: string[] = [];
  for (let index = 0; index < partials; index += 1) {
    chunks.push(
      responsesEventFrame(`${family}.partial_image`, {
        b64_json: b64,
        partial_image_index: index,
        created_at: createdAt,
        output_format: "png",
        size: "256x256",
      }),
    );
  }
  chunks.push(
    responsesEventFrame(`${family}.completed`, {
      b64_json: b64,
      created_at: createdAt,
      output_format: "png",
      size: "256x256",
      usage: IMAGE_USAGE,
    }),
  );
  chunks.push(`data: [DONE]\n\n`);
  return chunks;
}

// MU5: model ids containing "reasoning" activate reasoning output on the
// streaming endpoints so downstream reasoning pipelines can be exercised
// without a real reasoning upstream.
function hasReasoningTrigger(body: any): boolean {
  return String(body?.model ?? "").includes("reasoning");
}

function tokenize(text: string): string[] {
  return text.match(/\S+\s*/g) ?? [];
}

function responsesEventFrame(type: string, data: Record<string, unknown>): string {
  return `event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`;
}

// MU9: full OpenAI Responses lifecycle with a reasoning-summary item followed
// by a message item, so Monoize's upstream decoder reconstructs a Reasoning
// node with summary text.
function reasoningResponsesStream(model: string, text: string): string[] {
  const responseId = `resp_mock_${Date.now()}`;
  const createdAt = Math.floor(Date.now() / 1000);
  const summaryText = `Mock summary of: ${text}`;
  const reasoningItemDone = {
    type: "reasoning",
    id: "rs_mock_1",
    summary: [{ type: "summary_text", text: summaryText }],
  };
  const messageItemDone = {
    type: "message",
    id: "msg_mock_1",
    role: "assistant",
    status: "completed",
    content: [{ type: "output_text", text, annotations: [] }],
  };
  return [
    responsesEventFrame("response.created", {
      response: { id: responseId, object: "response", created_at: createdAt, model, status: "in_progress", output: [] },
    }),
    responsesEventFrame("response.output_item.added", {
      output_index: 0,
      item: { type: "reasoning", id: "rs_mock_1", summary: [] },
    }),
    responsesEventFrame("response.reasoning_summary_part.added", {
      item_id: "rs_mock_1",
      output_index: 0,
      summary_index: 0,
      part: { type: "summary_text", text: "" },
    }),
    ...tokenize(summaryText).map((token) =>
      responsesEventFrame("response.reasoning_summary_text.delta", {
        item_id: "rs_mock_1",
        output_index: 0,
        summary_index: 0,
        delta: token,
      }),
    ),
    responsesEventFrame("response.reasoning_summary_text.done", {
      item_id: "rs_mock_1",
      output_index: 0,
      summary_index: 0,
      text: summaryText,
    }),
    responsesEventFrame("response.reasoning_summary_part.done", {
      item_id: "rs_mock_1",
      output_index: 0,
      summary_index: 0,
      part: { type: "summary_text", text: summaryText },
    }),
    responsesEventFrame("response.output_item.done", {
      output_index: 0,
      item: reasoningItemDone,
    }),
    responsesEventFrame("response.output_item.added", {
      output_index: 1,
      item: { type: "message", id: "msg_mock_1", role: "assistant", status: "in_progress", content: [] },
    }),
    responsesEventFrame("response.content_part.added", {
      item_id: "msg_mock_1",
      output_index: 1,
      content_index: 0,
      part: { type: "output_text", text: "", annotations: [] },
    }),
    ...tokenize(text).map((token) =>
      responsesEventFrame("response.output_text.delta", {
        item_id: "msg_mock_1",
        output_index: 1,
        content_index: 0,
        delta: token,
      }),
    ),
    responsesEventFrame("response.output_text.done", {
      item_id: "msg_mock_1",
      output_index: 1,
      content_index: 0,
      text,
    }),
    responsesEventFrame("response.content_part.done", {
      item_id: "msg_mock_1",
      output_index: 1,
      content_index: 0,
      part: { type: "output_text", text, annotations: [] },
    }),
    responsesEventFrame("response.output_item.done", {
      output_index: 1,
      item: messageItemDone,
    }),
    responsesEventFrame("response.completed", {
      response: {
        id: responseId,
        object: "response",
        created_at: createdAt,
        model,
        status: "completed",
        output: [reasoningItemDone, messageItemDone],
        usage: { input_tokens: 8, output_tokens: 16, total_tokens: 24 },
      },
    }),
    `data: [DONE]\n\n`,
  ];
}

function responsesObject(model: string, text: string) {
  return {
    id: `resp_mock_${Date.now()}`,
    object: "response",
    created: Math.floor(Date.now() / 1000),
    model,
    status: "completed",
    output: [
      {
        type: "message",
        role: "assistant",
        content: [{ type: "output_text", text }],
      },
    ],
  };
}

Bun.serve({
  port,
  fetch: async (req: Request) => {
    const url = new URL(req.url);

    if (url.pathname === "/health") return jsonResponse({ ok: true });

    if (req.method === "POST" && url.pathname === "/v1/responses") {
      const body = await req.json();
      const model = String(body.model ?? "mock-model");
      const text = `${collectResponsesText(body.input)}${echoSuffix(body)}`;

      if (body.stream === true) {
        if (hasReasoningTrigger(body)) {
          return sseResponse(reasoningResponsesStream(model, text), 30);
        }
        const chunks = [
          `event: response.output_text.delta\n` +
            `data: ${JSON.stringify({ text })}\n\n`,
          `data: [DONE]\n\n`,
        ];
        return sseResponse(chunks);
      }

      return jsonResponse(responsesObject(model, text));
    }

    if (req.method === "POST" && url.pathname === "/v1/chat/completions") {
      const body = await req.json();
      const model = String(body.model ?? "mock-chat-model");
      const messages = Array.isArray(body.messages) ? body.messages : [];

      if (body.stream === true) {
        const text = `${collectChatText(messages)}${echoSuffix(body)}`;
        const base = {
          id: `chatcmpl_mock_${Date.now()}`,
          object: "chat.completion.chunk",
          created: Math.floor(Date.now() / 1000),
          model,
        };
        // Word-level deltas plus a terminal finish_reason chunk: Monoize
        // rejects streams that hit [DONE] without a terminal finish_reason.
        const words = text.match(/\S+\s*/g) ?? [];
        // MU12: reasoning-trigger models stream raw CoT via reasoning_content
        // before any content delta, mimicking open-source reasoning upstreams.
        const reasoningWords = hasReasoningTrigger(body)
          ? tokenize(`Mock reasoning about: ${collectChatText(messages)}`)
          : [];
        const chunks = [
          `data: ${JSON.stringify({
            ...base,
            choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }],
          })}\n\n`,
          ...reasoningWords.map(
            (word) =>
              `data: ${JSON.stringify({
                ...base,
                choices: [
                  { index: 0, delta: { reasoning_content: word }, finish_reason: null },
                ],
              })}\n\n`,
          ),
          ...words.map(
            (word) =>
              `data: ${JSON.stringify({
                ...base,
                choices: [{ index: 0, delta: { content: word }, finish_reason: null }],
              })}\n\n`,
          ),
          `data: ${JSON.stringify({
            ...base,
            choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
            usage: { prompt_tokens: 8, completion_tokens: 16, total_tokens: 24 },
          })}\n\n`,
          `data: [DONE]\n\n`,
        ];
        return sseResponse(chunks, 45);
      }

      return jsonResponse(toolAwareChatResponse(model, messages, body));
    }

    if (req.method === "POST" && url.pathname === "/v1/images/generations") {
      const body = await req.json();
      const prompt = String(body.prompt ?? "");
      if (body.stream === true) {
        const partialImages = Number(body.partial_images ?? 0);
        return sseResponse(
          imageStreamChunks("image_generation", GENERATION_PNG_B64, Number.isFinite(partialImages) ? partialImages : 0),
          45,
        );
      }
      return jsonResponse(imageResponse(prompt, GENERATION_PNG_B64));
    }

    if (req.method === "POST" && url.pathname === "/v1/images/edits") {
      const form = await req.formData();
      const prompt = String(form.get("prompt") ?? "");
      const image = form.get("image") ?? form.get("image[]");
      if (!image || typeof image === "string") {
        return jsonResponse(
          { error: { message: "image file field required" } },
          400,
        );
      }
      if (String(form.get("stream") ?? "") === "true") {
        const partialImages = Number(form.get("partial_images") ?? 0);
        return sseResponse(
          imageStreamChunks("image_edit", EDIT_PNG_B64, Number.isFinite(partialImages) ? partialImages : 0),
          45,
        );
      }
      return jsonResponse(imageResponse(prompt, EDIT_PNG_B64));
    }

    if (req.method === "POST" && url.pathname === "/v1/messages") {
      const body = await req.json();
      const model = String(body.model ?? "mock-messages-model");
      const messages = Array.isArray(body.messages) ? body.messages : [];
      const text = `${collectAnthropicText(messages)}${echoSuffix(body)}`;

      if (body.stream === true) {
        const start = {
          type: "message_start",
          message: { id: `msg_mock_${Date.now()}`, type: "message", role: "assistant", model, content: [] },
        };
        const blockStart = {
          type: "content_block_start",
          index: 0,
          content_block: { type: "text", text: "" },
        };
        const delta = {
          type: "content_block_delta",
          index: 0,
          delta: { type: "text_delta", text },
        };
        const stop = { type: "message_stop" };
        const chunks = [
          `data: ${JSON.stringify(start)}\n\n`,
          `data: ${JSON.stringify(blockStart)}\n\n`,
          `data: ${JSON.stringify(delta)}\n\n`,
          `data: ${JSON.stringify(stop)}\n\n`,
        ];
        return sseResponse(chunks);
      }

      return jsonResponse({
        id: `msg_mock_${Date.now()}`,
        type: "message",
        role: "assistant",
        model,
        content: [{ type: "text", text }],
      });
    }

    return jsonResponse({ error: "not found" }, 404);
  },
});

console.log(`mock upstream listening on ${port}`);
