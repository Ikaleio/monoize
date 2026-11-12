#!/usr/bin/env bun
// Differential smoke harness (spec/differential-smoke.spec.md): runs two
// Monoize binaries against the repository mock upstream on copies of one seed
// database, replays a fixed request matrix, normalizes volatile fields, and
// diffs the downstream responses.
import { Database } from "bun:sqlite";
import { parseArgs } from "node:util";
import {
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "../..");
const MOCK_DIR = join(ROOT, "mock");
const OUT_ROOT = join(ROOT, "artifacts/smoke/out");
const SEED_DB = join(ROOT, "artifacts/smoke/seed/monoize.db");
const SEED_META = join(dirname(SEED_DB), "meta.json");
const USAGE =
  "usage: bun run scripts/differential-smoke/run.ts --base <bin> --new <bin> [--only regex] [--out directory] [--rebootstrap] [--expected json]";

// ---------------------------------------------------------------- JSON helpers

type Obj = Record<string, unknown>;

function dig(value: unknown, ...path: (string | number)[]): unknown {
  for (const key of path) {
    if (value === null || typeof value !== "object") return undefined;
    value = (value as Obj)[key];
  }
  return value;
}
function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}
function objects(value: unknown): Obj[] {
  return Array.isArray(value)
    ? value.filter((x) => x && typeof x === "object" && !Array.isArray(x))
    : [];
}
function short(value: unknown): string {
  const s = JSON.stringify(value);
  if (s === undefined) return "<absent>";
  return s.length > 220 ? s.slice(0, 217) + "..." : s;
}

// ---------------------------------------------------------------- processes

type Child = { name: string; proc: Bun.Subprocess };
const children = new Set<Child>();

async function stop(child: Child) {
  child.proc.kill("SIGTERM");
  const timer = setTimeout(() => child.proc.kill("SIGKILL"), 5000);
  await child.proc.exited;
  clearTimeout(timer);
  children.delete(child);
}

for (const [signal, code] of [
  ["SIGINT", 130],
  ["SIGTERM", 143],
] as const) {
  process.on(signal, async () => {
    await Promise.all([...children].map(stop));
    process.exit(code);
  });
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

async function waitHttp(url: string) {
  const deadline = Date.now() + 60000;
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

// Children talk to loopback only, so proxy settings are stripped to keep a
// local proxy out of the request path.
async function spawnChild(
  name: string,
  command: string[],
  cwd: string,
  env: Record<string, string>,
  logPath: string,
  readyUrl: string,
) {
  const childEnv: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env))
    if (v !== undefined && !PROXY_VARS.includes(k)) childEnv[k] = v;
  Object.assign(childEnv, { NO_PROXY: "*", no_proxy: "*" }, env);
  const log = openSync(logPath, "w");
  const proc = Bun.spawn(command, {
    cwd,
    env: childEnv,
    stdout: log,
    stderr: log,
  });
  closeSync(log);
  const child = { name, proc };
  children.add(child);
  await waitHttp(readyUrl);
  return child;
}

function startServer(
  name: string,
  bin: string,
  port: number,
  dbPath: string,
  workDir: string,
) {
  mkdirSync(workDir, { recursive: true });
  return spawnChild(
    name,
    [bin],
    workDir,
    {
      MONOIZE_LISTEN: `127.0.0.1:${port}`,
      MONOIZE_DATABASE_DSN: `sqlite://${dbPath}?mode=rwc`,
      MONOIZE_REQUEST_LOG_SPOOL_DIR: join(workDir, "spool"),
      MONOIZE_IMAGE_TRANSFORM_CACHE_DIR: join(workDir, "image-cache"),
      RUST_LOG: "info",
    },
    join(workDir, "server.log"),
    `http://127.0.0.1:${port}/api/dashboard/settings/public`,
  );
}

// ---------------------------------------------------------------- seed

const UPSTREAMS = [
  { type: "responses", models: ["up-responses", "up-responses-reasoning"] },
  { type: "chat_completion", models: ["up-chat", "up-chat-reasoning"] },
  { type: "messages", models: ["up-messages", "up-messages-reasoning"] },
] as const;

type SeedMeta = { apiKey: string; username: string; createdWith: string };

