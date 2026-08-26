import { stepCountIs, generateText, tool } from "ai";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { z } from "zod";

const sdkDir = resolve(import.meta.dir);
const rootDir = resolve(sdkDir, "..");

const mockPort = Number(process.env.MOCK_PORT ?? 3901);
const monoizePort = Number(process.env.MONOIZE_PORT ?? 8085);

const mockBase = `http://127.0.0.1:${mockPort}`;
const monoizeBase = `http://127.0.0.1:${monoizePort}`;
const capSecret = "sdk-test-cap-secret";
const capToken = "sdk-test-cap-token";

const tmpDir = join(sdkDir, ".tmp");
const dbPath = join(tmpDir, `monoize-sdk-${monoizePort}.db`);

const env = {
  ...process.env,
  MOCK_API_KEY: process.env.MOCK_API_KEY ?? "mock-key",
  MONOIZE_DATABASE_DSN: `sqlite://${dbPath}`,
  MONOIZE_LISTEN: `127.0.0.1:${monoizePort}`,
  MONOIZE_CAP_API_ENDPOINT: "",
  MONOIZE_CAP_SECRET_KEY: capSecret,
};

function startTestCapServer() {
  return Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const url = new URL(request.url);
      if (request.method !== "POST" || url.pathname !== "/site-key/siteverify") {
        return new Response("not found", { status: 404 });
      }
      const body = asObject(await request.json());
      return Response.json({
        success: body?.secret === capSecret && body?.response === capToken,
      });
    },
  });
}

type SpawnedProcess = ReturnType<typeof Bun.spawn>;

interface RegisterResponseBody {
  token?: string;
  user?: {
    id?: string;
  };
}

interface ApiKeyResponseBody {
  key?: string;
}

interface StepSummary {
  index: number;
  text: string;
  toolCalls: number;
  toolResults: number;
  finishReason: string;
}

function asObject(value: unknown): Record<string, unknown> | null {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return null;
}

function parseRegisterResponse(value: unknown): RegisterResponseBody {
  const root = asObject(value);
  const user = asObject(root?.user);
  return {
    token: typeof root?.token === "string" ? root.token : undefined,
    user: user ? { id: typeof user.id === "string" ? user.id : undefined } : undefined,
  };
}

function parseApiKeyResponse(value: unknown): ApiKeyResponseBody {
  const root = asObject(value);
  return {
    key: typeof root?.key === "string" ? root.key : undefined,
  };
}

async function waitFor(url: string, timeoutMs: number) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const resp = await fetch(url);
      if (resp.ok) return;
    } catch {
      // ignore until timeout
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
  }
  throw new Error(`Timed out waiting for ${url}`);
}

async function ensureMockServer() {
  try {
    const resp = await fetch(`${mockBase}/health`);
    if (resp.ok) return null;
  } catch {
    // not running
  }
  const child = Bun.spawn({
    cmd: ["bun", "run", "server.ts"],
    cwd: join(rootDir, "mock"),
    env: { ...process.env, PORT: String(mockPort) },
    stdout: "inherit",
    stderr: "inherit",
  });
  await waitFor(`${mockBase}/health`, 5000);
  return child;
}

async function startMonoizeServer() {
  if (!existsSync(tmpDir)) {
    mkdirSync(tmpDir);
  }
  const child = Bun.spawn({
    cmd: ["cargo", "run", "--quiet"],
    cwd: rootDir,
    env,
    stdout: "inherit",
    stderr: "inherit",
  });
  await Promise.race([
    waitFor(`${monoizeBase}/api/dashboard/settings/public`, 30000),
    child.exited.then((code) => {
      throw new Error(`Monoize exited before ready (code ${code})`);
    }),
  ]);
  return child;
}

