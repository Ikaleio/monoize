import path from "node:path";
import { mkdirSync, appendFileSync, writeFileSync } from "node:fs";

export const HARNESSES = [
  "codex-responses-ws-v2",
  "claude-messages",
  "opencode-responses",
  "opencode-chat",
  "pi-responses",
  "pi-chat",
] as const;
export type Harness = (typeof HARNESSES)[number];
export type Protocol = "responses" | "chat_completion" | "messages";
export const ALIAS = "e2e-model";
export const BUDGET = 128 * 1024 * 1024;
export const ENDPOINTS: Record<Protocol, string> = {
  responses: "/v1/responses",
  chat_completion: "/v1/chat/completions",
  messages: "/v1/messages",
};
export function protocol(h: Harness): Protocol {
  return h === "claude-messages"
    ? "messages"
    : h.endsWith("-chat")
      ? "chat_completion"
      : "responses";
}
export interface Options {
  harnesses: Harness[];
  baseUrl: string;
  apiKey: string;
  model: string;
  upstreamType: Protocol;
  jobs: number;
  timeout: number;
  maxRequests: number;
  task?: string;
  repo: string;
  ref: string;
  monoizeImage?: string;
  output: string;
}
export async function options(
  args: string[],
  root: string,
  env: Record<string, string | undefined> = process.env,
): Promise<Options> {
  const allowed = new Set([
    "harness",
    "base-url",
    "api-key",
    "model",
    "upstream-type",
    "jobs",
    "timeout",
    "max-requests",
    "task",
    "task-file",
    "repo",
    "ref",
    "monoize-image",
    "output",
  ]);
  const values = new Map<string, string[]>();
  for (let i = 0; i < args.length; i++) {
    const match = /^--([^=]+)(?:=(.*))?$/.exec(args[i]!);
    if (!match || !allowed.has(match[1]!))
      throw new Error(`Unknown option: ${args[i]}`);
    const key = match[1]!;
    const value = match[2] ?? args[++i];
    if (value === undefined || value.startsWith("--") || !value.trim())
      throw new Error(`Missing value for --${key}`);
    if (key !== "harness" && values.has(key))
      throw new Error(`Repeated option: --${key}`);
    values.set(key, [...(values.get(key) ?? []), value]);
  }
  const get = (key: string, fallback?: string) =>
    values.get(key)?.[0] ?? fallback;
  const required = (key: string, name: string) => {
    const v = get(key, env[name]);
    if (!v?.trim()) throw new Error(`Missing --${key} or ${name}`);
    return v;
  };
  const positive = (key: string, fallback: number) => {
    const s = get(key, String(fallback))!;
    const n = Number(s);
    if (!/^\d+$/.test(s) || !Number.isSafeInteger(n) || n < 1)
      throw new Error(`--${key} must be a positive integer`);
    return n;
  };
  const baseUrl = required("base-url", "BASE_URL");
  let url: URL;
  try {
    url = new URL(baseUrl);
  } catch {
    throw new Error("BASE_URL must be an absolute HTTP(S) endpoint");
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.hash
  )
    throw new Error(
      "BASE_URL must use HTTP(S), without credentials or fragment",
    );
  const inferred = (Object.entries(ENDPOINTS) as [Protocol, string][]).find(
    ([, suffix]) => url.pathname.replace(/\/$/, "").endsWith(suffix.slice(3)),
  )?.[0];
  const upstreamType = get("upstream-type", env.UPSTREAM_TYPE) ?? inferred;
  if (!upstreamType || !(upstreamType in ENDPOINTS))
    throw new Error(
      "Specify --upstream-type responses, chat_completion, or messages",
    );
  if (inferred && inferred !== upstreamType)
    throw new Error("Upstream type conflicts with endpoint suffix");
  const selected = (values.get("harness") ?? ["all"]).flatMap((v) =>
    v.split(","),
  );
  const harnesses: Harness[] = [];
  for (const h of selected) {
    for (const id of h === "all" ? HARNESSES : [h]) {
      if (!(HARNESSES as readonly string[]).includes(id))
        throw new Error(`Unknown harness: ${id}`);
      if (!harnesses.includes(id as Harness)) harnesses.push(id as Harness);
    }
  }
  if (values.has("task") && values.has("task-file"))
    throw new Error("--task and --task-file are mutually exclusive");
  const task = values.has("task-file")
    ? await Bun.file(get("task-file")!).text()
    : get("task");
  if (task !== undefined && !task.trim())
    throw new Error("Task must not be empty");
  const output = path.resolve(
    root,
    get(
      "output",
      `e2e/.runs/${new Date().toISOString().replace(/[:.]/g, "-")}-${crypto.randomUUID().slice(0, 8)}`,
    )!,
  );
  if (!output.startsWith(root + path.sep))
    throw new Error("--output must be inside the project root");
  const repo = get("repo", "https://github.com/Ikaleio/Monoize")!;
  const repoURL = new URL(repo);
  if (
    repoURL.protocol !== "https:" ||
    repoURL.username ||
    repoURL.password ||
    repoURL.search ||
    repoURL.hash
  )
    throw new Error(
      "--repo must be a public HTTPS Git URL without credentials, query, or fragment",
    );
  return {
    harnesses,
    baseUrl,
    apiKey: required("api-key", "API_KEY"),
    model: required("model", "MODEL"),
    upstreamType: upstreamType as Protocol,
    jobs: positive("jobs", 1),
    timeout: positive("timeout", 1800),
    maxRequests: positive("max-requests", 100),
    task,
    repo,
    ref: get("ref", "HEAD")!,
    monoizeImage: get("monoize-image"),
    output,
  };
}

