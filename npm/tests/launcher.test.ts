import { describe, expect, test } from "bun:test";
import path from "node:path";

import {
  currentNativeTarget,
  detectPackageManager,
  formatLauncherError,
  launchNativeExecutable,
  reinstallCommand,
  resolveNativeExecutable,
} from "../src/launcher.ts";
import { NATIVE_TARGETS, nativeTargetFor, platformVersion } from "../src/targets.ts";

describe("native target selection", () => {
  test("maps every published platform and architecture", () => {
    for (const target of NATIVE_TARGETS) {
      expect(nativeTargetFor(target.platform, target.architecture)).toEqual(target);
      expect(platformVersion("1.5.13", target)).toBe(`1.5.13-${target.versionSuffix}`);
    }
  });

  test("rejects unsupported pairs", () => {
    expect(() => currentNativeTarget("freebsd", "x64")).toThrow(
      "unsupported platform: freebsd (x64)",
    );
    expect(() => currentNativeTarget("darwin", "x64")).toThrow(
      "unsupported platform: darwin (x64)",
    );
    expect(() => currentNativeTarget("win32", "arm64")).toThrow(
      "unsupported platform: win32 (arm64)",
    );
  });
});

describe("package manager errors", () => {
  test("detects Bun, pnpm, and npm", () => {
    expect(detectPackageManager({ npm_config_user_agent: "bun/1.4.0" }, "monoize.js")).toBe(
      "bun",
    );
    expect(detectPackageManager({ npm_config_user_agent: "pnpm/10.0.0 npm/?" }, "monoize.js")).toBe(
      "pnpm",
    );
    expect(detectPackageManager({ npm_config_user_agent: "npm/10.9.8" }, "monoize.js")).toBe(
      "npm",
    );
    expect(
      detectPackageManager(
        {},
        "node_modules/.bin/monoize",
        "/project/node_modules/.pnpm/monoize/bin/monoize.js",
      ),
    ).toBe("pnpm");
  });

  test("provides the exact reinstall commands", () => {
    expect(reinstallCommand("bun")).toBe("bun install -g monoize@latest");
    expect(reinstallCommand("npm")).toBe("npm install -g monoize@latest");
    expect(reinstallCommand("pnpm")).toBe("pnpm add -g monoize@latest");
  });

  test("formats a missing optional dependency without a stack trace", () => {
    expect(formatLauncherError(new Error("missing optional dependency monoize-linux-x64"))).toContain(
      "Reinstall with:",
    );
  });
});

describe("native execution", () => {
  test("resolves the executable inside the selected package", () => {
    const target = NATIVE_TARGETS[0];
    const packageJson = path.join("/virtual", target.packageAlias, "package.json");
    const executable = resolveNativeExecutable(target, () => packageJson, () => true);
    expect(executable).toBe(path.join("/virtual", target.packageAlias, "bin", "monoize"));
  });

  test("rejects a package without its executable", () => {
    const target = NATIVE_TARGETS[0];
    expect(() => resolveNativeExecutable(target, () => "/virtual/package.json", () => false)).toThrow(
      "expected monoize",
    );
  });

  test("mirrors the native process exit code", async () => {
    const result = await launchNativeExecutable(process.execPath, ["-e", "process.exit(23)"]);
    expect(result).toEqual({ type: "code", exitCode: 23 });
  });
});
