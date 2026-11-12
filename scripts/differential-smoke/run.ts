#!/usr/bin/env bun
// Differential smoke harness: runs two Monoize binaries against the same local
// mock upstream with identical SQLite state, replays a fixed request matrix,
// normalizes volatile fields, and diffs the downstream responses.
//
// Usage: see spec/differential-smoke.spec.md.
// --expected (default scripts/differential-smoke/expected-diffs.json) is a JSON array of
// { "case": <regex on case name>, "where": <regex on diff path>, "note"?: string };
// matching diffs are still reported but do not fail the run.
// The seed DB is bootstrapped with --base on first use (or --rebootstrap) and
// copied fresh into each run directory.
//
// Every server runs under sandbox-exec with outbound network limited to
// localhost (minus the local HTTP proxy port), and with proxy env removed.
import { Database } from "bun:sqlite";
import { parseArgs } from "node:util";
import {
  accessSync,
  closeSync,
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
  readFileSync,
  openSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "../..");
const SMOKE = join(ROOT, "artifacts/smoke");
const MOCK_DIR = join(ROOT, "mock");
const USAGE =
  "usage: bun run scripts/differential-smoke/run.ts --base <bin> --new <bin> [--only regex] [--out directory] [--rebootstrap] [--expected json]";

type CliOptions = {
  base: string;
  new: string;
  rebootstrap: boolean;
  expected: string;
  only?: string;
  out?: string;
};

function parseCli(): CliOptions {
  const { values } = parseArgs({
    options: {
      base: { type: "string" },
      new: { type: "string" },
      rebootstrap: { type: "boolean", default: false },
      expected: {
        type: "string",
        default: join(import.meta.dir, "expected-diffs.json"),
      },
      only: { type: "string" },
      out: { type: "string" },
    },
  });
  if (!values.base || !values.new) throw new Error(USAGE);
  if (process.platform !== "darwin")
    throw new Error(
      "unsupported platform: differential smoke requires macOS sandbox-exec; unrestricted fallback is disabled",
    );
  try {
    accessSync("/usr/bin/sandbox-exec", constants.X_OK);
  } catch {
    throw new Error(
      "network isolation unavailable: /usr/bin/sandbox-exec is not executable",
    );
  }
  for (const bin of [values.base, values.new]) {
    accessSync(resolve(bin), constants.X_OK);
    if (!statSync(resolve(bin)).isFile())
      throw new Error(`binary is not a file: ${bin}`);
  }
  return { ...values, base: values.base, new: values.new };
}

let args: CliOptions;
try {
  args = parseCli();
} catch (error) {
  console.error(String(error));
  process.exit(2);
}
const BASE_BIN = resolve(args.base);
const NEW_BIN = resolve(args.new);
const SEED_DB = join(SMOKE, "seed/monoize.db");
const SEED_META = join(dirname(SEED_DB), "meta.json");

// ---------------------------------------------------------------- JSON helpers

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

/** Reads a nested field from parsed JSON; returns undefined on any shape mismatch. */
function dig(value: unknown, ...path: (string | number)[]): unknown {
  let cur: unknown = value;
  for (const key of path) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string | number, unknown>)[key];
  }
  return cur;
}
function digArray(value: unknown, ...path: (string | number)[]): JsonObject[] {
  const v = dig(value, ...path);
  return Array.isArray(v)
    ? (v.filter(
        (x) => x && typeof x === "object" && !Array.isArray(x),
      ) as JsonObject[])
    : [];
}
function digString(value: unknown, ...path: (string | number)[]): string {
  const v = dig(value, ...path);
  return typeof v === "string" ? v : "";
}

// ---------------------------------------------------------------- processes

type Proc = { name: string; proc: Bun.Subprocess; stopping?: Promise<void> };
const procs = new Set<Proc>();

function stopProc(p: Proc): Promise<void> {
  if (p.stopping) return p.stopping;
  p.stopping = (async () => {
    try {
      p.proc.kill("SIGTERM");
    } catch {}
    const timer = setTimeout(() => {
      try {
        p.proc.kill("SIGKILL");
      } catch {}
    }, 5000);
    try {
      await p.proc.exited;
    } finally {
      clearTimeout(timer);
      procs.delete(p);
    }
  })();
  return p.stopping;
}
for (const [signal, code] of [
  ["SIGINT", 130],
  ["SIGTERM", 143],
] as const) {
  process.on(signal, async () => {
    await Promise.all([...procs].map(stopProc));
    process.exit(code);
  });
}

async function freePort(): Promise<number> {
  const server = Bun.listen({
    hostname: "127.0.0.1",
    port: 0,
    socket: { data() {} },
  });
  const port = server.port;
  server.stop(true);
  return port;
}

const PROXY_VARS = [
  "HTTPS_PROXY",
  "HTTP_PROXY",
  "ALL_PROXY",
  "https_proxy",
  "http_proxy",
  "all_proxy",
  "NODE_USE_ENV_PROXY",
];
function cleanEnv(extra: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v !== undefined && !PROXY_VARS.includes(k)) env[k] = v;
  }
  env.NO_PROXY = "*";
  env.no_proxy = "*";
  return { ...env, ...extra };
}

