#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir, readdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  NATIVE_TARGETS,
  nativeTargetForRustTarget,
  platformVersion,
  type NativeTarget,
} from "../src/targets.ts";

const PROJECT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const PACKAGE_DESCRIPTION =
  "A protocol-normalizing AI gateway with provider routing, fail-forward, transforms, and billing.";
const REPOSITORY = {
  type: "git",
  url: "git+https://github.com/Ikaleio/monoize.git",
};

interface PackageManifest {
  name: string;
  version: string;
  [key: string]: unknown;
}

function parseOptions(args: string[]): Map<string, string> {
  const options = new Map<string, string>();
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    const value = args[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      throw new Error(`invalid option list near ${key ?? "<end>"}`);
    }
    options.set(key.slice(2), value);
  }
  return options;
}

function requiredOption(options: Map<string, string>, name: string): string {
  const value = options.get(name);
  if (!value) {
    throw new Error(`missing --${name}`);
  }
  return value;
}

async function cargoVersion(): Promise<string> {
  const manifest = Bun.TOML.parse(await Bun.file(path.join(PROJECT_ROOT, "Cargo.toml")).text()) as {
    package?: { version?: unknown };
  };
  const version = manifest.package?.version;
  if (typeof version !== "string" || version.length === 0) {
    throw new Error("Cargo.toml does not contain a package version");
  }
  return version;
}

async function versionForTag(tag: string): Promise<string> {
  const version = await cargoVersion();
  const expected = `v${version}`;
  if (tag !== expected) {
    throw new Error(`release tag ${tag} does not match Cargo version ${version}; expected ${expected}`);
  }
  return version;
}

async function resetDirectory(directory: string): Promise<void> {
  const resolved = path.resolve(directory);
  if (resolved === PROJECT_ROOT || !resolved.startsWith(`${PROJECT_ROOT}${path.sep}`)) {
    throw new Error(`generated package directory must be inside ${PROJECT_ROOT}`);
  }
  await rm(resolved, { recursive: true, force: true });
  await mkdir(resolved, { recursive: true });
}

async function writeManifest(directory: string, manifest: PackageManifest): Promise<void> {
  await Bun.write(path.join(directory, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
}

async function copyPackageDocuments(directory: string, includeReadme: boolean): Promise<void> {
  await copyFile(path.join(PROJECT_ROOT, "LICENSE"), path.join(directory, "LICENSE"));
  if (includeReadme) {
    await copyFile(path.join(PROJECT_ROOT, "README.md"), path.join(directory, "README.md"));
  }
}

function rootManifest(version: string): PackageManifest {
  const optionalDependencies = Object.fromEntries(
    NATIVE_TARGETS.map((target) => [
      target.packageAlias,
      `npm:monoize@${platformVersion(version, target)}`,
    ]),
  );
  return {
    name: "monoize",
    version,
    description: PACKAGE_DESCRIPTION,
    license: "MIT",
    type: "module",
    bin: { monoize: "bin/monoize.js" },
    engines: { node: ">=18" },
    files: ["bin"],
    repository: REPOSITORY,
    homepage: "https://github.com/Ikaleio/monoize#readme",
    bugs: "https://github.com/Ikaleio/monoize/issues",
    optionalDependencies,
    publishConfig: { access: "public" },
  };
}

function platformManifest(version: string, target: NativeTarget): PackageManifest {
  return {
    name: "monoize",
    version: platformVersion(version, target),
    description: `${PACKAGE_DESCRIPTION} Native binary for ${target.platform} ${target.architecture}.`,
    license: "MIT",
    os: [target.platform],
    cpu: [target.architecture],
    engines: { node: ">=18" },
    files: ["bin"],
    repository: REPOSITORY,
    homepage: "https://github.com/Ikaleio/monoize#readme",
    publishConfig: { access: "public" },
  };
}

async function stageRoot(tag: string, outputDirectory: string): Promise<void> {
  const version = await versionForTag(tag);
  const packageDirectory = path.resolve(outputDirectory, "root");
  await resetDirectory(packageDirectory);
  const binDirectory = path.join(packageDirectory, "bin");
  await mkdir(binDirectory, { recursive: true });

  const build = await Bun.build({
    entrypoints: [path.join(PROJECT_ROOT, "npm/src/cli.ts")],
    target: "node",
    format: "esm",
    minify: true,
    outdir: binDirectory,
    naming: "monoize.js",
  });
  if (!build.success) {
    const messages = build.logs.map((log) => log.message).join("\n");
    throw new Error(`launcher build failed${messages ? `:\n${messages}` : ""}`);
  }
  await chmod(path.join(binDirectory, "monoize.js"), 0o755);
  await writeManifest(packageDirectory, rootManifest(version));
  await copyPackageDocuments(packageDirectory, true);
}

async function stagePlatform(
  tag: string,
  rustTarget: string,
  binary: string,
  outputDirectory: string,
): Promise<void> {
  const version = await versionForTag(tag);
  const target = nativeTargetForRustTarget(rustTarget);
  if (!target) {
    throw new Error(`unsupported Rust target: ${rustTarget}`);
  }

  const source = path.resolve(binary);
  if (!(await Bun.file(source).exists())) {
    throw new Error(`native executable does not exist: ${source}`);
  }

  const packageDirectory = path.resolve(outputDirectory, target.packageAlias);
  await resetDirectory(packageDirectory);
  const binDirectory = path.join(packageDirectory, "bin");
  await mkdir(binDirectory, { recursive: true });
  const destination = path.join(binDirectory, target.executable);
  await copyFile(source, destination);
  if (target.platform !== "win32") {
    await chmod(destination, 0o755);
  }
  await writeManifest(packageDirectory, platformManifest(version, target));
  await copyPackageDocuments(packageDirectory, false);
}

async function packageDirectories(packagesDirectory: string, outputDirectory: string): Promise<void> {
  const source = path.resolve(packagesDirectory);
  const output = path.resolve(outputDirectory);
  await mkdir(output, { recursive: true });
  const entries = await readdir(source, { withFileTypes: true });
  const packageDirectories = entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(source, entry.name))
    .sort();
  if (packageDirectories.length === 0) {
    throw new Error(`no staged packages in ${source}`);
  }

  for (const directory of packageDirectories) {
    const child = Bun.spawn(
      [process.execPath, "pm", "pack", "--destination", output, "--ignore-scripts"],
      { cwd: directory, stdin: "inherit", stdout: "inherit", stderr: "inherit" },
    );
    const exitCode = await child.exited;
    if (exitCode !== 0) {
      throw new Error(`bun pm pack failed for ${directory} with exit code ${exitCode}`);
    }
  }
}

function expectedTarballs(version: string, targets: readonly NativeTarget[]): string[] {
  return [
    `monoize-${version}.tgz`,
    ...targets.map((target) => `monoize-${platformVersion(version, target)}.tgz`),
  ].sort();
}

async function archiveText(archive: string, entry: string): Promise<string> {
  const child = Bun.spawn(["tar", "-xOzf", archive, entry], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`cannot read ${entry} from ${archive}: ${stderr.trim()}`);
  }
  return stdout;
}

