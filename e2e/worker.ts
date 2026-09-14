import { Database } from "bun:sqlite";
import {
  readFile,
  writeFile,
  mkdir,
  readdir,
  stat,
  realpath,
  chown,
  chmod,
} from "node:fs/promises";
import path from "node:path";
import {
  ALIAS,
  BUDGET,
  Recorder,
  redact,
  type Harness,
  type Protocol,
} from "./core";
import { createProxy } from "./proxy";
import type { Snapshot } from "./validate";

if (process.argv[2] === "seed") {
  const db = new Database("/data/monoize.db");
  const insert = db.query(
    "INSERT INTO system_settings (key,value,updated_at) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
  );
  for (const key of [
    "captcha_enabled",
    "monoize_active_probe_enabled",
    "price_sync_auto_enabled",
  ])
    insert.run(key, "false", new Date().toISOString());
  db.close();
  process.exit(0);
}
interface Config {
  baseUrl: string;
  apiKey: string;
  model: string;
  upstreamType: Protocol;
  maxRequests: number;
  controlKey: string;
  password: string;
  harness: Harness;
  prompt: string;
  timeout: number;
}
const cfg: Config = JSON.parse(await readFile("/config.json", "utf8"));
const recorder = new Recorder("/evidence", [
  cfg.apiKey,
  cfg.controlKey,
  cfg.password,
]);
const base = "http://monoize:8080";
let key = "";
let token = "";
async function api(endpoint: string, method = "GET", body?: unknown) {
  const response = await fetch(`${base}/api/dashboard/${endpoint}`, {
    method,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30000),
  });
  if (!response.ok)
    throw new Error(
      `Dashboard ${method} ${endpoint}: ${response.status} ${await response.text()}`,
    );
  return response.json() as Promise<any>;
}
async function bootstrap() {
  const auth = await api("auth/register", "POST", {
    username: "e2e_admin",
    password: cfg.password,
  });
  token = auth.token;
  recorder.secrets.push(token);
  await api(`users/${auth.user.id}`, "PUT", { balance_unlimited: true });
  await api("settings", "PUT", {
    captcha_enabled: false,
    monoize_active_probe_enabled: false,
    price_sync_auto_enabled: false,
    monoize_request_capture_enabled: true,
    monoize_request_capture_max_total_bytes: BUDGET,
    global_transforms: [],
    global_model_redirects: [],
    reasoning_suffix_map: {},
    monoize_request_timeout_ms: cfg.timeout * 1000,
    monoize_stream_idle_timeout_ms: cfg.timeout * 1000,
  });
  for (const model of new Set([ALIAS, cfg.model]))
    await api(`model-prices/${encodeURIComponent(model)}`, "PUT", {
      billing_mode: "per_token",
      input_usd_per_1m: "0",
      output_usd_per_1m: "0",
      cache_read_usd_per_1m: "0",
      cache_write_usd_per_1m: "0",
      cache_write_1h_usd_per_1m: "0",
      reasoning_usd_per_1m: "0",
      enabled: true,
    });
  await api("providers", "POST", {
    name: "E2E",
    enabled: true,
    max_retries: 0,
    channel_max_retries: 0,
    channel_retry_interval_ms: 0,
    circuit_breaker_enabled: false,
    active_probe_enabled_override: false,
    group_ids: [auth.user.group_id],
    transforms: [],
    channels: [
      {
        name: "E2E",
        provider_type: cfg.upstreamType,
        base_url: "http://recorder:4080/upstream",
        api_key: cfg.apiKey,
        enabled: true,
        weight: 1,
        active_probe_enabled_override: false,
        models: { [ALIAS]: { redirect: cfg.model, multiplier: "1" } },
      },
    ],
  });
  const credential = await api("tokens", "POST", {
    name: "E2E",
    model_limits_enabled: true,
    model_limits: [ALIAS],
    request_capture_mode: "capture-all",
    request_capture_retention: "24h",
  });
  key = credential.key;
  recorder.secrets.push(key);
  await mkdir("/work", { recursive: true });
  await chown("/work", 1000, 1000);
  await writeFile(
    "/work/client-config.json",
    JSON.stringify({ harness: cfg.harness, key, prompt: cfg.prompt }),
    { mode: 0o600 },
  );
  await chown("/work/client-config.json", 1000, 1000);
  return { ok: true };
}
async function git(args: string[]) {
  const child = Bun.spawn(
    ["git", "-c", "safe.directory=/work/repo", "-C", "/work/repo", ...args],
    {
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const out = await new Response(child.stdout).text();
  await new Response(child.stderr).text();
  if ((await child.exited) !== 0)
    throw new Error("repository inspection failed");
  return out.trim();
}
async function snapshot(): Promise<Snapshot> {
  const result: Snapshot = {};
  try {
    const root = await realpath("/work/repo");
    if (root !== "/work/repo") throw new Error("checkout is a symlink");
    result.origin = await git(["config", "--get", "remote.origin.url"]);
    result.commit = await git(["rev-parse", "HEAD"]);
    const documentPath = await realpath("/work/architecture.md");
    if (documentPath !== "/work/architecture.md")
      throw new Error("architecture document is a symlink");
    if ((await stat(documentPath)).size > 1024 * 1024)
      throw new Error("architecture document exceeds 1 MiB");
    const document = await readFile(documentPath, "utf8");
    const files: string[] = [];
    for (const candidate of [...document.matchAll(/`([^`\n]+)`/g)].map(
      (m) => m[1]!,
    )) {
      if (
        path.isAbsolute(candidate) ||
        candidate.split("/").includes("..") ||
        !/\.(rs|[cm]?[jt]sx?|py|go|c|cpp|h|java|rb|sh|vue|svelte)$/.test(
          candidate,
        )
      )
        continue;
      try {
        const p = await realpath(path.join(root, candidate));
        if (p.startsWith(root + "/") && (await stat(p)).isFile())
          files.push(candidate);
      } catch {}
    }
    return {
      ...result,
      document,
      files,
    };
  } catch (e) {
    return { ...result, error: String(e) };
  }
}
async function collect() {
  let clientExit: { exitCode: number | null; exceeded?: boolean } = {
    exitCode: null,
  };
  try {
    clientExit = JSON.parse(await readFile("/work/client-exit.json", "utf8"));
  } catch {}
  if (clientExit.exceeded)
    recorder.add("failure", { category: "evidence_limit" });
  for (const [file, kind] of [
    ["client.jsonl", "client"],
    ["client.stderr", "client_stderr"],
  ] as const) {
    try {
      if ((await stat("/work/" + file)).size > BUDGET)
        throw new Error("evidence_limit");
      const lines = (await readFile("/work/" + file, "utf8")).split("\n");
      for (const line of lines) {
        if (!line.trim()) continue;
        if (kind === "client") {
          try {
            recorder.add(kind, { event: JSON.parse(line) });
          } catch {
            recorder.add("client_unparsed", { text: line });
          }
        } else recorder.add(kind, { text: line });
      }
    } catch (e) {
      recorder.add("collection_error", { file, error: String(e) });
    }
  }
  let captureCount = 0;
  async function walk(dir: string) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(file);
      else if (entry.isFile()) {
        if ((await stat(file)).size > BUDGET) {
          recorder.add("failure", { category: "evidence_limit" });
          continue;
        }
        try {
          const bytes = await readFile(file);
          const decoded = file.endsWith(".zst")
            ? Bun.zstdDecompressSync(bytes)
            : bytes;
          if (decoded.length > BUDGET) {
            recorder.add("failure", { category: "evidence_limit" });
            continue;
          }
          const capture = JSON.parse(new TextDecoder().decode(decoded));
          recorder.add("capture", { file: entry.name, capture });
          captureCount++;
        } catch {
          recorder.add("collection_error", {
            file: entry.name,
            error: "invalid_capture",
          });
        }
      }
    }
  }
  try {
    await walk("/data/dumps");
  } catch {}
  const result = { ...clientExit, snapshot: await snapshot(), captureCount };
  recorder.add("inspection", result);
  return redact(result, recorder.secrets);
}
const proxy = createProxy(
  {
    monoize: base,
    baseUrl: cfg.baseUrl,
    upstreamType: cfg.upstreamType,
    maxRequests: cfg.maxRequests,
  },
  recorder,
  async (req) => {
    if (req.headers.get("authorization") !== `Bearer ${cfg.controlKey}`)
      return new Response("unauthorized", { status: 401 });
    try {
      switch (new URL(req.url).pathname) {
        case "/control/ready": {
          const r = await fetch(base + "/api/dashboard/settings/public", {
            signal: AbortSignal.timeout(2000),
          });
          return Response.json({ ready: r.ok });
        }
        case "/control/bootstrap":
          return Response.json(await bootstrap());
        case "/control/status":
          return Response.json(proxy.status());
        case "/control/log":
          recorder.add("monoize_log", { text: await req.text() });
          return Response.json({ ok: true });
        case "/control/collect":
          return Response.json(await collect());
        default:
          return new Response("not found", { status: 404 });
      }
    } catch (e) {
      recorder.add("setup_error", { error: String(e) });
      return Response.json(redact({ error: String(e) }, recorder.secrets), {
        status: 500,
      });
    }
  },
  4080,
);