// Outbound network: localhost only, explicitly excluding the local proxy port.
const SANDBOX_PROFILE = [
  "(version 1)",
  "(allow default)",
  "(deny network-outbound)",
  '(allow network-outbound (remote ip "localhost:*"))',
  "(allow network-outbound (remote unix-socket))",
  '(deny network-outbound (remote ip "localhost:7890"))',
].join("");

async function waitHttp(url: string, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
      last = `HTTP ${res.status}`;
    } catch (e) {
      last = String(e);
    }
    await Bun.sleep(200);
  }
  throw new Error(`timeout waiting for ${url}: ${last}`);
}

async function startMock(port: number, logPath: string): Promise<Proc> {
  const fd = openSync(logPath, "w");
  let p: Proc;
  try {
    const proc = Bun.spawn(
      [
        "/usr/bin/sandbox-exec",
        "-p",
        SANDBOX_PROFILE,
        process.execPath,
        "run",
        "server.ts",
      ],
      {
        cwd: MOCK_DIR,
        env: cleanEnv({ PORT: String(port) }),
        stdout: fd,
        stderr: fd,
      },
    );
    p = { name: "mock", proc };
    procs.add(p);
  } finally {
    closeSync(fd);
  }
  await waitHttp(`http://127.0.0.1:${port}/health`);
  return p;
}

async function startServer(
  name: string,
  bin: string,
  port: number,
  dbPath: string,
  workDir: string,
): Promise<Proc> {
  mkdirSync(workDir, { recursive: true });
  const fd = openSync(join(workDir, "server.log"), "w");
  let p: Proc;
  try {
    const proc = Bun.spawn(
      ["/usr/bin/sandbox-exec", "-p", SANDBOX_PROFILE, bin],
      {
        cwd: workDir,
        env: cleanEnv({
          MONOIZE_LISTEN: `127.0.0.1:${port}`,
          MONOIZE_DATABASE_DSN: `sqlite://${dbPath}?mode=rwc`,
          MONOIZE_REQUEST_LOG_SPOOL_DIR: join(workDir, "spool"),
          MONOIZE_IMAGE_TRANSFORM_CACHE_DIR: join(workDir, "image-cache"),
          RUST_LOG: process.env.SMOKE_RUST_LOG ?? "info",
        }),
        stdout: fd,
        stderr: fd,
      },
    );
    p = { name, proc };
    procs.add(p);
  } finally {
    closeSync(fd);
  }
  await waitHttp(`http://127.0.0.1:${port}/api/dashboard/settings/public`);
  return p;
}

function setSettings(dbPath: string, kv: Record<string, string>) {
  const db = new Database(dbPath);
  const now = new Date().toISOString();
  const stmt = db.prepare(
    "INSERT INTO system_settings (key, value, updated_at) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
  );
  for (const [k, v] of Object.entries(kv)) stmt.run(k, v, now);
  db.close();
}

function checkpoint(dbPath: string) {
  const db = new Database(dbPath);
  db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  db.close();
}

function copyDb(src: string, dst: string) {
  mkdirSync(dirname(dst), { recursive: true });
  copyFileSync(src, dst);
  for (const suffix of ["-wal", "-shm"]) {
    if (existsSync(src + suffix)) copyFileSync(src + suffix, dst + suffix);
  }
}

// ---------------------------------------------------------------- bootstrap

const UPSTREAMS = [
  { type: "responses", models: ["up-responses", "up-responses-reasoning"] },
  { type: "chat_completion", models: ["up-chat", "up-chat-reasoning"] },
  { type: "messages", models: ["up-messages", "up-messages-reasoning"] },
] as const;
const PLACEHOLDER_BASE_URL = "http://127.0.0.1:1";

type SeedMeta = { apiKey: string; username: string; createdWith: string };