async function archiveEntries(archive: string): Promise<string[]> {
  const child = Bun.spawn(["tar", "-tzf", archive], { stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`cannot list ${archive}: ${stderr.trim()}`);
  }
  return stdout
    .split("\n")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

async function verifyRootArchive(archive: string, version: string): Promise<void> {
  const manifest = JSON.parse(await archiveText(archive, "package/package.json")) as PackageManifest;
  const expectedDependencies = rootManifest(version).optionalDependencies;
  if (
    manifest.name !== "monoize" ||
    manifest.version !== version ||
    JSON.stringify(manifest.bin) !== JSON.stringify({ monoize: "bin/monoize.js" }) ||
    JSON.stringify(manifest.optionalDependencies) !== JSON.stringify(expectedDependencies)
  ) {
    throw new Error(`invalid root package manifest in ${archive}`);
  }
  const entries = await archiveEntries(archive);
  if (!entries.includes("package/bin/monoize.js")) {
    throw new Error(`root package does not contain bin/monoize.js: ${archive}`);
  }
}

async function verifyPlatformArchive(
  archive: string,
  version: string,
  target: NativeTarget,
): Promise<void> {
  const manifest = JSON.parse(await archiveText(archive, "package/package.json")) as PackageManifest;
  if (
    manifest.name !== "monoize" ||
    manifest.version !== platformVersion(version, target) ||
    JSON.stringify(manifest.os) !== JSON.stringify([target.platform]) ||
    JSON.stringify(manifest.cpu) !== JSON.stringify([target.architecture]) ||
    "bin" in manifest
  ) {
    throw new Error(`invalid platform package manifest in ${archive}`);
  }
  const entries = await archiveEntries(archive);
  const expectedExecutable = `package/bin/${target.executable}`;
  const nativeExecutables = entries.filter(
    (entry) => entry === "package/bin/monoize" || entry === "package/bin/monoize.exe",
  );
  if (nativeExecutables.length !== 1 || nativeExecutables[0] !== expectedExecutable) {
    throw new Error(`platform package contains the wrong native executable: ${archive}`);
  }
}

async function verifyPackageSet(tag: string, directory: string): Promise<readonly NativeTarget[]> {
  const version = await versionForTag(tag);
  const packageDirectory = path.resolve(directory);
  const actual = (await readdir(packageDirectory))
    .filter((entry) => entry.endsWith(".tgz"))
    .sort();
  const presentTargets = NATIVE_TARGETS.filter((target) =>
    actual.includes(`monoize-${platformVersion(version, target)}.tgz`),
  );
  if (presentTargets.length === 0) {
    throw new Error("npm package set contains no platform package");
  }
  const expected = expectedTarballs(version, presentTargets);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `npm package set mismatch; expected=${JSON.stringify(expected)}; actual=${JSON.stringify(actual)}`,
    );
  }

  await verifyRootArchive(path.join(packageDirectory, `monoize-${version}.tgz`), version);
  for (const target of presentTargets) {
    const archive = path.join(
      packageDirectory,
      `monoize-${platformVersion(version, target)}.tgz`,
    );
    await verifyPlatformArchive(archive, version, target);
  }
  return presentTargets;
}

