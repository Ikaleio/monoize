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

function sseResponse(chunks: string[]) {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
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
        const chunk = {
          id: `chatcmpl_mock_${Date.now()}`,
          object: "chat.completion.chunk",
          created: Math.floor(Date.now() / 1000),
          model,
          choices: [{ index: 0, delta: { content: text }, finish_reason: null }],
        };
        const chunks = [`data: ${JSON.stringify(chunk)}\n\n`, `data: [DONE]\n\n`];
        return sseResponse(chunks);
      }

      return jsonResponse(toolAwareChatResponse(model, messages, body));
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
