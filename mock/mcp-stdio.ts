type JsonRpcRequest = {
  jsonrpc?: string;
  id?: number | string;
  method?: string;
  params?: any;
};

function writeResponse(message: any) {
  const payload = JSON.stringify(message);
  process.stdout.write(`${payload}\n`);
}

function handleRequest(req: JsonRpcRequest) {
  const id = req.id ?? null;
  if (req.method === "tools/list") {
    writeResponse({
      jsonrpc: "2.0",
      id,
      result: {
        tools: [
          {
            name: "mcp_echo",
            description: "Echo input text",
            inputSchema: {
              type: "object",
              properties: { text: { type: "string" } },
              required: ["text"],
            },
          },
        ],
      },
    });
    return;
  }
  if (req.method === "tools/call") {
    const name = req.params?.name;
    const args = req.params?.arguments ?? {};
    if (name !== "mcp_echo") {
      writeResponse({
        jsonrpc: "2.0",
        id,
        error: { code: -32000, message: "tool_not_found" },
      });
      return;
    }
    const text = typeof args.text === "string" ? args.text : "";
    writeResponse({
      jsonrpc: "2.0",
      id,
      result: {
        content: [{ type: "text", text: `mcp:${text}` }],
      },
    });
    return;
  }
  writeResponse({
    jsonrpc: "2.0",
    id,
    error: { code: -32601, message: "method_not_found" },
  });
}

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index = buffer.indexOf("\n");
  while (index >= 0) {
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (line.length > 0) {
      try {
        const req = JSON.parse(line);
        handleRequest(req);
      } catch {
        // ignore malformed lines
      }
    }
    index = buffer.indexOf("\n");
  }
});
