import path from "node:path";
import {
  mkdir,
  readFile,
  writeFile,
  rm,
  realpath,
  stat,
} from "node:fs/promises";
import { createHash } from "node:crypto";
import {
  BUDGET,
  Cleanup,
  pool,
  redact,
  type Options,
  type Harness,
  type Evidence,
} from "./core";
import { taskPrompt } from "./harness";
import { validate } from "./validate";

export function buildProxyArgs(
  env: Record<string, string | undefined> = process.env,
  daemon: Record<string, string | undefined> = {},
): string[] {
  const args: string[] = [];
  for (const name of ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"]) {
    let value = env[name] ?? env[name.toLowerCase()];
    if (!value) continue;
    if (name !== "NO_PROXY") {
      const url = new URL(value);
      if (["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)) {
        url.hostname = "host.docker.internal";
        value = daemon[name] || url.toString();
      } else value = url.toString();
    }
    args.push("--build-arg", `${name}=${value}`);
  }
  return args;
}

export async function command(
  args: string[],
  signal?: AbortSignal,
  limit = BUDGET,
): Promise<string> {
  signal?.throwIfAborted();
  const child = Bun.spawn(args, {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    detached: true,
  });
  let escalation: ReturnType<typeof setTimeout> | undefined;
  const killGroup = (signal: NodeJS.Signals) => {
    try {
      process.kill(-child.pid, signal);
    } catch {
      try {
        child.kill(signal);
      } catch {}
    }
  };
  const abort = () => {
    killGroup("SIGTERM");
    escalation = setTimeout(() => killGroup("SIGKILL"), 2000);
  };
  signal?.addEventListener("abort", abort, { once: true });
  let bytes = 0;
  async function read(stream: ReadableStream<Uint8Array>) {
    const chunks: Uint8Array[] = [];
    for await (const c of stream) {
      bytes += c.length;
      if (bytes > limit) {
        child.kill("SIGKILL");
        throw new Error("command_output_limit");
      }
      chunks.push(c);
    }
    return Buffer.concat(chunks).toString();
  }
  try {
    const [out, err] = await Promise.all([
      read(child.stdout),
      read(child.stderr),
    ]);
    const code = await child.exited;
    signal?.throwIfAborted();
    if (code !== 0)
      throw new Error(`${args[0]} failed (${code}): ${err.slice(-4000)}`);
    return out;
  } finally {
    signal?.removeEventListener("abort", abort);
    if (escalation) clearTimeout(escalation);
  }
}
async function sourceState(root: string) {
  const commit = (
    await command(["git", "-C", root, "rev-parse", "HEAD"])
  ).trim();
  const status = await command(["git", "-C", root, "status", "--porcelain"]);
  const names = (
    await command([
      "git",
      "-C",
      root,
      "ls-files",
      "--cached",
      "--others",
      "--exclude-standard",
      "-z",
      "--",
      "src",
      "frontend",
      "Cargo.toml",
      "Cargo.lock",
      "build.rs",
    ])
  )
    .split("\0")
    .filter(
      (name) =>
        name &&
        !/^frontend\/(artifacts|dist|node_modules|\.cache)\//.test(name) &&
        !/^frontend\/\.env/.test(name),
    )
    .sort();
  const hash = createHash("sha256");
  for (const name of [...new Set(names)]) {
    hash.update(name + "\0");
    try {
      hash.update(await readFile(path.join(root, name)));
    } catch {
      hash.update("deleted");
    }
  }
  return { commit, status, sourceSha256: hash.digest("hex") };
}
async function resolveCommit(repo: string, ref: string, signal: AbortSignal) {
  if (/^[a-f\d]{40}$/i.test(ref)) return ref.toLowerCase();
  if (ref.startsWith("-")) throw new Error("Invalid Git ref");
  const output = await command(
    [
      "git",
      "ls-remote",
      "--",
      repo,
      ref,
      `refs/heads/${ref}`,
      `refs/tags/${ref}`,
      `refs/tags/${ref}^{}`,
    ],
    signal,
  );
  const entries = output
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((l) => l.split(/\s+/));
  const peeled = entries.find((e) => e[1]?.endsWith("^{}"));
  const commits = [...new Set(entries.map((e) => e[0]!))];
  if (peeled) return peeled[0]!;
  if (commits.length !== 1)
    throw new Error("Git ref is missing or ambiguous; supply an exact commit");
  return commits[0]!;
}
async function waitUntil(
  fn: () => Promise<boolean>,
  signal: AbortSignal,
  milliseconds = 120000,
) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    signal.throwIfAborted();
    try {
      if (await fn()) return;
    } catch (e) {
      if (signal.aborted) throw e;
    }
    await Bun.sleep(250);
  }
  throw new Error("Container readiness timeout");
}
interface ItemResult {
  harness: Harness;
  passed: boolean;
  category?: string;
  error?: string;
  protocol?: any;
  task?: any;
  exitCode?: number | null;
  cleanupErrors?: string[];
}
export async function run(
  opts: Options,
  root: string,
  externalSignal: AbortSignal,
) {
  // Resolve the existing parent before creating outputs to reject symlink escapes.
  let parent = path.dirname(opts.output);
  while (true) {
    try {
      parent = await realpath(parent);
      break;
    } catch {
      const next = path.dirname(parent);
      if (next === parent) throw new Error("Invalid output parent");
      parent = next;
    }
  }
  if (parent !== root && !parent.startsWith(root + path.sep))
    throw new Error("Output directory escapes project through a symlink");
  await mkdir(path.dirname(opts.output), { recursive: true });
  await mkdir(opts.output, { mode: 0o700 });
  const safe = (v: unknown) => redact(v, [opts.apiKey]);
  const results: ItemResult[] = [];
  const report: any = {
    schemaVersion: 1,
    startedAt: new Date().toISOString(),
    configuration: safe({ ...opts, apiKey: "[REDACTED]" }),
    results,
    unverified: opts.harnesses.slice(),
  };
  let saving = Promise.resolve();
  const save = () => {
    saving = saving.then(async () => {
      report.unverified = opts.harnesses.filter(
        (h) => !results.some((r) => r.harness === h),
      );
      await writeFile(
        path.join(opts.output, "report.json"),
        JSON.stringify(safe(report), null, 2),
        { mode: 0o600 },
      );
    });
    return saving;
  };
  await save();
  const runID = "e2e-" + crypto.randomUUID().slice(0, 12);
  report.resourcePrefix = runID;
  try {
    const daemonProxy = JSON.parse(
      await command(
        [
          "docker",
          "info",
          "--format",
          '{"HTTP_PROXY":{{json .HTTPProxy}},"HTTPS_PROXY":{{json .HTTPSProxy}}}',
        ],
        externalSignal,
      ),
    );
    report.source = await sourceState(root);
    const commit =
      opts.task === undefined
        ? await resolveCommit(opts.repo, opts.ref, externalSignal)
        : "";
    report.taskCommit = commit || null;
    let harnessImage = `monoize-e2e-harness:${createHash("sha256")
      .update(await readFile(path.join(root, "e2e/bun.lock")))
      .digest("hex")
      .slice(0, 12)}`;
    console.log("Building pinned harness image…");
    await command(
      [
        "docker",
        "build",
        ...buildProxyArgs(process.env, daemonProxy),
        "-f",
        path.join(root, "e2e/Harness.Dockerfile"),
        "-t",
        harnessImage,
        path.join(root, "e2e"),
      ],
      externalSignal,
    );
    let monoizeImage = opts.monoizeImage;
    if (!monoizeImage) {
      monoizeImage = `monoize-e2e:${report.source.sourceSha256.slice(0, 12)}`;
      console.log("Building current Monoize source…");
      await command(
        [
          "docker",
          "build",
          ...buildProxyArgs(process.env, daemonProxy),
          "-f",
          path.join(root, "e2e/Dockerfile"),
          "-t",
          monoizeImage,
          root,
        ],
        externalSignal,
      );
    } else {
      try {
        await command(
          ["docker", "image", "inspect", monoizeImage],
          externalSignal,
        );
      } catch {
        await command(["docker", "pull", monoizeImage], externalSignal);
      }
    }
    report.images = JSON.parse(
      await command(
        [
          "docker",
          "image",
          "inspect",
          harnessImage,
          monoizeImage,
          "--format",
          "{{json .}}",
        ],
        externalSignal,
      ).then((s) => "[" + s.trim().split("\n").join(",") + "]"),
    ).map((i: any) => ({ id: i.Id, digests: i.RepoDigests, tags: i.RepoTags }));
    harnessImage = report.images[0].id;
    monoizeImage = report.images[1].id;
    report.clientVersions = await command(
      [
        "docker",
        "run",
        "--rm",
        "--entrypoint",
        "sh",
        harnessImage,
        "-c",
        "codex --version && claude --version && opencode --version && pi --version",
      ],
      externalSignal,
    );
    await save();
    await pool(
      opts.harnesses,
      opts.jobs,
      async (harness) => {
        const prefix = `${runID}-${harness}`;
        const cleanup = new Cleanup();
        const timeout = AbortSignal.timeout(
          Math.min(opts.timeout * 1000, 2147483647),
        );
        const signal = AbortSignal.any([externalSignal, timeout]);
        const result: ItemResult = { harness, passed: false };
        const directory = path.join(opts.output, harness);
        await mkdir(directory);
        const configFile = path.join(directory, ".private.json");
        const controlKey = crypto.randomUUID();
        const password = crypto.randomUUID();
        const name = {
          network: prefix,
          data: prefix + "-data",
          work: prefix + "-work",
          recorder: prefix + "-recorder",
          monoize: prefix + "-monoize",
          client: prefix + "-client",
        };
        let controlURL = "";
        let createdRecorder = false;
        let createdMonoize = false;
        let clientCreated = false;
        async function control(
          endpoint: string,
          body?: string,
          activeSignal?: AbortSignal,
        ) {
          const response = await fetch(controlURL + "/control/" + endpoint, {
            method: body === undefined ? "GET" : "POST",
            headers: { authorization: `Bearer ${controlKey}` },
            body,
            signal: activeSignal ?? AbortSignal.timeout(30000),
          });
          if (!response.ok)
            throw new Error(
              `Control ${endpoint}: ${response.status} ${await response.text()}`,
            );
          return response.json() as Promise<any>;
        }
        const docker = (args: string[]) => command(["docker", ...args], signal);
        const removeResource = (args: string[]) =>
          cleanup.add(async () => {
            try {
              await command(["docker", ...args]);
            } catch (error) {
              if (
                !/No such (container|volume)|network .* not found/i.test(
                  String(error),
                )
              )
                throw error;
            }
          });
        removeResource(["network", "rm", name.network]);
        for (const volume of [name.data, name.work])
          removeResource(["volume", "rm", "-f", volume]);
        for (const container of [name.recorder, name.monoize, name.client])
          removeResource(["rm", "-f", container]);
        cleanup.add(() => rm(configFile, { force: true }));
        let inspection: any;
        try {
          console.log(`[${harness}] initializing`);
          await docker(["network", "create", name.network]);
          for (const volume of [name.data, name.work]) {
            await docker(["volume", "create", volume]);
          }
          await writeFile(
            configFile,
            JSON.stringify({
              baseUrl: opts.baseUrl,
              apiKey: opts.apiKey,
              model: opts.model,
              upstreamType: opts.upstreamType,
              maxRequests: opts.maxRequests,
              controlKey,
              password,
              harness,
              prompt: taskPrompt(opts.repo, commit, opts.task),
              timeout: opts.timeout,
            }),
            { mode: 0o600 },
          );
          await docker([
            "create",
            "--name",
            name.recorder,
            "--network",
            name.network,
            "--network-alias",
            "recorder",
            "-p",
            "127.0.0.1::4080",
            "-v",
            `${name.data}:/data`,
            "-v",
            `${name.work}:/work`,
            harnessImage,
          ]);
          createdRecorder = true;
          await docker(["cp", configFile, `${name.recorder}:/config.json`]);
          await rm(configFile, { force: true });
          await docker(["start", name.recorder]);
          const binding = JSON.parse(
            await docker([
              "inspect",
              name.recorder,
              "--format",
              "{{json .NetworkSettings.Ports}}",
            ]),
          );
          controlURL = `http://127.0.0.1:${binding["4080/tcp"][0].HostPort}`;
          await docker([
            "create",
            "--name",
            name.monoize,
            "--user",
            "0:0",
            "--network",
            name.network,
            "--network-alias",
            "monoize",
            "-v",
            `${name.data}:/data`,
            "-e",
            "MONOIZE_DATABASE_DSN=sqlite:///data/monoize.db?mode=rwc",
            "-e",
            "MONOIZE_LISTEN=0.0.0.0:8080",
            monoizeImage!,
          ]);
          createdMonoize = true;
          await docker(["start", name.monoize]);
          await waitUntil(
            async () => (await control("ready", undefined, signal)).ready,
            signal,
          );
          await docker(["stop", "-t", "10", name.monoize]);
          await docker([
            "exec",
            name.recorder,
            "bun",
            "/suite/worker.ts",
            "seed",
          ]);
          await docker(["start", name.monoize]);
          await waitUntil(
            async () => (await control("ready", undefined, signal)).ready,
            signal,
          );
          await control("bootstrap", "", signal);
          await docker([
            "create",
            "--name",
            name.client,
            "--user",
            "1000:1000",
            "--network",
            name.network,
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "-v",
            `${name.work}:/work`,
            harnessImage,
            "/suite/client.ts",
          ]);
          clientCreated = true;
          await docker(["start", name.client]);
          console.log(`[${harness}] running`);
          while (true) {
            signal.throwIfAborted();
            const status = await control("status", undefined, signal);
            if (status.fatal || status.exceeded)
              throw new Error(status.fatal ?? "evidence_limit");
            const state = JSON.parse(
              await docker([
                "inspect",
                name.client,
                "--format",
                "{{json .State}}",
              ]),
            );
            if (!state.Running) {
              result.exitCode = state.ExitCode;
              break;
            }
            await Bun.sleep(500);
          }
        } catch (e) {
          result.category = externalSignal.aborted
            ? "interrupted"
            : timeout.aborted
              ? "timeout"
              : String(e).includes("upstream_error")
                ? "upstream_error"
                : String(e).includes("request_limit")
                  ? "request_limit"
                  : String(e).includes("evidence_limit")
                    ? "evidence_limit"
                    : "setup";
          result.error = String(
            redact(String(e), [opts.apiKey, controlKey, password]),
          );
        } finally {
          try {
            if (clientCreated)
              await command(["docker", "stop", "-t", "2", name.client]);
            if (createdMonoize) {
              await command(["docker", "stop", "-t", "10", name.monoize]);
              const log = await command(["docker", "logs", name.monoize]).catch(
                (e) => String(e),
              );
              if (controlURL) await control("log", log);
            }
            if (createdRecorder && controlURL) {
              inspection = await control("collect", "");
              const status = await control("status");
              if (status.exceeded) {
                result.category = "evidence_limit";
              }
              await command([
                "docker",
                "cp",
                `${name.recorder}:/evidence/.`,
                directory,
              ]);
            }
          } catch (e) {
            result.category ??= "setup";
            result.error ??= String(
              redact(String(e), [opts.apiKey, controlKey, password]),
            );
          }
          result.cleanupErrors = await cleanup.run();
          if (result.cleanupErrors.length) result.category ??= "setup";
        }
        if (inspection) {
          try {
            const records = (
              await readFile(path.join(directory, "events.jsonl"), "utf8")
            )
              .split("\n")
              .filter(Boolean)
              .map((l) => JSON.parse(l)) as Evidence[];
            const validation = validate(
              harness,
              opts.upstreamType,
              opts.model,
              records,
              inspection.exitCode,
              inspection.snapshot,
              opts.repo,
              commit,
              opts.task !== undefined,
              opts.baseUrl,
            );
            result.protocol = validation.protocol;
            result.task = validation.task;
            if (!inspection.captureCount) {
              result.protocol.passed = false;
              result.protocol.errors.push("missing_monoize_capture");
            }
            result.passed =
              !result.category && result.protocol.passed && result.task.passed;
            if (!result.passed && !result.category)
              result.category = result.protocol.errors.includes(
                "transport_or_api_error",
              )
                ? "upstream_error"
                : result.protocol.errors.includes("tool_failure")
                  ? "tool_failure"
                  : !result.protocol.passed
                    ? "protocol_mismatch"
                    : "task_failure";
            await writeFile(
              path.join(directory, "final.md"),
              validation.final,
              { mode: 0o600 },
            );
            if (inspection.snapshot.document)
              await writeFile(
                path.join(directory, "architecture.md"),
                inspection.snapshot.document,
                { mode: 0o600 },
              );
          } catch (e) {
            result.category ??= "setup";
            result.error = String(e);
          }
        }
        results.push(result);
        await save();
        console.log(
          `[${harness}] ${result.passed ? "PASS" : `FAIL (${result.category})`}`,
        );
        return result;
      },
      externalSignal,
    );
    report.finishedAt = new Date().toISOString();
    report.interrupted = externalSignal.aborted;
    await save();
    console.table(
      results.map((r) => ({
        harness: r.harness,
        result: r.passed ? "PASS" : "FAIL",
        protocol: r.protocol?.passed ?? false,
        task: r.task?.passed ?? false,
        reason: r.category ?? "",
      })),
    );
    return externalSignal.aborted
      ? 130
      : results.length === opts.harnesses.length &&
          results.every((r) => r.passed)
        ? 0
        : 1;
  } catch (e) {
    report.error = safe(String(e));
    report.interrupted = externalSignal.aborted;
    await save();
    console.error(safe(String(e)));
    return externalSignal.aborted ? 130 : 2;
  }
}
