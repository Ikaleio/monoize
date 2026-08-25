import { spawn } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { nativeTargetFor, type NativeTarget } from "./targets.ts";

// pnpm keeps optional dependencies beside the canonical package directory in its virtual store.
const canonicalLauncherPath = realpathSync(fileURLToPath(import.meta.url));
const resolveFromInstalledPackage = createRequire(canonicalLauncherPath).resolve;

export type PackageManager = "bun" | "npm" | "pnpm";

export type ChildResult =
  | { readonly type: "code"; readonly exitCode: number }
  | { readonly type: "signal"; readonly signal: NodeJS.Signals };

export function detectPackageManager(
  environment: NodeJS.ProcessEnv = process.env,
  entrypoint = process.argv[1] ?? "",
  installedPath = canonicalLauncherPath,
): PackageManager {
  const userAgent = environment.npm_config_user_agent ?? "";
  const npmExecPath = environment.npm_execpath ?? "";

  if (
    /\bpnpm\//.test(userAgent) ||
    npmExecPath.includes("pnpm") ||
    installedPath.includes(`${path.sep}.pnpm${path.sep}`)
  ) {
    return "pnpm";
  }
  if (
    /\bbun\//.test(userAgent) ||
    npmExecPath.includes("bun") ||
    entrypoint.includes(".bun/install/global") ||
    entrypoint.includes(".bun\\install\\global")
  ) {
    return "bun";
  }
  return "npm";
}

export function reinstallCommand(packageManager: PackageManager): string {
  switch (packageManager) {
    case "bun":
      return "bun install -g monoize@latest";
    case "pnpm":
      return "pnpm add -g monoize@latest";
    default:
      return "npm install -g monoize@latest";
  }
}

export function currentNativeTarget(
  platform = process.platform,
  architecture = process.arch,
): NativeTarget {
  const target = nativeTargetFor(platform, architecture);
  if (!target) {
    throw new Error(`unsupported platform: ${platform} (${architecture})`);
  }
  return target;
}

export function resolveNativeExecutable(
  target: NativeTarget,
  resolvePackage = resolveFromInstalledPackage,
  fileExists = existsSync,
): string {
  let packageJson: string;
  try {
    packageJson = resolvePackage(`${target.packageAlias}/package.json`);
  } catch {
    throw new Error(`missing optional dependency ${target.packageAlias}`);
  }

  const executable = path.join(path.dirname(packageJson), "bin", target.executable);
  if (!fileExists(executable)) {
    throw new Error(
      `missing optional dependency ${target.packageAlias}: expected ${target.executable}`,
    );
  }
  return executable;
}

export async function launchNativeExecutable(
  executable: string,
  args: readonly string[],
): Promise<ChildResult> {
  const child = spawn(executable, [...args], {
    cwd: process.cwd(),
    env: process.env,
    stdio: "inherit",
  });

  const forwardedSignals: NodeJS.Signals[] = ["SIGINT", "SIGTERM", "SIGHUP"];
  const handlers = new Map<NodeJS.Signals, () => void>();
  for (const signal of forwardedSignals) {
    const handler = () => {
      if (!child.killed) {
        try {
          child.kill(signal);
        } catch {
          // The child can exit between the killed check and signal delivery.
        }
      }
    };
    handlers.set(signal, handler);
    process.on(signal, handler);
  }

  const removeSignalHandlers = () => {
    for (const [signal, handler] of handlers) {
      process.off(signal, handler);
    }
  };

  return await new Promise<ChildResult>((resolve, reject) => {
    child.once("error", (error) => {
      removeSignalHandlers();
      reject(error);
    });
    child.once("exit", (code, signal) => {
      removeSignalHandlers();
      if (signal) {
        resolve({ type: "signal", signal });
      } else {
        resolve({ type: "code", exitCode: code ?? 1 });
      }
    });
  });
}

export function formatLauncherError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (!message.startsWith("missing optional dependency")) {
    return `monoize: ${message}`;
  }

  const command = reinstallCommand(detectPackageManager());
  return `monoize: ${message}. Reinstall with: ${command}`;
}