async function dashboardApi(
  port: number,
  path: string,
  body: unknown,
  token?: string,
) {
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`POST ${path} -> ${res.status}: ${text}`);
  return JSON.parse(text);
}

async function bootstrap(baseBin: string) {
  const seedDir = dirname(SEED_DB);
  rmSync(seedDir, { recursive: true, force: true });
  mkdirSync(seedDir, { recursive: true });
  const port = await freePort();
  console.log(`[bootstrap] starting base binary on ${port}`);
  const server = await startServer(
    "bootstrap",
    baseBin,
    port,
    SEED_DB,
    join(seedDir, "bootstrap-work"),
  );
  try {
    // Settings are read from the DB per call, so direct writes apply immediately.
    const db = new Database(SEED_DB);
    const upsert = db.prepare(
      "INSERT INTO system_settings (key, value, updated_at) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    );
    const now = new Date().toISOString();
    for (const key of [
      "captcha_enabled",
      "price_sync_auto_enabled",
      "monoize_active_probe_enabled",
    ])
      upsert.run(key, "false", now);
    db.close();

    const username = "smoke_admin";
    const { token } = await dashboardApi(port, "/api/dashboard/auth/register", {
      username,
      password: "smoke-password-1",
    });
    for (const up of UPSTREAMS) {
      await dashboardApi(
        port,
        "/api/dashboard/providers",
        {
          name: `mock-${up.type}`,
          channels: [
            {
              name: `mock-${up.type}`,
              provider_type: up.type,
              // Rewritten to the live mock address in each run database.
              base_url: "http://127.0.0.1:1",
              api_key: "sk-mock-not-a-real-key",
              models: Object.fromEntries(
                up.models.map((m) => [m, { redirect: null, multiplier: "1" }]),
              ),
            },
          ],
          circuit_breaker_enabled: false,
          allow_free_when_unpriced_override: true,
          allow_free_when_missing_usage_override: true,
        },
        token,
      );
    }
    const key = await dashboardApi(
      port,
      "/api/dashboard/tokens",
      { name: "smoke" },
      token,
    );
    const meta: SeedMeta = { apiKey: key.key, username, createdWith: baseBin };
    writeFileSync(SEED_META, JSON.stringify(meta, null, 2));
  } finally {
    await stop(server);
    const db = new Database(SEED_DB);
    db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    db.close();
    for (const s of ["-wal", "-shm"]) rmSync(SEED_DB + s, { force: true });
  }
}

function prepareRunDb(dst: string, mockPort: number) {
  mkdirSync(dirname(dst), { recursive: true });
  copyFileSync(SEED_DB, dst);
  const db = new Database(dst);
  db.prepare("UPDATE monoize_channels SET base_url = ?").run(
    `http://127.0.0.1:${mockPort}`,
  );
  db.close();
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
    const lines = block.split(/\r?\n/).filter((l) => !l.startsWith(":"));
    if (!lines.length) {
      comments++;
      continue;
    }
    const frame: Frame = { data: undefined };
    const data: string[] = [];
    for (const line of lines) {
      const i = line.indexOf(":");
      const field = i < 0 ? line : line.slice(0, i);
      let value = i < 0 ? "" : line.slice(i + 1);
      if (value.startsWith(" ")) value = value.slice(1);
      if (field === "event") frame.event = value;
      else if (field === "id") frame.id = value;
      else if (field === "data") data.push(value);
    }
    const raw = data.join("\n");
    try {
      frame.data = JSON.parse(raw);
    } catch {
      frame.data = raw;
    }
    frames.push(frame);
  }
  return { frames, comments };
}

type Ctx = { port: number; apiKey: string };