async function bootstrapMonoizeRouting() {
  const adminUsername = `sdk_admin_${monoizePort}`;
  const adminPassword = "sdk-pass-123";

  const registerResp = await fetch(`${monoizeBase}/api/dashboard/auth/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      username: adminUsername,
      password: adminPassword,
      captcha_token: capToken,
    }),
  });
  const registerBody = parseRegisterResponse(await registerResp.json());
  const sessionToken = registerBody?.token;
  const userId = registerBody?.user?.id;
  if (!registerResp.ok || typeof sessionToken !== "string" || typeof userId !== "string") {
    throw new Error(`Failed to register bootstrap admin: ${JSON.stringify(registerBody)}`);
  }

  const balanceResp = await fetch(`${monoizeBase}/api/dashboard/users/${userId}`, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ balance_unlimited: true }),
  });
  if (!balanceResp.ok) {
    throw new Error(`Failed to set unlimited balance: ${await balanceResp.text()}`);
  }

  const apiKeyResp = await fetch(`${monoizeBase}/api/dashboard/tokens`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ name: "sdk-forward-key" }),
  });
  const apiKeyBody = parseApiKeyResponse(await apiKeyResp.json());
  const forwardApiKey = apiKeyBody?.key;
  if (!apiKeyResp.ok || typeof forwardApiKey !== "string") {
    throw new Error(`Failed to create forwarding key: ${JSON.stringify(apiKeyBody)}`);
  }

  const providerResp = await fetch(`${monoizeBase}/api/dashboard/providers`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      name: "sdk-mock-provider",
      channels: [
        {
          name: "sdk-mock-channel",
          provider_type: "chat_completion",
          base_url: mockBase,
          api_key: env.MOCK_API_KEY,
          models: {
            "gpt-4o-mini": { redirect: null, multiplier: "1" },
          },
        },
      ],
    }),
  });
  if (!providerResp.ok) {
    throw new Error(`Failed to create provider: ${await providerResp.text()}`);
  }

  const metadataResp = await fetch(`${monoizeBase}/api/dashboard/model-metadata/gpt-4o-mini`, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      max_input_tokens: 8192,
      max_output_tokens: 4096,
    }),
  });
  if (!metadataResp.ok) {
    throw new Error(`Failed to seed model metadata: ${await metadataResp.text()}`);
  }

  // model-pricing.spec.md MP-D1: one model_prices row prices the model;
  // 0.001 USD per 1M tokens equals the former 1 nano-USD per token.
  const priceResp = await fetch(`${monoizeBase}/api/dashboard/model-prices/gpt-4o-mini`, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${sessionToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      billing_mode: "per_token",
      input_usd_per_1m: "0.001",
      output_usd_per_1m: "0.001",
    }),
  });
  if (!priceResp.ok) {
    throw new Error(`Failed to seed model price: ${await priceResp.text()}`);
  }

  return forwardApiKey;
}

function summarizeSteps(steps: unknown[]): StepSummary[] {
  return steps.map((step, index) => {
    const record = asObject(step);
    const toolCalls = Array.isArray(record?.toolCalls) ? record.toolCalls.length : 0;
    const toolResults = Array.isArray(record?.toolResults) ? record.toolResults.length : 0;
    const text = typeof record?.text === "string" ? record.text : "";
    const finishReason = typeof record?.finishReason === "string" ? record.finishReason : "unknown";
    return {
      index,
      text,
      toolCalls,
      toolResults,
      finishReason,
    };
  });
}

function assertMultiStepToolLoop(steps: unknown[]) {
  if (steps.length < 3) {
    throw new Error(`Expected at least 3 steps, got ${steps.length}`);
  }

  const summaries = summarizeSteps(steps);
  const toolCallSteps = summaries.filter((step) => step.toolCalls > 0);
  const toolResultSteps = summaries.filter((step) => step.toolResults > 0);

  if (toolCallSteps.length < 2) {
    throw new Error(`Expected at least 2 tool-call steps, got ${toolCallSteps.length}: ${JSON.stringify(summaries)}`);
  }
  if (toolResultSteps.length < 2) {
    throw new Error(`Expected at least 2 tool-result steps, got ${toolResultSteps.length}: ${JSON.stringify(summaries)}`);
  }
  if (toolCallSteps[0]?.toolCalls !== 1 || toolCallSteps[1]?.toolCalls !== 1) {
    throw new Error(`Expected one tool call in each tool step: ${JSON.stringify(summaries)}`);
  }
}

async function main() {
  const capServer = startTestCapServer();
  env.MONOIZE_CAP_API_ENDPOINT = `http://127.0.0.1:${capServer.port}/site-key/`;
  let mockProcess: SpawnedProcess | null = null;
  let monoizeProcess: SpawnedProcess | null = null;
  try {
    mockProcess = await ensureMockServer();
    monoizeProcess = await startMonoizeServer();
    const forwardApiKey = await bootstrapMonoizeRouting();
    const provider = createOpenAICompatible({
      name: "monoize-local",
      apiKey: forwardApiKey,
      baseURL: `${monoizeBase}/v1`,
    });

    const result = await generateText({
      model: provider.chatModel("gpt-4o-mini"),
      prompt:
        "Use the weather tool for Taipei, then use the websearch tool for Monoize, then answer with the tool results.",
      tools: {
        weather: tool({
          description: "Get weather information for a city.",
          inputSchema: z.object({
            city: z.string().min(1),
          }),
          execute: async ({ city }) => `WEATHER_RESULT:${city}:sunny 25C`,
        }),
        websearch: tool({
          description: "Search for public facts about a topic.",
          inputSchema: z.object({
            query: z.string().min(1),
          }),
          execute: async ({ query }) => `WEBSEARCH_RESULT:${query}:Monoize proxy`,
        }),
      },
      stopWhen: stepCountIs(5),
    });

    const steps = Array.isArray(result.steps) ? result.steps : [];
    assertMultiStepToolLoop(steps);

    if (!result.text.includes("WEATHER_RESULT:Taipei:sunny 25C")) {
      throw new Error(`Final text missing weather result: ${result.text}`);
    }
    if (!result.text.includes("WEBSEARCH_RESULT:Monoize:Monoize proxy")) {
      throw new Error(`Final text missing websearch result: ${result.text}`);
    }

    console.log("PASS openai-agent-tool-smoke");
    console.log(JSON.stringify(summarizeSteps(steps), null, 2));
  } finally {
    if (monoizeProcess) {
      monoizeProcess.kill();
      await monoizeProcess.exited;
    }
    if (mockProcess) {
      mockProcess.kill();
      await mockProcess.exited;
    }
    capServer.stop(true);
    try {
      rmSync(dbPath, { force: true });
    } catch {
      // ignore cleanup errors
    }
  }
}

main().catch((err) => {
  console.error("FAIL openai-agent-tool-smoke");
  console.error(err);
  process.exitCode = 1;
});