async function api(
  port: number,
  method: string,
  path: string,
  body: unknown,
  token?: string,
) {
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}: ${text}`);
  return text ? JSON.parse(text) : null;
}

async function bootstrap(): Promise<SeedMeta> {
  const seedDir = dirname(SEED_DB);
  rmSync(seedDir, { recursive: true, force: true });
  mkdirSync(seedDir, { recursive: true });
  const port = await freePort();
  const work = join(seedDir, "bootstrap-work");
  console.log(`[bootstrap] starting base binary on ${port}`);
  const server = await startServer("bootstrap", BASE_BIN, port, SEED_DB, work);
  try {
    // Settings are read from the DB per call, so direct writes apply immediately.
    setSettings(SEED_DB, {
      captcha_enabled: "false",
      price_sync_auto_enabled: "false",
      monoize_active_probe_enabled: "false",
    });
    const username = "smoke_admin";
    const reg = await api(port, "POST", "/api/dashboard/auth/register", {
      username,
      password: "smoke-password-1",
    });
    const token: string = reg.token;
    if (reg.user?.role !== "super_admin")
      throw new Error(`first user role is ${reg.user?.role}`);
    for (const up of UPSTREAMS) {
      const models: Record<string, { redirect: null; multiplier: string }> = {};
      for (const m of up.models)
        models[m] = { redirect: null, multiplier: "1" };
      await api(
        port,
        "POST",
        "/api/dashboard/providers",
        {
          name: `mock-${up.type}`,
          channels: [
            {
              name: `mock-${up.type}`,
              provider_type: up.type,
              base_url: PLACEHOLDER_BASE_URL,
              api_key: "sk-mock-not-a-real-key",
              models,
            },
          ],
          circuit_breaker_enabled: false,
          allow_free_when_unpriced_override: true,
          allow_free_when_missing_usage_override: true,
        },
        token,
      );
    }
    const key = await api(
      port,
      "POST",
      "/api/dashboard/tokens",
      { name: "smoke" },
      token,
    );
    const meta: SeedMeta = { apiKey: key.key, username, createdWith: BASE_BIN };
    writeFileSync(SEED_META, JSON.stringify(meta, null, 2));
    return meta;
  } finally {
    await stopProc(server);
    checkpoint(SEED_DB);
    for (const s of ["-wal", "-shm"]) rmSync(SEED_DB + s, { force: true });
  }
}

function prepareRunDb(dst: string, mockPort: number) {
  copyDb(SEED_DB, dst);
  const db = new Database(dst);
  const r = db
    .prepare("UPDATE monoize_channels SET base_url = ?")
    .run(`http://127.0.0.1:${mockPort}`);
  db.close();
  if (r.changes !== UPSTREAMS.length)
    throw new Error(
      `expected ${UPSTREAMS.length} channels, updated ${r.changes}`,
    );
}

// ---------------------------------------------------------------- HTTP capture

type Frame = { event?: string; id?: string; data: unknown };
type Step = {
  label: string;
  request: { path: string; body: unknown };
  status: number;
  contentType: string | null;
  kind: "json" | "sse" | "text";
  body?: unknown;
  frames?: Frame[];
  droppedComments?: number;
  error?: string;
};

function parseSse(text: string): { frames: Frame[]; comments: number } {
  const frames: Frame[] = [];
  let comments = 0;
  for (const block of text.split(/\r?\n\r?\n/)) {
    if (!block.trim()) continue;
    let event: string | undefined;
    let id: string | undefined;
    const data: string[] = [];
    let onlyComments = true;
    for (const line of block.split(/\r?\n/)) {
      if (line.startsWith(":")) continue;
      onlyComments = false;
      const i = line.indexOf(":");
      const field = i < 0 ? line : line.slice(0, i);
      let value = i < 0 ? "" : line.slice(i + 1);
      if (value.startsWith(" ")) value = value.slice(1);
      if (field === "event") event = value;
      else if (field === "id") id = value;
      else if (field === "data") data.push(value);
    }
    if (onlyComments) {
      comments++;
      continue;
    }
    const raw = data.join("\n");
    let parsed: unknown = raw;
    try {
      parsed = JSON.parse(raw);
    } catch {}
    const frame: Frame = { data: parsed };
    if (event !== undefined) frame.event = event;
    if (id !== undefined) frame.id = id;
    frames.push(frame);
  }
  return { frames, comments };
}

async function send(
  port: number,
  apiKey: string,
  label: string,
  path: string,
  body: unknown,
): Promise<Step> {
  const step: Step = {
    label,
    request: { path, body },
    status: 0,
    contentType: null,
    kind: "text",
  };
  try {
    const headers: Record<string, string> = {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    };
    if (path.startsWith("/v1/messages")) {
      headers["x-api-key"] = apiKey;
      headers["anthropic-version"] = "2023-06-01";
    }
    const res = await fetch(`http://127.0.0.1:${port}${path}`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(90000),
    });
    step.status = res.status;
    step.contentType = res.headers.get("content-type");
    const text = await res.text();
    if (step.contentType?.includes("text/event-stream")) {
      const { frames, comments } = parseSse(text);
      step.kind = "sse";
      step.frames = frames;
      step.droppedComments = comments;
    } else {
      try {
        step.body = JSON.parse(text);
        step.kind = "json";
      } catch {
        step.body = text;
      }
    }
  } catch (e) {
    step.error = String(e);
  }
  return step;
}

// ---------------------------------------------------------------- matrix

type Ctx = { port: number; apiKey: string };
type CaseDef = { name: string; run: (ctx: Ctx) => Promise<Step[]> };

const SYSTEM = "Be brief.";
const USER = "Hello from smoke";

function downstreamBody(
  proto: "responses" | "chat" | "messages",
  model: string,
  stream: boolean,
): unknown {
  if (proto === "responses") {
    return {
      model,
      instructions: SYSTEM,
      input: [
        {
          type: "message",
          role: "user",
          content: [{ type: "input_text", text: USER }],
        },
      ],
      max_output_tokens: 256,
      stream,
    };
  }
  if (proto === "chat") {
    return {
      model,
      messages: [
        { role: "system", content: SYSTEM },
        { role: "user", content: USER },
      ],
      max_tokens: 256,
      stream,
      ...(stream ? { stream_options: { include_usage: true } } : {}),
    };
  }
  return {
    model,
    system: SYSTEM,
    messages: [{ role: "user", content: [{ type: "text", text: USER }] }],
    max_tokens: 256,
    stream,
  };
}
const PATHS = {
  responses: "/v1/responses",
  chat: "/v1/chat/completions",
  messages: "/v1/messages",
} as const;

