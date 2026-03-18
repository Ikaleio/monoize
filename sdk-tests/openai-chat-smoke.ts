export {};

const baseURL = process.env.CHAT_BASE_URL ?? "[set-CHAT_BASE_URL]";
const apiKey = process.env.CHAT_API_KEY ?? "[set-CHAT_API_KEY]";
const model = process.env.CHAT_MODEL ?? "gpt-5-mini";
const reasoningEffort = process.env.CHAT_REASONING_EFFORT ?? "low";

interface ChatStreamChunk {
  choices?: Array<{
    delta?: {
      content?: string;
      reasoning_details?: Array<{
        type?: string;
        text?: string;
        summary?: string;
      }>;
    };
  }>;
}

async function parseSSE(resp: Response): Promise<ChatStreamChunk[]> {
  const body = resp.body;
  if (!body) {
    throw new Error("Chat completion stream returned no body.");
  }

  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const events: ChatStreamChunk[] = [];

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });

    while (true) {
      const boundary = buffer.indexOf("\n\n");
      if (boundary === -1) break;
      const frame = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);

      const dataLines = frame
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trim());
      if (dataLines.length === 0) continue;
      const payload = dataLines.join("\n");
      if (payload === "[DONE]") continue;
      events.push(JSON.parse(payload) as ChatStreamChunk);
    }
  }

  return events;
}

async function main() {
  const resp = await fetch(`${baseURL}/chat/completions`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model,
      messages: [
        {
          role: "user",
          content:
            "平面四边形 ABCD 中 AB=AD=1, BC=2BD, BD 垂直于 BC，则 AC 的最大值为？（答案是 sqrt5 + 2）",
        },
      ],
      stream: true,
      reasoning_effort: reasoningEffort,
    }),
  });

  if (!resp.ok) {
    throw new Error(`Chat completion request failed with status ${resp.status}.`);
  }

  const events = await parseSSE(resp);

  let content = "";
  let reasoning = "";
  let sawReasoningFrame = false;

  for (const chunk of events) {
    const delta = chunk?.choices?.[0]?.delta ?? {};
    if (typeof delta.content === "string") {
      content += delta.content;
    }
    if (Array.isArray(delta.reasoning_details)) {
      for (const detail of delta.reasoning_details) {
        if (detail?.type === "reasoning.text" && typeof detail.text === "string") {
          reasoning += detail.text;
        } else if (
          detail?.type === "reasoning.summary" &&
          typeof detail.summary === "string"
        ) {
          reasoning += detail.summary;
        }
      }
      sawReasoningFrame = delta.reasoning_details.length > 0 || sawReasoningFrame;
    }
  }

  if (!content.trim() && !reasoning.trim()) {
    throw new Error("Chat completion stream returned empty content and reasoning.");
  }
  if (!sawReasoningFrame) {
    throw new Error("No reasoning_details frames observed in stream.");
  }

  console.log("Chat Completion SDK smoke test passed.");
  console.log(`content: ${content.trim()}`);
  console.log(`reasoning: ${reasoning.trim()}`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
