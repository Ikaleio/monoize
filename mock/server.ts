declare const process: {
  env: Record<string, string | undefined>;
};

declare const Bun: {
  serve(options: {
    port: number;
    fetch: (
      req: Request,
      server: { upgrade: (req: Request) => boolean },
    ) => Response | Promise<Response | undefined>;
    websocket: {
      message: (
        ws: { send: (data: string) => void },
        message: string | ArrayBuffer,
      ) => void | Promise<void>;
    };
  }): void;
};

const port = Number(process.env.PORT ?? 4010);
const TEXT_USAGE = { input_tokens: 8, output_tokens: 16 };
const RESPONSES_USAGE = { ...TEXT_USAGE, total_tokens: 24 };
const CHAT_USAGE = {
  prompt_tokens: 8,
  completion_tokens: 16,
  total_tokens: 24,
};

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function object(value: unknown): Record<string, unknown> {
  return isObject(value) ? value : {};
}

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
          if (delayMs > 0)
            await new Promise((resolve) => setTimeout(resolve, delayMs));
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

function collectResponsesText(input: unknown): string {
  if (typeof input === "string") return input;
  if (!Array.isArray(input)) return "";
  let out = "";
  for (const item of input) {
    if (typeof item === "string") {
      out += item;
      continue;
    }
    if (
      isObject(item) &&
      item.type === "message" &&
      Array.isArray(item.content)
    ) {
      for (const part of item.content) {
        if (!isObject(part)) continue;
        if (typeof part.text === "string") out += part.text;
        if (typeof part.input_text === "string") out += part.input_text;
      }
    }
  }
  return out;
}

function collectChatText(messages: unknown[]): string {
  let out = "";
  for (const msg of messages) {
    if (isObject(msg) && typeof msg.content === "string") out += msg.content;
  }
  return out;
}

function collectToolMessages(
  messages: unknown[],
): Array<{ toolCallId: string; content: string }> {
  const toolMessages: Array<{ toolCallId: string; content: string }> = [];
  for (const msg of messages) {
    if (!isObject(msg) || msg.role !== "tool") {
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

function chatCompletion(
  model: string,
  message: Record<string, unknown>,
  finishReason = "stop",
) {
  return {
    id: `chatcmpl_mock_${Date.now()}`,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: { role: "assistant", ...message },
        finish_reason: finishReason,
      },
    ],
    usage: CHAT_USAGE,
  };
}

function toolAwareChatResponse(
  model: string,
  messages: unknown[],
  body: Record<string, unknown>,
) {
  const toolMessages = collectToolMessages(messages);
  const toolNames = Array.isArray(body.tools)
    ? body.tools
        .map((tool: unknown) => object(object(tool).function).name)
        .filter((name: unknown): name is string => typeof name === "string")
    : [];

  if (toolNames.includes("weather") && toolNames.includes("websearch")) {
    if (toolMessages.length === 0) {
      return chatCompletion(
        model,
        {
          content: "",
          tool_calls: [
            chatToolCall("call_weather_1", "weather", { city: "Taipei" }),
          ],
        },
        "tool_calls",
      );
    }

    if (toolMessages.length === 1) {
      return chatCompletion(
        model,
        {
          content: "",
          tool_calls: [
            chatToolCall("call_websearch_1", "websearch", { query: "Monoize" }),
          ],
        },
        "tool_calls",
      );
    }

    const weatherResult =
      toolMessages.find((message) => message.toolCallId === "call_weather_1")
        ?.content ?? "missing-weather";
    const websearchResult =
      toolMessages.find((message) => message.toolCallId === "call_websearch_1")
        ?.content ?? "missing-websearch";

    return chatCompletion(model, {
      content: `PASS weather=${weatherResult}; websearch=${websearchResult}`,
    });
  }

  const text = `${collectChatText(messages)}${echoSuffix(body)}`;
  return chatCompletion(model, { content: text });
}

function collectAnthropicContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter(
      (block: unknown): block is { type: "text"; text: string } =>
        isObject(block) &&
        block.type === "text" &&
        typeof block.text === "string",
    )
    .map((block) => block.text)
    .join("");
}

function collectAnthropicText(messages: unknown[]): string {
  return messages
    .map((message) => collectAnthropicContent(object(message).content))
    .join("");
}