const TOOL_RESULTS: Record<string, string> = {
  weather: "sunny 25C",
  websearch: "Monoize is an LLM gateway",
};
const TOOL_PARAMS = {
  weather: {
    type: "object",
    properties: { city: { type: "string" } },
    required: ["city"],
  },
  websearch: {
    type: "object",
    properties: { query: { type: "string" } },
    required: ["query"],
  },
};

async function messagesToolLoop(ctx: Ctx, model: string): Promise<Step[]> {
  const steps: Step[] = [];
  const tools = Object.entries(TOOL_PARAMS).map(([name, input_schema]) => ({
    name,
    input_schema,
  }));
  const messages: Json[] = [
    {
      role: "user",
      content: [
        { type: "text", text: "Weather in Taipei, then search Monoize." },
      ],
    },
  ];
  for (let turn = 1; turn <= 3; turn++) {
    const step = await send(
      ctx.port,
      ctx.apiKey,
      `turn${turn}`,
      PATHS.messages,
      {
        model,
        max_tokens: 256,
        messages: structuredClone(messages),
        tools,
        stream: false,
      },
    );
    steps.push(step);
    const content = digArray(step.body, "content");
    const uses = content.filter((b) => b.type === "tool_use");
    if (!uses.length) break;
    messages.push({ role: "assistant", content });
    messages.push({
      role: "user",
      content: uses.map((u) => ({
        type: "tool_result",
        tool_use_id: digString(u, "id"),
        content: TOOL_RESULTS[digString(u, "name")] ?? "unknown",
      })),
    });
  }
  return steps;
}

function toolLoopCase(proto: "responses" | "chat" | "messages"): CaseDef {
  const model = "up-chat";
  return {
    name: `toolloop/${proto}->chat_completion`,
    async run(ctx) {
      if (proto === "messages") return messagesToolLoop(ctx, model);
      const steps: Step[] = [];
      if (proto === "chat") {
        const tools = Object.entries(TOOL_PARAMS).map(([name, parameters]) => ({
          type: "function",
          function: { name, parameters },
        }));
        const messages: Json[] = [
          { role: "user", content: "Weather in Taipei, then search Monoize." },
        ];
        for (let turn = 1; turn <= 3; turn++) {
          const step = await send(
            ctx.port,
            ctx.apiKey,
            `turn${turn}`,
            PATHS.chat,
            {
              model,
              messages: structuredClone(messages),
              tools,
              stream: false,
            },
          );
          steps.push(step);
          const msg = dig(step.body, "choices", 0, "message") as
            JsonObject | undefined;
          const calls = digArray(msg, "tool_calls");
          if (!msg || !calls.length) break;
          messages.push(msg);
          for (const call of calls) {
            messages.push({
              role: "tool",
              tool_call_id: digString(call, "id"),
              content:
                TOOL_RESULTS[digString(call, "function", "name")] ?? "unknown",
            });
          }
        }
      } else if (proto === "responses") {
        const tools = Object.entries(TOOL_PARAMS).map(([name, parameters]) => ({
          type: "function",
          name,
          parameters,
        }));
        const input: Json[] = [
          {
            type: "message",
            role: "user",
            content: [
              {
                type: "input_text",
                text: "Weather in Taipei, then search Monoize.",
              },
            ],
          },
        ];
        for (let turn = 1; turn <= 3; turn++) {
          const step = await send(
            ctx.port,
            ctx.apiKey,
            `turn${turn}`,
            PATHS.responses,
            { model, input: structuredClone(input), tools, stream: false },
          );
          steps.push(step);
          const output = digArray(step.body, "output");
          const calls = output.filter((o) => o.type === "function_call");
          if (!calls.length) break;
          input.push(...output);
          for (const call of calls) {
            input.push({
              type: "function_call_output",
              call_id: digString(call, "call_id"),
              output: TOOL_RESULTS[digString(call, "name")] ?? "unknown",
            });
          }
        }
      }
      return steps;
    },
  };
}

