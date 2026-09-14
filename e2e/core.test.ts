import { test, expect } from "bun:test";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import path from "node:path";
import {
  options,
  redact,
  SSEParser,
  Recorder,
  pool,
  Cleanup,
  HARNESSES,
  ALIAS,
  type Evidence,
} from "./core";
import { createProxy } from "./proxy";
import { validate, command } from "./validate";
const root = path.resolve(import.meta.dir, "..");
const env = {
  BASE_URL: "https://example.com/prefix/v1/responses?api-version=42",
  API_KEY: "secret",
  MODEL: "model",
};
test("CLI precedence, ordered selections, defaults and endpoint validation", async () => {
  const o = await options(
    [
      "--harness",
      "pi-chat,claude-messages",
      "--harness",
      "pi-chat",
      "--model",
      "override",
    ],
    root,
    env,
  );
  expect(o.harnesses).toEqual(["pi-chat", "claude-messages"]);
  expect(o.model).toBe("override");
  expect(o.baseUrl).toBe(env.BASE_URL);
  expect(o.timeout).toBe(1800);
  expect(o.maxRequests).toBe(100);
  expect(o.jobs).toBe(1);
  expect((await options([], root, env)).harnesses).toEqual([...HARNESSES]);
  for (const args of [
    ["--upstream-type", "messages"],
    ["--harness", "unknown"],
    ["--jobs", "0"],
    ["--timeout", "no"],
    ["--task", "a", "--task-file", "b"],
    ["--output", "/tmp/out"],
    ["--bad", "a"],
  ])
    await expect(options(args, root, env)).rejects.toThrow();
  await expect(
    options(["--base-url", "https://example.com/custom"], root, env),
  ).rejects.toThrow();
  expect(
    (
      await options(
        [
          "--base-url",
          "https://example.com/custom?q=1",
          "--upstream-type",
          "messages",
        ],
        root,
        env,
      )
    ).upstreamType,
  ).toBe("messages");
});
test("redaction covers nested secrets, headers, query values and logs", () => {
  const r = redact(
    {
      headers: { Authorization: "Bearer a", "x-api-key": "b" },
      apiKey: "c",
      url: "https://x.test/v1/responses?key=d",
      log: "credential=secret",
    },
    ["secret"],
  );
  expect(JSON.stringify(r)).not.toContain("secret");
  expect(r.headers.Authorization).toBe("[REDACTED]");
  expect(r.apiKey).toBe("[REDACTED]");
  expect(r.url).toContain("key=REDACTED");
});
test("SSE arbitrary UTF8 chunks, CRLF, multiline and incomplete frames", () => {
  const events: any[] = [];
  const p = new SSEParser((e) => events.push(e));
  const data = new TextEncoder().encode(
    'event: x\r\ndata: {"text":"中文",\r\ndata: "ok":true}\r\n\r\ndata: [DONE]\n\n',
  );
  for (const b of data) p.feed(new Uint8Array([b]));
  p.end();
  expect(events).toEqual([{ text: "中文", ok: true }, { type: "sse.done" }]);
  const truncated = new SSEParser(() => {});
  truncated.feed(new TextEncoder().encode("data: {"));
  expect(() => truncated.end()).toThrow();
});
test("evidence limit fails closed and never writes the overflowing secret", async () => {
  const dir = await mkdtemp(path.join(import.meta.dir, ".test-"));
  try {
    const r = new Recorder(dir, ["secret"], 120);
    r.add("a", { text: "secret" });
    r.add("b", { text: "x".repeat(500) });
    expect(r.exceeded).toBe(true);
    expect(await readFile(dir + "/events.jsonl", "utf8")).not.toContain(
      "secret",
    );
    expect(await readFile(dir + "/limit.json", "utf8")).toContain(
      "evidence_limit",
    );
  } finally {
    await rm(dir, { recursive: true });
  }
});
test("bounded pool continues and cleanup attempts all steps once in reverse order", async () => {
  let active = 0,
    max = 0;
  const got = await pool([1, 2, 3], 2, async (x) => {
    max = Math.max(max, ++active);
    await Bun.sleep(5);
    active--;
    return x === 2 ? "failed" : "passed";
  });
  expect(got).toEqual(["passed", "failed", "passed"]);
  expect(max).toBe(2);
  const cleanup = new Cleanup();
  const order: number[] = [];
  cleanup.add(async () => {
    order.push(1);
  });
  cleanup.add(async () => {
    order.push(2);
    throw Error("failure");
  });
  expect((await cleanup.run()).length).toBe(1);
  expect(order).toEqual([2, 1]);
  expect(await cleanup.run()).toEqual([]);
  const abort = new AbortController();
  abort.abort();
  expect(await pool([1, 2], 1, async () => 1, abort.signal)).toEqual([]);
});
function trace(): Evidence[] {
  let seq = 0;
  const records: Evidence[] = [];
  const add = (kind: string, v: any) =>
    records.push({ seq: ++seq, time: "now", kind, ...v });
  add("upgrade", {
    side: "downstream",
    id: "ws",
    headers: { "openai-beta": "responses_websockets=2026-02-06" },
  });
  add("upgraded", { side: "downstream", id: "ws" });
  for (const side of ["downstream", "upstream"])
    for (let turn = 0; turn < 2; turn++) {
      const id = side === "downstream" ? "ws" : `u${turn}`;
      add("request", {
        side,
        id,
        path: "/v1/responses",
        transport: side === "downstream" ? "ws" : undefined,
        body: {
          model: side === "downstream" ? ALIAS : "model",
          ...(turn ? { previous_response_id: "r0" } : {}),
          input: turn
            ? [{ type: "function_call_output", call_id: "call1", output: "ok" }]
            : [{ role: "user", content: "task" }],
        },
      });
      add("event", {
        side,
        id,
        event: {
          type: "response.completed",
          response: {
            id: `r${turn}`,
            output: turn
              ? []
              : [
                  {
                    type: "function_call",
                    call_id: "call1",
                    name: "exec_command",
                    arguments: JSON.stringify({ cmd: "echo ok" }),
                  },
                ],
          },
        },
      });
      add("end", { side, id });
    }
  add("client", {
    event: {
      type: "item.completed",
      item: {
        type: "command_execution",
        command: "echo ok",
        aggregated_output: "ok",
        exit_code: 0,
      },
    },
  });
  add("client", {
    event: {
      type: "item.completed",
      item: { type: "agent_message", text: "Done" },
    },
  });
  return records;
}
test("tool cycle requires execution, response ID, completion and selected transport", () => {
  const run = (r: Evidence[]) =>
    validate(
      "codex-responses-ws-v2",
      "responses",
      "model",
      r,
      0,
      {},
      "",
      "",
      true,
    );
  expect(run(trace()).protocol.passed).toBe(true);
  expect(run(trace().filter((r) => r.kind !== "client")).protocol.passed).toBe(
    false,
  );
  const wrong = trace();
  wrong.find(
    (r) => r.kind === "request" && r.body.previous_response_id,
  )!.body.previous_response_id = "unknown";
  expect(run(wrong).protocol.errors).toContain("missing_ws_v2_continuation");
  const fallback = trace();
  fallback.find((r) => r.kind === "request")!.transport = undefined;
  expect(run(fallback).protocol.errors).toContain("http_fallback");
  expect(
    run(trace().filter((r) => !(r.kind === "event" && r.side === "upstream")))
      .protocol.errors,
  ).toContain("unfinished_upstream_stream");
  expect(
    command({ command: ["bash", "-lc", "cat -- /work/repo/src/main.rs"] }),
  ).toBe("cat -- /work/repo/src/main.rs");
});
test("HTTP recorder preserves full endpoint, JSON bytes, and fragmented SSE", async () => {
  const dir = await mkdtemp(path.join(import.meta.dir, ".test-"));
  let seen = "";
  let body = "";
  const upstream = Bun.serve({
    port: 0,
    fetch: async (req) => {
      seen = req.url;
      body = await req.text();
      return new Response(
        new ReadableStream({
          start(c) {
            for (const s of ['data: {"type":', '"response.completed"}\n', "\n"])
              c.enqueue(new TextEncoder().encode(s));
            c.close();
          },
        }),
        { headers: { "content-type": "text/event-stream" } },
      );
    },
  });
  const recorder = new Recorder(dir);
  const proxy = createProxy(
    {
      monoize: upstream.url.toString(),
      baseUrl: `${upstream.url}custom?version=7`,
      upstreamType: "responses",
      maxRequests: 1,
    },
    recorder,
  );
  try {
    const payload = '{"model":"model","input":[]}';
    const response = await fetch(
      `${proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1")}upstream/v1/responses`,
      { method: "POST", body: payload },
    );
    expect(await response.text()).toContain("response.completed");
    expect(seen).toBe(`${upstream.url}custom?version=7`);
    expect(body).toBe(payload);
    const records = (await readFile(dir + "/events.jsonl", "utf8"))
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    expect(
      records.some(
        (r) => r.kind === "event" && r.event.type === "response.completed",
      ),
    ).toBe(true);
    await (
      await fetch(
        `${proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1")}v1/responses`,
        { method: "POST", body: payload },
      )
    ).text();
    expect(
      (
        await fetch(
          `${proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1")}v1/responses`,
          { method: "POST", body: payload },
        )
      ).status,
    ).toBe(429);
    expect(proxy.status().fatal).toBe("request_limit");
  } finally {
    proxy.stop();
    upstream.stop(true);
    await rm(dir, { recursive: true });
  }
});
test("WS proxy forwards unchanged JSON and excludes warmup from request limit", async () => {
  const dir = await mkdtemp(path.join(import.meta.dir, ".test-"));
  const received: string[] = [];
  const upstream = Bun.serve({
    port: 0,
    fetch(req, server) {
      if (server.upgrade(req)) return;
      return new Response("upgrade required", { status: 400 });
    },
    websocket: {
      message(ws, msg) {
        received.push(String(msg));
        ws.send(
          JSON.stringify({ type: "response.completed", response: { id: "r" } }),
        );
      },
    },
  });
  const recorder = new Recorder(dir);
  const proxy = createProxy(
    {
      monoize: upstream.url.toString(),
      baseUrl: upstream.url.toString(),
      upstreamType: "responses",
      maxRequests: 1,
    },
    recorder,
  );
  try {
    const ws = new WebSocket(
      proxy.server.url.toString().replace("http:", "ws:") + "v1/responses",
    );
    await new Promise<void>((resolve, reject) => {
      ws.onopen = () => resolve();
      ws.onerror = reject;
    });
    const exchange = (payload: any) =>
      new Promise<void>((resolve, reject) => {
        ws.onmessage = () => resolve();
        ws.onerror = reject;
        ws.send(JSON.stringify(payload));
      });
    await exchange({
      type: "response.create",
      model: "model",
      generate: false,
      input: [],
    });
    await exchange({ type: "response.create", model: "model", input: [] });
    expect(received.length).toBe(2);
    expect(proxy.status().generations).toBe(1);
    ws.close();
  } finally {
    proxy.stop();
    upstream.stop(true);
    await Bun.sleep(20);
    await rm(dir, { recursive: true });
  }
});