function echoSuffix(body: Record<string, unknown>): string {
  if (
    body &&
    typeof body.extra_echo === "string" &&
    body.extra_echo.length > 0
  ) {
    return `|extra_echo=${body.extra_echo}`;
  }
  if (
    body &&
    typeof body.unparsed_field === "string" &&
    body.unparsed_field.length > 0
  ) {
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
function imageStreamChunks(
  family: "image_generation" | "image_edit",
  b64: string,
  partialImages: number,
): string[] {
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
function hasReasoningTrigger(body: Record<string, unknown>): boolean {
  return String(body?.model ?? "").includes("reasoning");
}

function tokenize(text: string): string[] {
  return text.match(/\s*\S+\s*|\s+/g) ?? [];
}

function responsesEventFrame(
  type: string,
  data: Record<string, unknown>,
): string {
  return `event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`;
}

function warmupResponsesEvents(model: string): Record<string, unknown>[] {
  const id = `resp_mock_${Date.now()}`;
  const createdAt = Math.floor(Date.now() / 1000);
  const response = {
    id,
    object: "response",
    created_at: createdAt,
    model,
    status: "completed",
    output: [],
  };
  return [
    {
      type: "response.created",
      response: { ...response, status: "in_progress" },
    },
    { type: "response.completed", response },
  ];
}

type MockEvent = { type: string; [key: string]: unknown };

function reasoningResponsesEvents(text: string): MockEvent[] {
  const summaryText = `Mock summary of: ${text}`;
  const location = { item_id: "rs_mock_1", output_index: 0, summary_index: 0 };
  return [
    {
      type: "response.output_item.added",
      output_index: 0,
      item: { type: "reasoning", id: "rs_mock_1", summary: [] },
    },
    {
      type: "response.reasoning_summary_part.added",
      ...location,
      part: { type: "summary_text", text: "" },
    },
    ...tokenize(summaryText).map((delta) => ({
      type: "response.reasoning_summary_text.delta",
      ...location,
      delta,
    })),
    {
      type: "response.reasoning_summary_text.done",
      ...location,
      text: summaryText,
    },
    {
      type: "response.reasoning_summary_part.done",
      ...location,
      part: { type: "summary_text", text: summaryText },
    },
    {
      type: "response.output_item.done",
      output_index: 0,
      item: {
        type: "reasoning",
        id: "rs_mock_1",
        summary: [{ type: "summary_text", text: summaryText }],
      },
    },
  ];
}

function messageResponsesEvents(
  text: string,
  outputIndex: number,
): MockEvent[] {
  const location = {
    item_id: "msg_mock_1",
    output_index: outputIndex,
    content_index: 0,
  };
  const item = {
    type: "message",
    id: "msg_mock_1",
    role: "assistant",
    status: "in_progress",
    content: [],
  };
  const part = { type: "output_text", text, annotations: [] };
  return [
    { type: "response.output_item.added", output_index: outputIndex, item },
    {
      type: "response.content_part.added",
      ...location,
      part: { ...part, text: "" },
    },
    ...tokenize(text).map((delta) => ({
      type: "response.output_text.delta",
      ...location,
      delta,
    })),
    { type: "response.output_text.done", ...location, text },
    { type: "response.content_part.done", ...location, part },
    {
      type: "response.output_item.done",
      output_index: outputIndex,
      item: { ...item, status: "completed", content: [part] },
    },
  ];
}

function responsesEvents(
  model: string,
  text: string,
  reasoning: boolean,
): MockEvent[] {
  const response = {
    id: `resp_mock_${Date.now()}`,
    object: "response",
    created_at: Math.floor(Date.now() / 1000),
    model,
  };
  const items = [
    ...(reasoning ? reasoningResponsesEvents(text) : []),
    ...messageResponsesEvents(text, reasoning ? 1 : 0),
  ];
  return [
    {
      type: "response.created",
      response: { ...response, status: "in_progress", output: [] },
    },
    ...items,
    {
      type: "response.completed",
      response: {
        ...response,
        status: "completed",
        usage: RESPONSES_USAGE,
        output: items
          .filter((event) => event.type === "response.output_item.done")
          .map((event) => event.item),
      },
    },
  ].map((event, sequence_number) => ({ ...event, sequence_number }));
}

function messagesBlockEvents(
  index: number,
  text: string,
  thinking: boolean,
): MockEvent[] {
  const contentBlock = thinking
    ? { type: "thinking", thinking: "" }
    : { type: "text", text: "" };
  const deltas = tokenize(text).map((token) => ({
    type: "content_block_delta",
    index,
    delta: thinking
      ? { type: "thinking_delta", thinking: token }
      : { type: "text_delta", text: token },
  }));
  return [
    { type: "content_block_start", index, content_block: contentBlock },
    ...deltas,
    ...(thinking
      ? [
          {
            type: "content_block_delta",
            index,
            delta: {
              type: "signature_delta",
              signature: "mock-thinking-signature",
            },
          },
        ]
      : []),
    { type: "content_block_stop", index },
  ];
}

function messagesStream(
  model: string,
  input: string,
  text: string,
  reasoning: boolean,
): string[] {
  const events: MockEvent[] = [
    {
      type: "message_start",
      message: {
        id: `msg_mock_${Date.now()}`,
        type: "message",
        role: "assistant",
        model,
        content: [],
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 8, output_tokens: 0 },
      },
    },
    ...(reasoning
      ? messagesBlockEvents(0, `Mock reasoning about: ${input}`, true)
      : []),
    ...messagesBlockEvents(reasoning ? 1 : 0, text, false),
    {
      type: "message_delta",
      delta: { stop_reason: "end_turn", stop_sequence: null },
      usage: TEXT_USAGE,
    },
    { type: "message_stop" },
  ];
  return events.map((event) => responsesEventFrame(event.type, event));
}

function systemOneAnswer(value: unknown): Record<string, unknown> {
  const question = object(value);
  const type = typeof question.type === "string" ? question.type : "";
  if (type === "noul") return { type, noul: 0.5 };
  if (type === "choice") {
    const criteria = isObject(question.criteria) ? question.criteria : {};
    const keys = Object.keys(criteria);
    const n = Math.max(keys.length, 1);
    const share = 1 / n;
    const probabilities: Record<string, number> = {};
    for (const key of keys) probabilities[key] = share;
    return {
      type,
      choice: keys.length > 0 ? keys[0] : null,
      probabilities,
      confidence: share,
    };
  }
  if (type === "score") {
    const legend = question?.legend;
    let keys: string[] = [];
    let legendOut: unknown = {};
    if (legend && typeof legend === "object" && !Array.isArray(legend)) {
      keys = Object.keys(legend);
      legendOut = legend;
    } else if (Array.isArray(legend)) {
      keys = legend.map((_, index) => String(index));
      legendOut = legend;
    }
    const n = Math.max(keys.length, 1);
    const share = 1 / n;
    const probabilities: Record<string, number> = {};
    for (const key of keys) probabilities[key] = share;
    return {
      type,
      score: (n - 1) / 2,
      legend: legendOut,
      probabilities,
      confidence: share,
    };
  }
  return { type };
}

function systemOneResponse(body: unknown): Response {
  if (!isObject(body)) {
    return jsonResponse({ error: "body must be object" }, 400);
  }
  const questions = body.questions;
  if (!isObject(questions)) {
    return jsonResponse({ error: "questions must be an object" }, 400);
  }
  const answers: Record<string, unknown> = {};
  for (const key of Object.keys(questions)) {
    answers[key] = systemOneAnswer(questions[key]);
  }
  const text = JSON.stringify({
    state: body.state === undefined ? null : body.state,
    questions,
  });
  const inputTokens = Math.ceil(new TextEncoder().encode(text).length / 4);
  const requested = typeof body.model === "string" ? body.model : "";
  return jsonResponse({
    model: requested === "jev-latest" ? "jev-1.13.0" : requested,
    answers,
    usage: {
      input_tokens: inputTokens,
      output_tokens: Object.keys(questions).length,
    },
  });
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
    usage: RESPONSES_USAGE,
  };
}

function chatStream(
  model: string,
  messages: unknown[],
  body: Record<string, unknown>,
): string[] {
  const text = `${collectChatText(messages)}${echoSuffix(body)}`;
  const base = {
    id: `chatcmpl_mock_${Date.now()}`,
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model,
  };
  const reasoningWords = hasReasoningTrigger(body)
    ? tokenize(`Mock reasoning about: ${collectChatText(messages)}`)
    : [];
  return [
    `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }] })}\n\n`,
    ...reasoningWords.map(
      (word) =>
        `data: ${JSON.stringify({
          ...base,
          choices: [
            {
              index: 0,
              delta: { reasoning_content: word },
              finish_reason: null,
            },
          ],
        })}\n\n`,
    ),
    ...tokenize(text).map(
      (word) =>
        `data: ${JSON.stringify({
          ...base,
          choices: [
            { index: 0, delta: { content: word }, finish_reason: null },
          ],
        })}\n\n`,
    ),
    `data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: CHAT_USAGE })}\n\n`,
    `data: [DONE]\n\n`,
  ];
}

function messagesResponse(body: Record<string, unknown>): Response {
  const model = String(body.model ?? "mock-messages-model");
  const input = collectAnthropicText(
    Array.isArray(body.messages) ? body.messages : [],
  );
  const text = `${input}${echoSuffix(body)}`;
  if (body.stream === true) {
    return sseResponse(
      messagesStream(model, input, text, hasReasoningTrigger(body)),
    );
  }
  return jsonResponse({
    id: `msg_mock_${Date.now()}`,
    type: "message",
    role: "assistant",
    model,
    content: [{ type: "text", text }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: TEXT_USAGE,
  });
}

async function imageEditResponse(req: Request): Promise<Response> {
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
      imageStreamChunks(
        "image_edit",
        EDIT_PNG_B64,
        Number.isFinite(partialImages) ? partialImages : 0,
      ),
      45,
    );
  }
  return jsonResponse(imageResponse(prompt, EDIT_PNG_B64));
}