function multiTurnCases(): CaseDef[] {
  return [
    {
      // Streamed turn 1 on the reasoning trigger; its completed output items
      // (reasoning + message) go back as turn-2 input.
      name: "multiturn/responses->responses/reasoning/stream",
      async run(ctx) {
        const model = "up-responses-reasoning";
        const input: Json[] = [
          {
            type: "message",
            role: "user",
            content: [{ type: "input_text", text: "First question" }],
          },
        ];
        const s1 = await send(ctx.port, ctx.apiKey, "turn1", PATHS.responses, {
          model,
          input: structuredClone(input),
          stream: true,
        });
        const completed = s1.frames?.find(
          (f) => dig(f.data, "type") === "response.completed",
        );
        const output = digArray(completed?.data, "response", "output");
        input.push(...output, {
          type: "message",
          role: "user",
          content: [{ type: "input_text", text: "Second question" }],
        });
        const s2 = await send(ctx.port, ctx.apiKey, "turn2", PATHS.responses, {
          model,
          input: structuredClone(input),
          stream: true,
        });
        return [s1, s2];
      },
    },
    {
      name: "multiturn/chat->messages/plain/nonstream",
      async run(ctx) {
        const model = "up-messages";
        const messages: Json[] = [{ role: "user", content: "First question" }];
        const s1 = await send(ctx.port, ctx.apiKey, "turn1", PATHS.chat, {
          model,
          messages: structuredClone(messages),
          stream: false,
        });
        const msg = dig(s1.body, "choices", 0, "message") as
          JsonObject | undefined;
        if (msg) messages.push(msg);
        messages.push({ role: "user", content: "Second question" });
        const s2 = await send(ctx.port, ctx.apiKey, "turn2", PATHS.chat, {
          model,
          messages: structuredClone(messages),
          stream: false,
        });
        return [s1, s2];
      },
    },
    {
      name: "multiturn/messages->chat_completion/reasoning/nonstream",
      async run(ctx) {
        const model = "up-chat-reasoning";
        const messages: Json[] = [
          { role: "user", content: [{ type: "text", text: "First question" }] },
        ];
        const s1 = await send(ctx.port, ctx.apiKey, "turn1", PATHS.messages, {
          model,
          max_tokens: 256,
          messages: structuredClone(messages),
          stream: false,
        });
        const content = digArray(s1.body, "content");
        if (content.length) messages.push({ role: "assistant", content });
        messages.push({
          role: "user",
          content: [{ type: "text", text: "Second question" }],
        });
        const s2 = await send(ctx.port, ctx.apiKey, "turn2", PATHS.messages, {
          model,
          max_tokens: 256,
          messages: structuredClone(messages),
          stream: false,
        });
        return [s1, s2];
      },
    },
  ];
}

function buildMatrix(): CaseDef[] {
  const cases: CaseDef[] = [];
  for (const proto of ["responses", "chat", "messages"] as const) {
    for (const up of UPSTREAMS) {
      for (const model of up.models) {
        for (const stream of [false, true]) {
          const variant = model.endsWith("-reasoning") ? "reasoning" : "plain";
          cases.push({
            name: `${proto}->${up.type}/${variant}/${stream ? "stream" : "nonstream"}`,
            run: async (ctx) => [
              await send(
                ctx.port,
                ctx.apiKey,
                "request",
                PATHS[proto],
                downstreamBody(proto, model, stream),
              ),
            ],
          });
        }
      }
    }
  }
  for (const proto of ["responses", "chat", "messages"] as const)
    cases.push(toolLoopCase(proto));
  cases.push(...multiTurnCases());
  for (const up of UPSTREAMS) {
    cases.push({
      name: `count_tokens/messages->${up.type}`,
      run: async (ctx) => [
        await send(
          ctx.port,
          ctx.apiKey,
          "request",
          "/v1/messages/count_tokens",
          {
            model: up.models[0],
            system: SYSTEM,
            messages: [
              { role: "user", content: [{ type: "text", text: USER }] },
            ],
          },
        ),
      ],
    });
  }
  return cases;
}

// ---------------------------------------------------------------- normalization

const TS_KEYS: Record<string, true> = {
  created: true,
  created_at: true,
  completed_at: true,
  started_at: true,
  duration: true,
};
// Deterministic ids emitted verbatim by the mock: kept so a pass-through vs.
// regenerated difference stays visible.
const STABLE_IDS: Record<string, true> = {
  rs_mock_1: true,
  msg_mock_1: true,
  call_weather_1: true,
  call_websearch_1: true,
};
const ID_RE =
  /\b(?:resp|msg|chatcmpl|rs|fc|call|toolu|req)[_-][A-Za-z0-9_-]{3,}|\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;

function makeNormalizer() {
  const map = new Map<string, string>();
  const counters = new Map<string, number>();
  const idFor = (raw: string) => {
    if (STABLE_IDS[raw]) return raw;
    let v = map.get(raw);
    if (!v) {
      const prefix = /^[0-9a-f]{8}-/i.test(raw)
        ? "uuid"
        : raw.match(/^[a-z]+/i)![0].toLowerCase();
      const n = (counters.get(prefix) ?? 0) + 1;
      counters.set(prefix, n);
      v = `<${prefix}#${n}>`;
      map.set(raw, v);
    }
    return v;
  };
  const walk = (value: unknown, key?: string): unknown => {
    if (key && TS_KEYS[key] && typeof value === "number") return "<ts>";
    if (typeof value === "string") return value.replace(ID_RE, (m) => idFor(m));
    if (Array.isArray(value)) return value.map((v) => walk(v));
    if (value && typeof value === "object") {
      const out: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(value)) out[k] = walk(v, k);
      return out;
    }
    return value;
  };
  return walk;
}

function normalizeSteps(steps: Step[]): Step[] {
  const walk = makeNormalizer();
  return walk(steps) as Step[];
}

// ---------------------------------------------------------------- diffing

type Diff = { where: string; kind: string; base?: unknown; new?: unknown };