test("upstream HTTP errors cannot become successful evidence", async () => {
  const directory = await mkdtemp(path.join(import.meta.dir, ".test-"));
  const upstream = Bun.serve({
    port: 0,
    fetch: () =>
      new Response('{"error":{"message":"denied"}}', {
        status: 401,
        headers: { "content-type": "application/json" },
      }),
  });
  const recorder = new Recorder(directory);
  const proxy = createProxy(
    {
      monoize: upstream.url.toString(),
      baseUrl: upstream.url.toString(),
      upstreamType: "responses",
      maxRequests: 5,
    },
    recorder,
  );
  try {
    const response = await fetch(
      proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1") +
        "upstream/v1/responses",
      { method: "POST", body: JSON.stringify({ model: "model", input: [] }) },
    );
    expect(response.status).toBe(401);
    await response.text();
    const records = (await readFile(directory + "/events.jsonl", "utf8"))
      .trim()
      .split("\n")
      .map((l) => JSON.parse(l));
    expect(
      records.some((r) => r.kind === "http_error" && r.status === 401),
    ).toBe(true);
    expect(
      validate(
        "pi-responses",
        "responses",
        "model",
        records,
        0,
        {},
        "",
        "",
        true,
      ).protocol.passed,
    ).toBe(false);
  } finally {
    proxy.stop();
    upstream.stop(true);
    await rm(directory, { recursive: true });
  }
});
test("CLI invalid configuration exits 2 without creating containers", async () => {
  const child = Bun.spawn(
    [
      "bun",
      path.join(import.meta.dir, "cli.ts"),
      "run",
      "--harness",
      "invalid",
      "--base-url",
      "https://example.com/v1/responses",
      "--api-key",
      "test-secret",
      "--model",
      "model",
    ],
    { stdout: "pipe", stderr: "pipe" },
  );
  const text = await new Response(child.stderr).text();
  expect(await child.exited).toBe(2);
  expect(text).not.toContain("test-secret");
});
test("task acceptance cannot pass with a fabricated clone or source read", () => {
  const records = trace();
  const snapshot = {
    origin: "https://example.com/repo",
    commit: "abc",
    document:
      "# Entry points\nA\n# Modules\nB\n# Request flow\nC\n# Sources\n`a.rs` `b.rs` `c.rs`",
    files: ["a.rs", "b.rs", "c.rs"],
  };
  const result = validate(
    "codex-responses-ws-v2",
    "responses",
    "model",
    records,
    0,
    snapshot,
    "https://example.com/repo",
    "abc",
    false,
  );
  expect(result.task.passed).toBe(false);
  expect(result.task.errors).toContain("missing_clone_execution");
  expect(result.task.errors).toContain("source_read_evidence");
});
test("aborting an executing command does not leave it running", async () => {
  const { command: execute } = await import("./runner");
  const abort = new AbortController();
  const process = execute(
    ["bun", "-e", "await Bun.sleep(60000)"],
    abort.signal,
  );
  setTimeout(() => abort.abort(new Error("interrupted")), 20);
  await expect(process).rejects.toThrow("interrupted");
});