async function send(
  ctx: Ctx,
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
  const headers: Record<string, string> = {
    "content-type": "application/json",
    authorization: `Bearer ${ctx.apiKey}`,
  };
  if (path.startsWith("/v1/messages")) {
    headers["x-api-key"] = ctx.apiKey;
    headers["anthropic-version"] = "2023-06-01";
  }
  try {
    const res = await fetch(`http://127.0.0.1:${ctx.port}${path}`, {
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

type Proto = "responses" | "chat" | "messages";
type ToolCall = { id: string; name: string };
type Dialect = {
  path: string;
  user(text: string): Obj;
  tool(name: string, schema: Obj): Obj;
  // Single-turn matrix request with a system prompt.
  single(model: string, stream: boolean): Obj;
  // Conversation request: `history` plus extra fields such as tools and stream.
  converse(model: string, history: Obj[], extra: Obj): Obj;
  // Assistant items to replay from a reply, and the tool calls it requests.
  reply(step: Step): { assistant: Obj[]; calls: ToolCall[] };
  toolResults(calls: ToolCall[]): Obj[];
};

const SYSTEM = "Be brief.";
const USER = "Hello from smoke";
const TOOL_PROMPT = "Weather in Taipei, then search Monoize.";
const TOOLS: Record<string, { result: string; schema: Obj }> = {
  weather: {
    result: "sunny 25C",
    schema: {
      type: "object",
      properties: { city: { type: "string" } },
      required: ["city"],
    },
  },
  websearch: {
    result: "Monoize is an LLM gateway",
    schema: {
      type: "object",
      properties: { query: { type: "string" } },
      required: ["query"],
    },
  },
};
const toolResult = (name: string) => TOOLS[name]?.result ?? "unknown";

const DIALECTS: Record<Proto, Dialect> = {
  responses: {
    path: "/v1/responses",
    user: (text) => ({
      type: "message",
      role: "user",
      content: [{ type: "input_text", text }],
    }),
    tool: (name, parameters) => ({ type: "function", name, parameters }),
    single: (model, stream) => ({
      model,
      instructions: SYSTEM,
      input: [DIALECTS.responses.user(USER)],
      max_output_tokens: 256,
      stream,
    }),
    converse: (model, input, extra) => ({ model, input, ...extra }),
    reply(step) {
      const completed = step.frames?.find(
        (f) => dig(f.data, "type") === "response.completed",
      );
      const output = objects(
        step.kind === "sse"
          ? dig(completed?.data, "response", "output")
          : dig(step.body, "output"),
      );
      return {
        assistant: output,
        calls: output
          .filter((o) => o.type === "function_call")
          .map((o) => ({ id: str(o.call_id), name: str(o.name) })),
      };
    },
    toolResults: (calls) =>
      calls.map((c) => ({
        type: "function_call_output",
        call_id: c.id,
        output: toolResult(c.name),
      })),
  },
  chat: {
    path: "/v1/chat/completions",
    user: (text) => ({ role: "user", content: text }),
    tool: (name, parameters) => ({
      type: "function",
      function: { name, parameters },
    }),
    single: (model, stream) => ({
      model,
      messages: [{ role: "system", content: SYSTEM }, DIALECTS.chat.user(USER)],
      max_tokens: 256,
      stream,
      ...(stream ? { stream_options: { include_usage: true } } : {}),
    }),
    converse: (model, messages, extra) => ({ model, messages, ...extra }),
    reply(step) {
      const message = objects([dig(step.body, "choices", 0, "message")]);
      return {
        assistant: message,
        calls: objects(message[0]?.tool_calls).map((c) => ({
          id: str(c.id),
          name: str(dig(c, "function", "name")),
        })),
      };
    },
    toolResults: (calls) =>
      calls.map((c) => ({
        role: "tool",
        tool_call_id: c.id,
        content: toolResult(c.name),
      })),
  },
  messages: {
    path: "/v1/messages",
    user: (text) => ({ role: "user", content: [{ type: "text", text }] }),
    tool: (name, input_schema) => ({ name, input_schema }),
    single: (model, stream) => ({
      model,
      system: SYSTEM,
      messages: [DIALECTS.messages.user(USER)],
      max_tokens: 256,
      stream,
    }),
    converse: (model, messages, extra) => ({
      model,
      max_tokens: 256,
      messages,
      ...extra,
    }),
    reply(step) {
      const content = objects(dig(step.body, "content"));
      return {
        assistant: content.length ? [{ role: "assistant", content }] : [],
        calls: content
          .filter((b) => b.type === "tool_use")
          .map((b) => ({ id: str(b.id), name: str(b.name) })),
      };
    },
    toolResults: (calls) => [
      {
        role: "user",
        content: calls.map((c) => ({
          type: "tool_result",
          tool_use_id: c.id,
          content: toolResult(c.name),
        })),
      },
    ],
  },
};

type CaseDef = { name: string; run: (ctx: Ctx) => Promise<Step[]> };

// Weather call, websearch call, final answer; stops early when no tool call comes back.
function toolLoopCase(proto: Proto): CaseDef {
  const d = DIALECTS[proto];
  const tools = Object.entries(TOOLS).map(([name, t]) =>
    d.tool(name, t.schema),
  );
  return {
    name: `toolloop/${proto}->chat_completion`,
    async run(ctx) {
      const history = [d.user(TOOL_PROMPT)];
      const steps: Step[] = [];
      for (let turn = 1; turn <= 3; turn++) {
        const step = await send(
          ctx,
          `turn${turn}`,
          d.path,
          d.converse("up-chat", structuredClone(history), {
            tools,
            stream: false,
          }),
        );
        steps.push(step);
        const { assistant, calls } = d.reply(step);
        if (!calls.length) break;
        history.push(...assistant, ...d.toolResults(calls));
      }
      return steps;
    },
  };
}

// Two turns; the first reply's assistant items are replayed into the second.
function multiTurnCase(
  proto: Proto,
  upstream: string,
  model: string,
  stream: boolean,
): CaseDef {
  const d = DIALECTS[proto];
  const variant = model.endsWith("-reasoning") ? "reasoning" : "plain";
  return {
    name: `multiturn/${proto}->${upstream}/${variant}/${stream ? "stream" : "nonstream"}`,
    async run(ctx) {
      const history = [d.user("First question")];
      const turn = (label: string) =>
        send(
          ctx,
          label,
          d.path,
          d.converse(model, structuredClone(history), { stream }),
        );
      const first = await turn("turn1");
      history.push(...d.reply(first).assistant, d.user("Second question"));
      return [first, await turn("turn2")];
    },
  };
}

function buildMatrix(): CaseDef[] {
  const cases: CaseDef[] = [];
  for (const proto of ["responses", "chat", "messages"] as const) {
    for (const up of UPSTREAMS) {
      for (const model of up.models) {
        for (const stream of [false, true]) {
          const variant = model.endsWith("-reasoning") ? "reasoning" : "plain";
          const d = DIALECTS[proto];
          cases.push({
            name: `${proto}->${up.type}/${variant}/${stream ? "stream" : "nonstream"}`,
            run: async (ctx) => [
              await send(ctx, "request", d.path, d.single(model, stream)),
            ],
          });
        }
      }
    }
  }
  for (const proto of ["responses", "chat", "messages"] as const)
    cases.push(toolLoopCase(proto));
  cases.push(
    multiTurnCase("responses", "responses", "up-responses-reasoning", true),
    multiTurnCase("chat", "messages", "up-messages", false),
    multiTurnCase("messages", "chat_completion", "up-chat-reasoning", false),
  );
  for (const up of UPSTREAMS) {
    cases.push({
      name: `count_tokens/messages->${up.type}`,
      run: async (ctx) => [
        await send(ctx, "request", "/v1/messages/count_tokens", {
          model: up.models[0],
          system: SYSTEM,
          messages: [DIALECTS.messages.user(USER)],
        }),
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

// One placeholder map per case and side, shared by all of its steps.
function normalize(steps: Step[]): Step[] {
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
    if (typeof value === "string") return value.replace(ID_RE, idFor);
    if (Array.isArray(value)) return value.map((v) => walk(v));
    if (value && typeof value === "object") {
      const out: Obj = {};
      for (const [k, v] of Object.entries(value)) out[k] = walk(v, k);
      return out;
    }
    return value;
  };
  return walk(steps) as Step[];
}

// ---------------------------------------------------------------- diffing

type Diff = { where: string; kind: string; base?: unknown; new?: unknown };

function deepDiff(a: unknown, b: unknown, path: string, out: Diff[]) {
  if (a === b) return;
  const [type, typeB] = [a, b].map((v) =>
    Array.isArray(v) ? "array" : v === null ? "null" : typeof v,
  );
  if (type !== typeB || (type !== "array" && type !== "object")) {
    out.push({ where: path, kind: "value", base: a, new: b });
    return;
  }
  if (type === "array") {
    const aa = a as unknown[];
    const bb = b as unknown[];
    for (let i = 0; i < Math.max(aa.length, bb.length); i++) {
      const at = `${path}[${i}]`;
      if (i >= aa.length) out.push({ where: at, kind: "added", new: bb[i] });
      else if (i >= bb.length)
        out.push({ where: at, kind: "removed", base: aa[i] });
      else deepDiff(aa[i], bb[i], at, out);
    }
    return;
  }
  const ao = a as Obj;
  const bo = b as Obj;
  for (const k of Object.keys(ao)) {
    if (!(k in bo))
      out.push({ where: `${path}.${k}`, kind: "removed", base: ao[k] });
    else deepDiff(ao[k], bo[k], `${path}.${k}`, out);
  }
  for (const k of Object.keys(bo))
    if (!(k in ao))
      out.push({ where: `${path}.${k}`, kind: "added", new: bo[k] });
  const commonA = Object.keys(ao).filter((k) => k in bo);
  const commonB = Object.keys(bo).filter((k) => k in ao);
  if (commonA.join("\u0000") !== commonB.join("\u0000"))
    out.push({
      where: path || ".",
      kind: "key-order",
      base: commonA,
      new: commonB,
    });
}

function frameSig(f: Frame): string {
  const d = f.data;
  const parts = [
    f.event ?? "",
    typeof d === "string" ? d : str(dig(d, "type")),
  ];
  const delta = dig(d, "choices", 0, "delta");
  if (delta && typeof delta === "object")
    parts.push(Object.keys(delta).sort().join(","));
  const finish = str(dig(d, "choices", 0, "finish_reason"));
  if (finish) parts.push(`finish=${finish}`);
  const deltaType = str(dig(d, "delta", "type"));
  if (deltaType) parts.push(deltaType);
  const itemType = str(dig(d, "item", "type"));
  if (itemType) parts.push(`item=${itemType}`);
  const blockType = str(dig(d, "content_block", "type"));
  if (blockType) parts.push(`block=${blockType}`);
  return parts.join("|");
}

function frameLabel(f: Frame): string {
  const d = f.data;
  if (f.event !== undefined) return f.event;
  if (typeof d === "string") return d;
  return str(dig(d, "type")) || str(dig(d, "object")) || "data";
}

// LCS alignment of frame signatures, then field diffs for aligned pairs.
function diffFrames(a: Frame[], b: Frame[], prefix: string, out: Diff[]) {
  const sa = a.map(frameSig);
  const sb = b.map(frameSig);
  const n = sa.length;
  const m = sb.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () =>
    new Array(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      lcs[i][j] =
        sa[i] === sb[j]
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && sa[i] === sb[j]) {
      const where = `${prefix}frames[base ${i}/new ${j} ${frameLabel(a[i])}]`;
      deepDiff(a[i], b[j], where, out);
      i++;
      j++;
    } else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) {
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
  for (let k = 0; k < Math.max(a.length, b.length); k++) {
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
      diffFrames(sa.frames!, sb.frames!, p, out);
    else deepDiff(sa.body ?? sa.frames, sb.body ?? sb.frames, `${p}body`, out);
  }
  return out;
}

// ---------------------------------------------------------------- outcomes

function usageOf(steps: Step[]): unknown[] {
  const found: unknown[] = [];
  const visit = (v: unknown) => {
    if (Array.isArray(v)) v.forEach(visit);
    else if (v && typeof v === "object") {
      for (const [k, x] of Object.entries(v)) {
        if (k === "usage" && x != null) found.push(x);
        else if (k === "input_tokens" && x != null)
          found.push({ input_tokens: x });
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

const ERROR_EVENTS = ["error", "response.failed", "response.incomplete"];

// HTTP 200 alone is not success: a stream must also end with its protocol terminal.
function streamFailure(step: Step): string | undefined {
  const frames = step.frames!;
  const error = frames.find(
    (f) =>
      ERROR_EVENTS.includes(f.event ?? "") ||
      ERROR_EVENTS.includes(str(dig(f.data, "type"))) ||
      dig(f.data, "error"),
  );
  if (error) return short(error.data);
  const any = (pred: (data: unknown) => unknown) =>
    frames.some((f) => pred(f.data));
  switch (step.request.path) {
    case DIALECTS.responses.path:
      return any(
        (d) =>
          dig(d, "type") === "response.completed" &&
          dig(d, "response", "status") === "completed",
      )
        ? undefined
        : "missing completed Responses terminal";
    case DIALECTS.chat.path:
      return any((d) => str(dig(d, "choices", 0, "finish_reason")))
        ? undefined
        : "missing Chat finish_reason";
    case DIALECTS.messages.path:
      return any((d) => dig(d, "type") === "message_stop") &&
        any((d) => str(dig(d, "delta", "stop_reason")))
        ? undefined
        : "missing Messages terminal";
  }
}

function outcomeOf(step: Step): Outcome {
  let reason = step.error;
  if (!reason && (step.status < 200 || step.status >= 300))
    reason = `HTTP ${step.status}`;
  if (!reason && dig(step.body, "error"))
    reason = short(dig(step.body, "error"));
  if (!reason && step.kind === "sse") reason = streamFailure(step);
  return {
    label: step.label,
    status: step.status,
    result: reason ? "error" : "success",
    ...(reason ? { reason } : {}),
  };
}

// ---------------------------------------------------------------- main

type Expected = { case: RegExp; where: RegExp };

function loadExpected(path: string): Expected[] {
  const entries = JSON.parse(readFileSync(path, "utf8"));
  if (!Array.isArray(entries))
    throw new Error(`${path}: expected a JSON array`);
  // A missing pattern would become an empty regex and silently match every diff.
  return entries.map((e) => {
    if (typeof e?.case !== "string" || typeof e?.where !== "string")
      throw new Error(`${path}: every entry needs string case and where`);
    return { case: new RegExp(e.case), where: new RegExp(e.where) };
  });
}

type Config = {
  baseBin: string;
  newBin: string;
  outDir: string;
  rebootstrap: boolean;
  cases: CaseDef[];
  expected: Expected[];
};

// Every input error is reported here, before any server starts.
function parseCli(): Config {
  const { values } = parseArgs({
    options: {
      base: { type: "string" },
      new: { type: "string" },
      only: { type: "string" },
      out: { type: "string" },
      rebootstrap: { type: "boolean", default: false },
      expected: {
        type: "string",
        default: join(import.meta.dir, "expected-diffs.json"),
      },
    },
  });
  if (!values.base || !values.new) throw new Error(USAGE);
  const only = values.only ? new RegExp(values.only) : null;
  const cases = buildMatrix().filter((c) => !only || only.test(c.name));
  if (!cases.length) throw new Error("--only selected no cases");
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  return {
    baseBin: resolve(values.base),
    newBin: resolve(values.new),
    outDir: resolve(values.out ?? join(OUT_ROOT, stamp)),
    rebootstrap: values.rebootstrap,
    cases,
    expected: loadExpected(resolve(values.expected)),
  };
}

type CaseResult = {
  name: string;
  identical: boolean;
  diffs: Diff[];
  unexpected: Diff[];
  outcomes: { base: Outcome[]; new: Outcome[] };
  usage: { base: unknown[]; new: unknown[] };
};

async function main(): Promise<number> {
  const config = parseCli();
  const { outDir } = config;
  for (const dir of [
    "logs",
    "base/raw",
    "base/normalized",
    "new/raw",
    "new/normalized",
  ])
    mkdirSync(join(outDir, dir), { recursive: true });

  if (config.rebootstrap || !existsSync(SEED_DB) || !existsSync(SEED_META))
    await bootstrap(config.baseBin);
  const meta: SeedMeta = JSON.parse(readFileSync(SEED_META, "utf8"));

  const mockPort = await freePort();
  const basePort = await freePort();
  const newPort = await freePort();
  console.log(
    `[run] out=${outDir} mock=${mockPort} base=${basePort} new=${newPort}`,
  );
  await spawnChild(
    "mock",
    [process.execPath, "run", "server.ts"],
    MOCK_DIR,
    { PORT: String(mockPort) },
    join(outDir, "logs/mock.log"),
    `http://127.0.0.1:${mockPort}/health`,
  );
  for (const [side, bin, port] of [
    ["base", config.baseBin, basePort],
    ["new", config.newBin, newPort],
  ] as const) {
    const db = join(outDir, side, "db/monoize.db");
    prepareRunDb(db, mockPort);
    await startServer(side, bin, port, db, join(outDir, side, "work"));
  }
  writeFileSync(
    join(outDir, "run.json"),
    JSON.stringify(
      {
        base: config.baseBin,
        new: config.newBin,
        seed: SEED_DB,
        mockPort,
        basePort,
        newPort,
        pids: [...children].map((c) => ({ name: c.name, pid: c.proc.pid })),
      },
      null,
      2,
    ),
  );

  const results: CaseResult[] = [];
  for (const c of config.cases) {
    const [rawA, rawB] = await Promise.all([
      c.run({ port: basePort, apiKey: meta.apiKey }),
      c.run({ port: newPort, apiKey: meta.apiKey }),
    ]);
    const a = normalize(rawA);
    const b = normalize(rawB);
    const file = c.name.replace(/[^A-Za-z0-9._-]+/g, "__") + ".json";
    for (const [side, raw, norm] of [
      ["base", rawA, a],
      ["new", rawB, b],
    ] as const) {
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
        !config.expected.some(
          (e) => e.case.test(c.name) && e.where.test(d.where),
        ),
    );
    results.push({
      name: c.name,
      identical: diffs.length === 0,
      diffs,
      unexpected,
      outcomes: { base: a.map(outcomeOf), new: b.map(outcomeOf) },
      usage: { base: usageOf(a), new: usageOf(b) },
    });
    const tag = !diffs.length ? "SAME" : unexpected.length ? "DIFF" : "EXPD";
    console.log(
      `${tag}  ${c.name}${diffs.length ? `  (${diffs.length} diffs)` : ""}`,
    );
  }
  return writeReports(config, results);
}

function writeReports(config: Config, results: CaseResult[]): number {
  const identical = results.filter((r) => r.identical).length;
  const withUnexpected = results.filter((r) => r.unexpected.length > 0).length;
  const lines = [
    `# Monoize differential smoke`,
    ``,
    `- base: \`${config.baseBin}\``,
    `- new: \`${config.newBin}\``,
    `- seed: \`${SEED_DB}\``,
    `- cases: ${results.length}, identical: ${identical}, differing: ${results.length - identical}, with unexpected diffs: ${withUnexpected}`,
    ``,
    `## Cases`,
    ``,
    `| case | comparison | base status | new status | base outcome | new outcome | base usage | new usage |`,
    `|---|---|---|---|---|---|---|---|`,
  ];
  for (const r of results) {
    const outcomes = [r.outcomes.base, r.outcomes.new];
    const cells = [
      r.name,
      r.identical ? "identical" : `${r.diffs.length} diffs`,
      ...outcomes.map((side) => side.map((o) => o.status).join(",")),
      ...outcomes.map((side) =>
        side
          .map((o) => (o.reason ? `${o.result}: ${o.reason}` : o.result))
          .join("; ")
          .replaceAll("|", "\\|"),
      ),
      ...[r.usage.base, r.usage.new].map(
        (usage) => `\`${short(usage.at(-1) ?? null)}\``,
      ),
    ];
    lines.push(`| ${cells.join(" | ")} |`);
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
  writeFileSync(join(config.outDir, "report.md"), lines.join("\n"));
  writeFileSync(
    join(config.outDir, "report.json"),
    JSON.stringify(
      {
        base: config.baseBin,
        new: config.newBin,
        total: results.length,
        identical,
        results,
      },
      null,
      2,
    ),
  );
  console.log(
    `\ncases=${results.length} identical=${identical} differing=${results.length - identical} unexpected=${withUnexpected}`,
  );
  console.log(`report: ${join(config.outDir, "report.md")}`);
  return withUnexpected > 0 ? 1 : 0;
}

let code: number;
try {
  code = await main();
} catch (e) {
  console.error(e instanceof Error ? e.message : String(e));
  code = 2;
} finally {
  await Promise.all([...children].map(stop));
}
process.exit(code);
