import OpenAI from "openai";

const baseURL = process.env.CHAT_BASE_URL ?? "http://localhost:4141/v1";
const apiKey = process.env.CHAT_API_KEY ?? "test-key";
const model = process.env.CHAT_MODEL ?? "gpt-5-mini";
const reasoningEffort = process.env.CHAT_REASONING_EFFORT ?? "low";

async function main() {
  const client = new OpenAI({ apiKey, baseURL });
  const stream = await client.chat.completions.create({
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
  } as any);

  let content = "";
  let reasoning = "";
  let sawReasoningFrame = false;

  for await (const chunk of stream as AsyncIterable<any>) {
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