test("build proxies map loopback without changing runtime configuration", async () => {
  const { buildProxyArgs } = await import("./runner");
  expect(
    buildProxyArgs({
      HTTP_PROXY: "http://127.0.0.1:7890",
      https_proxy: "http://localhost:7890",
      NO_PROXY: "localhost",
    }),
  ).toEqual([
    "--build-arg",
    "HTTP_PROXY=http://host.docker.internal:7890/",
    "--build-arg",
    "HTTPS_PROXY=http://host.docker.internal:7890/",
    "--build-arg",
    "NO_PROXY=localhost",
  ]);
});

test("endpoint and Monoize capture correlation are required for live validation", () => {
  const records = trace();
  const endpoint = "https://example.com/v1/responses";
  for (const r of records.filter(
    (r) => r.kind === "request" && r.side === "upstream",
  )) {
    r.target = endpoint;
    records.push({
      seq: records.length,
      time: new Date().toISOString(),
      kind: "capture",
      capture: {
        request_id: r.id,
        downstream_protocol: "responses",
        attempts: [
          {
            provider_type: "responses",
            logical_model: ALIAS,
            upstream_model: "model",
            upstream_request: r.body,
          },
        ],
      },
    } as Evidence);
  }
  const check = (r: Evidence[]) =>
    validate(
      "codex-responses-ws-v2",
      "responses",
      "model",
      r,
      0,
      {},
      "",
      "",
      true,
      endpoint,
    ).protocol;
  expect(check(records).passed).toBe(true);
  expect(check(records.filter((r) => r.kind !== "capture")).errors).toContain(
    "uncorrelated_monoize_capture",
  );
  records.find((r) => r.kind === "request" && r.side === "upstream")!.target =
    endpoint + "/wrong";
  expect(check(records).errors).toContain("upstream_endpoint");
});

