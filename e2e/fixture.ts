import { ENDPOINTS, type Protocol } from "./core";
export function fixture(port = 0, commands = ["printf fixture-tool-ok"]) {
  return Bun.serve({
    hostname: "0.0.0.0",
    port,
    async fetch(req) {
      if (req.method !== "POST") return Response.json({ ok: true });
      const body = (await req.json()) as any;
      const p = (Object.entries(ENDPOINTS) as [Protocol, string][]).find(
        ([, v]) => new URL(req.url).pathname === v,
      )?.[0];
      if (!p) return new Response("unknown endpoint", { status: 404 });
      const resultCount =
        p === "responses"
          ? (body.input ?? []).filter((i: any) =>
              ["function_call_output", "custom_tool_call_output"].includes(
                i.type,
              ),
            ).length
          : (body.messages ?? []).filter(
              (m: any) =>
                m.role === "tool" ||
                (Array.isArray(m.content) &&
                  m.content.some((c: any) => c.type === "tool_result")),
            ).length;
      const results = resultCount >= commands.length;
      const shellCommand = commands[resultCount] ?? "true";
      const tool = (body.tools ?? []).find((t: any) =>
        /^(exec_command|shell|bash)$/i.test(t.name ?? t.function?.name),
      );
      const name = tool?.name ?? tool?.function?.name;
      const props =
        tool?.parameters?.properties ??
        tool?.input_schema?.properties ??
        tool?.function?.parameters?.properties ??
        {};
      const args = props.cmd
        ? { cmd: shellCommand }
        : props.command?.type === "array"
          ? { command: ["bash", "-lc", shellCommand] }
          : {
              command: shellCommand,
              description: "Run deterministic fixture tool",
            };
      if (!results && !tool)
        return Response.json(
          { error: { message: "fixture requires shell tool" } },
          { status: 400 },
        );
      const id = "resp_" + crypto.randomUUID();
      const call = `call_fixture_${resultCount}`;
      const text = "Fixture task completed.";
      const events: any[] = [];
      if (p === "responses") {
        const item = results
          ? {
              type: "message",
              id: "msg_fixture",
              role: "assistant",
              status: "completed",
              content: [{ type: "output_text", text, annotations: [] }],
            }
          : {
              type: "function_call",
              id: `fc_fixture_${resultCount}`,
              call_id: call,
              name,
              arguments: JSON.stringify(args),
              status: "completed",
            };
        const response = {
          id,
          object: "response",
          created_at: Math.floor(Date.now() / 1000),
          model: body.model,
          status: "in_progress",
          output: [],
          usage: { input_tokens: 10, output_tokens: 10, total_tokens: 20 },
        };
        events.push(
          { type: "response.created", response },
          { type: "response.in_progress", response },
          {
            type: "response.output_item.added",
            output_index: 0,
            item: {
              ...item,
              ...(!results ? { arguments: "" } : { content: [] }),
            },
          },
        );
        if (results)
          events.push(
            {
              type: "response.content_part.added",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              part: { type: "output_text", text: "", annotations: [] },
            },
            {
              type: "response.output_text.delta",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              delta: text,
            },
            {
              type: "response.output_text.done",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              text,
            },
            {
              type: "response.content_part.done",
              output_index: 0,
              content_index: 0,
              item_id: item.id,
              part: item.content![0],
            },
          );
        else
          events.push(
            {
              type: "response.function_call_arguments.delta",
              output_index: 0,
              item_id: item.id,
              delta: JSON.stringify(args),
            },
            {
              type: "response.function_call_arguments.done",
              output_index: 0,
              item_id: item.id,
              arguments: JSON.stringify(args),
            },
          );
        events.push(
          { type: "response.output_item.done", output_index: 0, item },
          {
            type: "response.completed",
            response: { ...response, status: "completed", output: [item] },
          },
        );
      } else if (p === "messages") {
        events.push(
          {
            type: "message_start",
            message: {
              id,
              type: "message",
              role: "assistant",
              model: body.model,
              content: [],
              stop_reason: null,
              stop_sequence: null,
              usage: { input_tokens: 10, output_tokens: 0 },
            },
          },
          {
            type: "content_block_start",
            index: 0,
            content_block: results
              ? { type: "text", text: "" }
              : { type: "tool_use", id: call, name, input: {} },
          },
          {
            type: "content_block_delta",
            index: 0,
            delta: results
              ? { type: "text_delta", text }
              : {
                  type: "input_json_delta",
                  partial_json: JSON.stringify(args),
                },
          },
          { type: "content_block_stop", index: 0 },
          {
            type: "message_delta",
            delta: {
              stop_reason: results ? "end_turn" : "tool_use",
              stop_sequence: null,
            },
            usage: { output_tokens: 10 },
          },
          { type: "message_stop" },
        );
      } else {
        const chunk = {
          id,
          object: "chat.completion.chunk",
          created: Math.floor(Date.now() / 1000),
          model: body.model,
        };
        events.push(
          {
            ...chunk,
            choices: [
              {
                index: 0,
                delta: results
                  ? { role: "assistant", content: text }
                  : {
                      role: "assistant",
                      tool_calls: [
                        {
                          index: 0,
                          id: call,
                          type: "function",
                          function: { name, arguments: JSON.stringify(args) },
                        },
                      ],
                    },
                finish_reason: null,
              },
            ],
          },
          {
            ...chunk,
            choices: [
              {
                index: 0,
                delta: {},
                finish_reason: results ? "stop" : "tool_calls",
              },
            ],
            usage: {
              prompt_tokens: 10,
              completion_tokens: 10,
              total_tokens: 20,
            },
          },
        );
      }
      const wire =
        events
          .map(
            (e, i) =>
              `${p !== "chat_completion" ? "event: " + e.type + "\n" : ""}data: ${JSON.stringify({ ...e, ...(p === "responses" ? { sequence_number: i } : {}) })}\n\n`,
          )
          .join("") + (p === "chat_completion" ? "data: [DONE]\n\n" : "");
      return new Response(wire, {
        headers: { "content-type": "text/event-stream" },
      });
    },
  });
}
if (import.meta.main) {
  const server = fixture(Number(process.env.PORT ?? 4099));
  console.log(`Controlled fixture: ${server.url}`);
}