export function redact(value: unknown, secrets: string[] = []): any {
  if (typeof value === "string") {
    let s = value;
    for (const secret of secrets
      .filter(Boolean)
      .sort((a, b) => b.length - a.length))
      s = s.split(secret).join("[REDACTED]");
    s = s.replace(/\b(?:https?|wss?):\/\/[^\s<>"']+/g, (raw) => {
      try {
        const u = new URL(raw);
        if (u.username || u.password) {
          u.username = "REDACTED";
          u.password = "";
        }
        for (const k of [...u.searchParams.keys()])
          u.searchParams.set(k, "REDACTED");
        return u.toString();
      } catch {
        return "[REDACTED_URL]";
      }
    });
    return s.replace(
      /((?:authorization|x-api-key|api[_-]?key|cookie|password|secret|token)\s*[=:]\s*)(?:Bearer\s+)?[^\s,;]+/gi,
      "$1[REDACTED]",
    );
  }
  if (Array.isArray(value)) return value.map((v) => redact(v, secrets));
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([k, v]) => [
        k,
        /^(authorization|proxy-authorization|x-api-key|api[_-]?key|cookie|set-cookie|password|secret|access_token|refresh_token|token)$/i.test(
          k,
        )
          ? "[REDACTED]"
          : redact(v, secrets),
      ]),
    );
  return value;
}
export interface Evidence {
  seq: number;
  time: string;
  kind: string;
  [key: string]: any;
}
export class Recorder {
  bytes = 0;
  seq = 0;
  exceeded = false;
  constructor(
    public directory: string,
    public secrets: string[] = [],
    public limit = BUDGET,
  ) {
    mkdirSync(directory, { recursive: true });
  }
  add(kind: string, data: Record<string, unknown> = {}): void {
    if (this.exceeded) return;
    const line =
      JSON.stringify(
        redact(
          { seq: ++this.seq, time: new Date().toISOString(), kind, ...data },
          this.secrets,
        ),
      ) + "\n";
    if (this.bytes + Buffer.byteLength(line) > this.limit) {
      this.exceeded = true;
      writeFileSync(
        path.join(this.directory, "limit.json"),
        JSON.stringify({ category: "evidence_limit" }),
      );
      return;
    }
    this.bytes += Buffer.byteLength(line);
    appendFileSync(path.join(this.directory, "events.jsonl"), line, {
      mode: 0o600,
    });
  }
}

// TextDecoder retains partial UTF-8 code points; SSE boundaries need not match network chunks.
export class SSEParser {
  private decoder = new TextDecoder();
  private pending = "";
  constructor(
    private onEvent: (data: any) => void,
    private limit = BUDGET,
  ) {}
  feed(bytes: Uint8Array) {
    this.pending += this.decoder.decode(bytes, { stream: true });
    this.drain();
  }
  private drain() {
    if (Buffer.byteLength(this.pending) > this.limit)
      throw new Error("evidence_limit");
    let boundary: RegExpExecArray | null;
    while ((boundary = /\r?\n\r?\n/.exec(this.pending))) {
      const frame = this.pending.slice(0, boundary.index);
      this.pending = this.pending.slice(boundary.index + boundary[0].length);
      const data = frame
        .split(/\r?\n/)
        .filter((l) => l.startsWith("data:"))
        .map((l) => l.slice(5).replace(/^ /, ""))
        .join("\n");
      if (!data) continue;
      this.onEvent(data === "[DONE]" ? { type: "sse.done" } : JSON.parse(data));
    }
  }
  end() {
    this.pending += this.decoder.decode();
    this.drain();
    if (this.pending.trim() && !this.pending.trim().startsWith(":"))
      throw new Error("incomplete_sse_frame");
  }
}
export async function pool<T, R>(
  items: readonly T[],
  jobs: number,
  work: (item: T) => Promise<R>,
  signal?: AbortSignal,
): Promise<R[]> {
  const results: R[] = [];
  let index = 0;
  await Promise.all(
    Array.from({ length: Math.min(jobs, items.length) }, async () => {
      while (!signal?.aborted) {
        const i = index++;
        if (i >= items.length) break;
        results[i] = await work(items[i]!);
      }
    }),
  );
  return results;
}
export class Cleanup {
  private steps: (() => Promise<unknown>)[] = [];
  add(fn: () => Promise<unknown>) {
    this.steps.push(fn);
  }
  async run() {
    const errors: string[] = [];
    for (const fn of this.steps.splice(0).reverse()) {
      try {
        await fn();
      } catch (e) {
        errors.push(String(e));
      }
    }
    return errors;
  }
}