test("mid-stream disconnect preserves incomplete evidence without a synthetic end", async () => {
  const directory = await mkdtemp(path.join(import.meta.dir, ".test-"));
  const upstream = Bun.serve({
    port: 0,
    fetch: () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(
              new TextEncoder().encode(
                'data: {"type":"response.created","response":{"id":"r"}}\n\n',
              ),
            );
            setTimeout(
              () => controller.error(new Error("fixture_disconnect")),
              20,
            );
          },
        }),
        { headers: { "content-type": "text/event-stream" } },
      ),
    error: () => new Response("fixture disconnect", { status: 500 }),
  });
  const recorder = new Recorder(directory);
  const proxy = createProxy(
    {
      monoize: upstream.url.toString(),
      baseUrl: upstream.url.toString(),
      upstreamType: "responses",
      maxRequests: 5,
    },
    recorder,
  );
  try {
    const response = await fetch(
      proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1") +
        "upstream/v1/responses",
      { method: "POST", body: JSON.stringify({ model: "model", input: [] }) },
    );
    await response.text().catch(() => "");
    const records = (await readFile(directory + "/events.jsonl", "utf8"))
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    expect(records.some((r) => r.kind === "transport_error")).toBe(true);
    expect(records.some((r) => r.event?.type === "response.completed")).toBe(
      false,
    );
    expect(
      validate(
        "pi-responses",
        "responses",
        "model",
        records,
        0,
        {},
        "",
        "",
        true,
      ).protocol.passed,
    ).toBe(false);
  } finally {
    proxy.stop();
    upstream.stop(true);
    await rm(directory, { recursive: true });
  }
});

test("terminal SSE errors stop retries and preserve the original event", async () => {
  for (const event of [
    { error: { message: "overloaded" } },
    { type: "error", message: "overloaded" },
    { type: "response.failed", response: { error: { message: "overloaded" } } },
  ]) {
    const directory = await mkdtemp(path.join(import.meta.dir, ".test-"));
    const payload = `data: ${JSON.stringify(event)}\n\n`;
    const upstream = Bun.serve({
      port: 0,
      fetch: () =>
        new Response(payload, {
          headers: { "content-type": "text/event-stream" },
        }),
    });
    const proxy = createProxy(
      {
        monoize: upstream.url.toString(),
        baseUrl: upstream.url.toString(),
        upstreamType: "responses",
        maxRequests: 5,
      },
      new Recorder(directory),
    );
    try {
      const response = await fetch(
        proxy.server.url.toString().replace("0.0.0.0", "127.0.0.1") +
          "upstream/v1/responses",
        { method: "POST", body: JSON.stringify({ model: "model", input: [] }) },
      );
      expect(await response.text()).toBe(payload);
      expect(proxy.status().fatal).toBe("upstream_error");
    } finally {
      proxy.stop();
      upstream.stop(true);
      await rm(directory, { recursive: true });
    }
  }
});