Bun.serve({
  port,
  websocket: {
    message(ws, message) {
      const raw =
        typeof message === "string"
          ? message
          : new TextDecoder().decode(message);
      let event: Record<string, unknown>;
      try {
        event = object(JSON.parse(raw));
      } catch {
        ws.send(
          JSON.stringify({ type: "error", error: { message: "invalid JSON" } }),
        );
        return;
      }
      if (event?.type !== "response.create") {
        ws.send(
          JSON.stringify({
            type: "error",
            error: { message: "unsupported event" },
          }),
        );
        return;
      }
      const model = String(event.model ?? "mock-model");
      if (event.generate === false) {
        for (const frame of warmupResponsesEvents(model)) {
          ws.send(JSON.stringify(frame));
        }
        return;
      }
      const text = `${collectResponsesText(event.input)}${echoSuffix(event)}`;
      const frames = responsesEvents(model, text, hasReasoningTrigger(event));
      for (const frame of frames) {
        ws.send(JSON.stringify(frame));
      }
    },
  },
  fetch: async (req: Request, server) => {
    const url = new URL(req.url);

    if (url.pathname === "/health") return jsonResponse({ ok: true });

    if (req.method === "GET" && url.pathname === "/v1/responses") {
      if (server.upgrade(req)) return;
      return jsonResponse(
        { error: { message: "WebSocket upgrade failed" } },
        400,
      );
    }

    if (req.method === "POST" && url.pathname === "/v1/responses") {
      const body = object(await req.json());
      const model = String(body.model ?? "mock-model");
      const text = `${collectResponsesText(body.input)}${echoSuffix(body)}`;

      if (body.stream === true) {
        const events = responsesEvents(model, text, hasReasoningTrigger(body));
        const chunks = events.map((event) =>
          responsesEventFrame(event.type, event),
        );
        return sseResponse([...chunks, `data: [DONE]\n\n`], 30);
      }

      return jsonResponse(responsesObject(model, text));
    }

    if (req.method === "POST" && url.pathname === "/v1/chat/completions") {
      const body = object(await req.json());
      const model = String(body.model ?? "mock-chat-model");
      const messages = Array.isArray(body.messages) ? body.messages : [];

      if (body.stream === true) {
        return sseResponse(chatStream(model, messages, body), 45);
      }

      return jsonResponse(toolAwareChatResponse(model, messages, body));
    }

    if (req.method === "POST" && url.pathname === "/v1/images/generations") {
      const body = object(await req.json());
      const prompt = String(body.prompt ?? "");
      if (body.stream === true) {
        const partialImages = Number(body.partial_images ?? 0);
        return sseResponse(
          imageStreamChunks(
            "image_generation",
            GENERATION_PNG_B64,
            Number.isFinite(partialImages) ? partialImages : 0,
          ),
          45,
        );
      }
      return jsonResponse(imageResponse(prompt, GENERATION_PNG_B64));
    }

    if (req.method === "POST" && url.pathname === "/v1/images/edits") {
      return imageEditResponse(req);
    }

    if (req.method === "POST" && url.pathname === "/v1/messages") {
      return messagesResponse(object(await req.json()));
    }

    if (req.method === "POST" && url.pathname === "/v1/messages/count_tokens") {
      const body = object(await req.json());
      const messages = Array.isArray(body.messages) ? body.messages : [];
      const text =
        collectAnthropicContent(body.system) + collectAnthropicText(messages);
      return jsonResponse({
        input_tokens: Math.ceil(new TextEncoder().encode(text).length / 4),
      });
    }

    if (req.method === "POST" && url.pathname === "/v1/systemone") {
      const body = await req.json();
      return systemOneResponse(body);
    }

    return jsonResponse({ error: "not found" }, 404);
  },
});

console.log(`mock upstream listening on ${port}`);