function short(v: unknown): string {
  const s = JSON.stringify(v);
  if (s === undefined) return "<absent>";
  return s.length > 220 ? s.slice(0, 217) + "..." : s;
}

function deepDiff(a: unknown, b: unknown, path: string, out: Diff[]) {
  if (a === b) return;
  const ta = Array.isArray(a) ? "array" : a === null ? "null" : typeof a;
  const tb = Array.isArray(b) ? "array" : b === null ? "null" : typeof b;
  if (ta !== tb) {
    out.push({ where: path, kind: "value", base: a, new: b });
    return;
  }
  if (ta === "array") {
    const aa = a as unknown[];
    const bb = b as unknown[];
    const n = Math.max(aa.length, bb.length);
    for (let i = 0; i < n; i++) {
      if (i >= aa.length)
        out.push({ where: `${path}[${i}]`, kind: "added", new: bb[i] });
      else if (i >= bb.length)
        out.push({ where: `${path}[${i}]`, kind: "removed", base: aa[i] });
      else deepDiff(aa[i], bb[i], `${path}[${i}]`, out);
    }
    return;
  }
  if (ta === "object") {
    const ao = a as Record<string, unknown>;
    const bo = b as Record<string, unknown>;
    const ak = Object.keys(ao);
    const bk = Object.keys(bo);
    for (const k of ak) {
      if (!(k in bo))
        out.push({ where: `${path}.${k}`, kind: "removed", base: ao[k] });
      else deepDiff(ao[k], bo[k], `${path}.${k}`, out);
    }
    for (const k of bk)
      if (!(k in ao))
        out.push({ where: `${path}.${k}`, kind: "added", new: bo[k] });
    const common = ak.filter((k) => k in bo);
    const commonB = bk.filter((k) => k in ao);
    if (common.join("\u0000") !== commonB.join("\u0000")) {
      out.push({
        where: path || ".",
        kind: "key-order",
        base: common,
        new: commonB,
      });
    }
    return;
  }
  out.push({ where: path, kind: "value", base: a, new: b });
}

function frameSig(f: Frame): string {
  const d = f.data;
  const parts = [
    f.event ?? "",
    typeof d === "string" ? d : digString(d, "type"),
  ];
  const delta = dig(d, "choices", 0, "delta");
  if (delta && typeof delta === "object")
    parts.push(Object.keys(delta).sort().join(","));
  const finish = digString(d, "choices", 0, "finish_reason");
  if (finish) parts.push(`finish=${finish}`);
  const deltaType = digString(d, "delta", "type");
  if (deltaType) parts.push(deltaType);
  const itemType = digString(d, "item", "type");
  if (itemType) parts.push(`item=${itemType}`);
  const blockType = digString(d, "content_block", "type");
  if (blockType) parts.push(`block=${blockType}`);
  return parts.join("|");
}

function frameLabel(f: Frame): string {
  const d = f.data;
  return (
    f.event ??
    (typeof d === "string"
      ? d
      : digString(d, "type") || digString(d, "object") || "data")
  );
}

// LCS alignment of frame signatures, then field diffs for aligned pairs.
function diffFrames(a: Frame[], b: Frame[], prefix: string, out: Diff[]) {
  const sa = a.map(frameSig);
  const sb = b.map(frameSig);
  const n = sa.length;
  const m = sb.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () =>
    new Array(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i][j] =
        sa[i] === sb[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && sa[i] === sb[j]) {
      deepDiff(
        a[i],
        b[j],
        `${prefix}frames[base ${i}/new ${j} ${frameLabel(a[i])}]`,
        out,
      );
      i++;
      j++;
    } else if (j < m && (i >= n || dp[i][j + 1] >= dp[i + 1][j])) {
      out.push({
        where: `${prefix}frames[new ${j} ${frameLabel(b[j])}]`,
        kind: "frame-added",
        new: b[j],
      });
      j++;
    } else {
      out.push({
        where: `${prefix}frames[base ${i} ${frameLabel(a[i])}]`,
        kind: "frame-removed",
        base: a[i],
      });
      i++;
    }
  }
}

function diffCase(a: Step[], b: Step[]): Diff[] {
  const out: Diff[] = [];
  const n = Math.max(a.length, b.length);
  for (let k = 0; k < n; k++) {
    const sa = a[k];
    const sb = b[k];
    const p = `${sa?.label ?? sb?.label}:`;
    if (!sa || !sb) {
      out.push({
        where: `${p}step`,
        kind: sa ? "removed" : "added",
        base: sa?.label,
        new: sb?.label,
      });
      continue;
    }
    deepDiff(sa.request, sb.request, `${p}request`, out);
    for (const f of [
      "status",
      "contentType",
      "kind",
      "error",
      "droppedComments",
    ] as const) {
      if (sa[f] !== sb[f])
        out.push({ where: `${p}${f}`, kind: "value", base: sa[f], new: sb[f] });
    }
    if (sa.kind === "sse" && sb.kind === "sse")
      diffFrames(sa.frames ?? [], sb.frames ?? [], p, out);
    else deepDiff(sa.body ?? sa.frames, sb.body ?? sb.frames, `${p}body`, out);
  }
  return out;
}