async function npmOutput(args: string[]): Promise<{ exitCode: number; stdout: string }> {
  const child = Bun.spawn(["npm", ...args], { stdout: "pipe", stderr: "pipe" });
  const [stdout, , exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { exitCode, stdout: stdout.trim() };
}

async function publishArchive(
  archive: string,
  version: string,
  distTag: string,
): Promise<void> {
  const bytes = new Uint8Array(await Bun.file(archive).arrayBuffer());
  const localIntegrity = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
  const remote = await npmOutput(["view", `monoize@${version}`, "dist.integrity", "--json"]);

  if (remote.exitCode === 0 && remote.stdout) {
    const remoteIntegrity = JSON.parse(remote.stdout) as unknown;
    if (remoteIntegrity !== localIntegrity) {
      throw new Error(`npm already contains monoize@${version} with different bytes`);
    }
    const remoteTag = await npmOutput(["view", `monoize@${distTag}`, "version", "--json"]);
    const taggedVersion = remoteTag.exitCode === 0 && remoteTag.stdout
      ? JSON.parse(remoteTag.stdout) as unknown
      : undefined;
    if (taggedVersion !== version) {
      throw new Error(
        `npm already contains monoize@${version}, but dist-tag ${distTag} does not resolve to it`,
      );
    }
    return;
  }

  await runNpm(["publish", archive, "--access", "public", "--tag", distTag]);
}

async function runNpm(args: string[]): Promise<void> {
  const child = Bun.spawn(["npm", ...args], {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await child.exited;
  if (exitCode !== 0) {
    throw new Error(`npm ${args.join(" ")} failed with exit code ${exitCode}`);
  }
}

async function publishPackageSet(tag: string, directory: string): Promise<void> {
  const presentTargets = await verifyPackageSet(tag, directory);
  const version = await versionForTag(tag);
  const packageDirectory = path.resolve(directory);

  for (const target of presentTargets) {
    const targetVersion = platformVersion(version, target);
    await publishArchive(
      path.join(packageDirectory, `monoize-${targetVersion}.tgz`),
      targetVersion,
      `platform-${target.versionSuffix}`,
    );
  }
  await publishArchive(
    path.join(packageDirectory, `monoize-${version}.tgz`),
    version,
    "latest",
  );
}

function usage(): string {
  return [
    "usage:",
    "  bun npm/scripts/package.ts stage-root --tag <tag> --output-dir <dir>",
    "  bun npm/scripts/package.ts stage-platform --tag <tag> --target <rust-target> --binary <path> --output-dir <dir>",
    "  bun npm/scripts/package.ts pack --packages-dir <dir> --output-dir <dir>",
    "  bun npm/scripts/package.ts verify --tag <tag> --directory <dir>",
    "  bun npm/scripts/package.ts publish --tag <tag> --directory <dir>",
  ].join("\n");
}

async function main(): Promise<void> {
  const [command, ...args] = process.argv.slice(2);
  const options = parseOptions(args);
  switch (command) {
    case "stage-root":
      await stageRoot(requiredOption(options, "tag"), requiredOption(options, "output-dir"));
      break;
    case "stage-platform":
      await stagePlatform(
        requiredOption(options, "tag"),
        requiredOption(options, "target"),
        requiredOption(options, "binary"),
        requiredOption(options, "output-dir"),
      );
      break;
    case "pack":
      await packageDirectories(
        requiredOption(options, "packages-dir"),
        requiredOption(options, "output-dir"),
      );
      break;
    case "verify":
      await verifyPackageSet(requiredOption(options, "tag"), requiredOption(options, "directory"));
      break;
    case "publish":
      await publishPackageSet(requiredOption(options, "tag"), requiredOption(options, "directory"));
      break;
    default:
      throw new Error(usage());
  }
}

try {
  await main();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`npm package error: ${message}`);
  process.exit(1);
}
