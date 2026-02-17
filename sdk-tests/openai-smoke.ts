import OpenAI from "openai";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const sdkDir = resolve(import.meta.dir);
const rootDir = resolve(sdkDir, "..");

const mockPort = Number(process.env.MOCK_PORT ?? 3901);
const monoizePort = Number(process.env.MONOIZE_PORT ?? 8085);

const mockBase = `http://127.0.0.1:${mockPort}`;
const monoizeBase = `http://127.0.0.1:${monoizePort}`;

const tmpDir = join(sdkDir, ".tmp");
const dbPath = join(tmpDir, `monoize-sdk-${monoizePort}.db`);

const env = {
  ...process.env,
  MOCK_API_KEY: process.env.MOCK_API_KEY ?? "mock-key",
  MONOIZE_DATABASE_DSN: `sqlite://${dbPath}`,
  MONOIZE_LISTEN: `127.0.0.1:${monoizePort}`,
};

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
    waitFor(`${monoizeBase}/metrics`, 30000),
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
    body: JSON.stringify({ username: adminUsername, password: adminPassword }),
  });
  const registerBody = await registerResp.json();
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
  const apiKeyBody = await apiKeyResp.json();
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
      provider_type: "responses",
      models: {
        "gpt-4o-mini": { multiplier: 1.0 },
      },
      channels: [
        {
          name: "sdk-mock-channel",
          base_url: mockBase,
          api_key: env.MOCK_API_KEY,
        },
      ],
    }),
  });
  if (!providerResp.ok) {
    throw new Error(`Failed to create provider: ${await providerResp.text()}`);
  }

  return forwardApiKey;
}

async function main() {
  const mockProcess = await ensureMockServer();
  let monoizeProcess: Bun.ChildProcess | null = null;
  try {
    monoizeProcess = await startMonoizeServer();
    const forwardApiKey = await bootstrapMonoizeRouting();
    const client = new OpenAI({
      apiKey: forwardApiKey,
      baseURL: `${monoizeBase}/v1`,
    });

    const response = await client.responses.create({
      model: "gpt-4o-mini",
      input: "hello from sdk",
    });
    if (!Array.isArray(response.output) || response.output.length === 0) {
      throw new Error("responses.create returned empty output");
    }

    console.log("OpenAI SDK smoke test passed.");
  } finally {
    if (monoizeProcess) {
      monoizeProcess.kill();
      await monoizeProcess.exited;
    }
    if (mockProcess) {
      mockProcess.kill();
      await mockProcess.exited;
    }
    try {
      rmSync(dbPath, { force: true });
    } catch {
      // ignore cleanup errors
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