// Usage extraction for the report (as returned, after normalization).
function usageOf(steps: Step[]): unknown[] {
  const found: unknown[] = [];
  const visit = (v: unknown) => {
    if (Array.isArray(v)) v.forEach(visit);
    else if (v && typeof v === "object") {
      for (const [k, x] of Object.entries(v)) {
        if (
          (k === "usage" || k === "input_tokens") &&
          x !== null &&
          x !== undefined
        )
          found.push(k === "usage" ? x : { input_tokens: x });
        else visit(x);
      }
    }
  };
  for (const s of steps) visit(s.kind === "sse" ? s.frames : s.body);
  return found;
}

type Outcome = {
  label: string;
  status: number;
  result: "success" | "error";
  reason?: string;
};

function outcomesOf(steps: Step[]): Outcome[] {
  return steps.map((step) => {
    let reason = step.error;
    if (!reason && (step.status < 200 || step.status >= 300))
      reason = `HTTP ${step.status}`;
    if (!reason && dig(step.body, "error"))
      reason = short(dig(step.body, "error"));
    if (!reason && step.kind === "sse") {
      const frames = step.frames ?? [];
      const error = frames.find(
        (frame) =>
          ["error", "response.failed", "response.incomplete"].includes(
            frame.event ?? "",
          ) ||
          ["error", "response.failed", "response.incomplete"].includes(
            digString(frame.data, "type"),
          ) ||
          dig(frame.data, "error"),
      );
      if (error) reason = short(error.data);
      else if (step.request.path === PATHS.responses) {
        if (
          !frames.some(
            (frame) =>
              digString(frame.data, "type") === "response.completed" &&
              digString(frame.data, "response", "status") === "completed",
          )
        ) {
          reason = "missing completed Responses terminal";
        }
      } else if (step.request.path === PATHS.chat) {
        if (
          !frames.some((frame) =>
            digString(frame.data, "choices", 0, "finish_reason"),
          )
        )
          reason = "missing Chat finish_reason";
      } else if (step.request.path === PATHS.messages) {
        if (
          !frames.some(
            (frame) => digString(frame.data, "type") === "message_stop",
          ) ||
          !frames.some((frame) => digString(frame.data, "delta", "stop_reason"))
        )
          reason = "missing Messages terminal";
      }
    }
    return {
      label: step.label,
      status: step.status,
      result: reason ? "error" : "success",
      ...(reason ? { reason } : {}),
    };
  });
}

type Expected = { case: RegExp; where: RegExp };
function loadExpected(): Expected[] {
  const raw: unknown = JSON.parse(readFileSync(args.expected, "utf8"));
  if (!Array.isArray(raw))
    throw new Error("expected-diffs file must contain an array");
  return raw.map((entry: unknown) => {
    if (
      !entry ||
      typeof entry !== "object" ||
      !("case" in entry) ||
      !("where" in entry) ||
      typeof entry.case !== "string" ||
      typeof entry.where !== "string" ||
      ("note" in entry && typeof entry.note !== "string")
    ) {
      throw new Error(
        "expected-diffs entries require string case, where, and optional note fields",
      );
    }
    return { case: new RegExp(entry.case), where: new RegExp(entry.where) };
  });
}

type CaseResult = {
  name: string;
  identical: boolean;
  diffs: Diff[];
  unexpected: Diff[];
  outcomes: { base: Outcome[]; new: Outcome[] };
  usage: { base: unknown[]; new: unknown[] };
};

// ---------------------------------------------------------------- main

async function main() {
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const outDir = resolve(args.out ?? join(SMOKE, "out", stamp));
  const outRelative = relative(join(SMOKE, "out"), outDir);
  if (
    !outRelative ||
    outRelative === ".." ||
    outRelative.startsWith("../") ||
    isAbsolute(outRelative)
  ) {
    throw new Error("--out must name a directory below artifacts/smoke/out/");
  }
  if (existsSync(outDir) && readdirSync(outDir).length > 0)
    throw new Error(`output directory is not empty: ${outDir}`);
  const expected = loadExpected();
  const only = args.only ? new RegExp(args.only) : null;
  const cases = buildMatrix().filter((c) => !only || only.test(c.name));
  if (!cases.length) throw new Error("--only selected no cases");
  mkdirSync(outDir, { recursive: true });

  if (args.rebootstrap || !existsSync(SEED_DB) || !existsSync(SEED_META))
    await bootstrap();
  const meta: SeedMeta = JSON.parse(readFileSync(SEED_META, "utf8"));

  const [mockPort, basePort, newPort] = [
    await freePort(),
    await freePort(),
    await freePort(),
  ];
  console.log(
    `[run] out=${outDir} mock=${mockPort} base=${basePort} new=${newPort}`,
  );
  mkdirSync(join(outDir, "logs"), { recursive: true });
  await startMock(mockPort, join(outDir, "logs/mock.log"));
  prepareRunDb(join(outDir, "base/db/monoize.db"), mockPort);
  prepareRunDb(join(outDir, "new/db/monoize.db"), mockPort);
  await startServer(
    "base",
    BASE_BIN,
    basePort,
    join(outDir, "base/db/monoize.db"),
    join(outDir, "base/work"),
  );
  await startServer(
    "new",
    NEW_BIN,
    newPort,
    join(outDir, "new/db/monoize.db"),
    join(outDir, "new/work"),
  );
  writeFileSync(
    join(outDir, "run.json"),
    JSON.stringify(
      {
        base: BASE_BIN,
        new: NEW_BIN,
        seed: SEED_DB,
        mockPort,
        basePort,
        newPort,
        pids: [...procs].map((p) => ({ name: p.name, pid: p.proc.pid })),
      },
      null,
      2,
    ),
  );

  const results: CaseResult[] = [];
  for (const c of cases) {
    const [rawA, rawB] = await Promise.all([
      c.run({ port: basePort, apiKey: meta.apiKey }),
      c.run({ port: newPort, apiKey: meta.apiKey }),
    ]);
    const a = normalizeSteps(rawA);
    const b = normalizeSteps(rawB);
    const file = c.name.replace(/[^A-Za-z0-9._-]+/g, "__") + ".json";
    for (const [side, raw, norm] of [
      ["base", rawA, a],
      ["new", rawB, b],
    ] as const) {
      mkdirSync(join(outDir, side, "raw"), { recursive: true });
      mkdirSync(join(outDir, side, "normalized"), { recursive: true });
      writeFileSync(
        join(outDir, side, "raw", file),
        JSON.stringify(raw, null, 2),
      );
      writeFileSync(
        join(outDir, side, "normalized", file),
        JSON.stringify(norm, null, 2),
      );
    }
    const diffs = diffCase(a, b);
    const unexpected = diffs.filter(
      (d) =>
        !expected.some((e) => e.case.test(c.name) && e.where.test(d.where)),
    );
    results.push({
      name: c.name,
      identical: diffs.length === 0,
      diffs,
      unexpected,
      outcomes: { base: outcomesOf(a), new: outcomesOf(b) },
      usage: { base: usageOf(a), new: usageOf(b) },
    });
    console.log(
      `${diffs.length === 0 ? "SAME" : unexpected.length ? "DIFF" : "EXPD"}  ${c.name}${diffs.length ? `  (${diffs.length} diffs)` : ""}`,
    );
  }

  return writeReports(outDir, results);
}

function writeReports(outDir: string, results: CaseResult[]): number {
  const identical = results.filter((r) => r.identical).length;
  const withUnexpected = results.filter((r) => r.unexpected.length > 0);
  const lines: string[] = [
    `# Monoize differential smoke`,
    ``,
    `- base: \`${BASE_BIN}\``,
    `- new: \`${NEW_BIN}\``,
    `- seed: \`${SEED_DB}\``,
    `- cases: ${results.length}, identical: ${identical}, differing: ${results.length - identical}, with unexpected diffs: ${withUnexpected.length}`,
    ``,
    `## Cases`,
    ``,
    `| case | comparison | base status | new status | base outcome | new outcome | base usage | new usage |`,
    `|---|---|---|---|---|---|---|---|`,
  ];
  for (const r of results) {
    lines.push(
      `| ${r.name} | ${r.identical ? "identical" : `${r.diffs.length} diffs`} | ${r.outcomes.base.map((s) => s.status).join(",")} | ${r.outcomes.new.map((s) => s.status).join(",")} | ${r.outcomes.base.map((s) => s.result + (s.reason ? `: ${s.reason.replaceAll("|", "\\|")}` : "")).join("; ")} | ${r.outcomes.new.map((s) => s.result + (s.reason ? `: ${s.reason.replaceAll("|", "\\|")}` : "")).join("; ")} | \`${short(r.usage.base.at(-1) ?? null)}\` | \`${short(r.usage.new.at(-1) ?? null)}\` |`,
    );
  }
  lines.push("", "## Differences", "");
  for (const r of results.filter((x) => !x.identical)) {
    lines.push(`### ${r.name}`, "");
    for (const d of r.diffs) {
      const flag = r.unexpected.includes(d) ? "" : " (expected)";
      lines.push(
        `- \`${d.where}\` ${d.kind}${flag}: base=\`${short(d.base)}\` new=\`${short(d.new)}\``,
      );
    }
    lines.push("");
  }
  writeFileSync(join(outDir, "report.md"), lines.join("\n"));
  writeFileSync(
    join(outDir, "report.json"),
    JSON.stringify(
      {
        base: BASE_BIN,
        new: NEW_BIN,
        total: results.length,
        identical,
        results,
      },
      null,
      2,
    ),
  );
  console.log(
    `\ncases=${results.length} identical=${identical} differing=${results.length - identical} unexpected=${withUnexpected.length}`,
  );
  console.log(`report: ${join(outDir, "report.md")}`);
  return withUnexpected.length > 0 ? 1 : 0;
}

let code = 1;
try {
  code = await main();
} catch (e) {
  console.error(e);
  code = 2;
} finally {
  await Promise.all([...procs].map(stopProc));
}
process.exit(code);
