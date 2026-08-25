#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { mkdir, readdir, realpath, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { NATIVE_TARGETS, nativeTargetFor, platformVersion } from "../src/targets.ts";

const PROJECT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

interface RegistryManifest {
  name: string;
  version: string;
  [key: string]: unknown;
}

interface RegistryHarness {
  readonly url: string;
  readonly downloadedVersions: string[];
  resetDownloads(): void;
  stop(): void;
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

async function run(command: string[], cwd: string, environment: NodeJS.ProcessEnv): Promise<void> {
  const child = Bun.spawn(command, {
    cwd,
    env: environment,
    stdin: "ignore",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await child.exited;
  if (exitCode !== 0) {
    throw new Error(`${command.join(" ")} failed with exit code ${exitCode}`);
  }
}

async function archiveManifest(archive: string): Promise<RegistryManifest> {
  const child = Bun.spawn(["tar", "-xOzf", archive, "package/package.json"], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`cannot read package manifest from ${archive}: ${stderr.trim()}`);
  }
  return JSON.parse(stdout) as RegistryManifest;
}

async function registryManifest(
  archive: string,
  tarballUrl: string,
): Promise<RegistryManifest> {
  const bytes = new Uint8Array(await Bun.file(archive).arrayBuffer());
  return {
    ...(await archiveManifest(archive)),
    dist: {
      tarball: tarballUrl,
      shasum: createHash("sha1").update(bytes).digest("hex"),
      integrity: `sha512-${createHash("sha512").update(bytes).digest("base64")}`,
    },
  };
}

async function startRegistry(packageDirectory: string): Promise<RegistryHarness> {
  const archives = (await readdir(packageDirectory))
    .filter((entry) => entry.startsWith("monoize-") && entry.endsWith(".tgz"))
    .sort();
  const archiveByFilename = new Map(
    archives.map((filename) => [filename, path.join(packageDirectory, filename)]),
  );
  const downloadedVersions: string[] = [];
  let registryUrl = "";
  let versions: Record<string, RegistryManifest> = {};

  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(request) {
      const pathname = decodeURIComponent(new URL(request.url).pathname);
      if (pathname === "/monoize") {
        return Response.json({
          name: "monoize",
          "dist-tags": { latest: Object.keys(versions).find((version) => !version.includes("-")) },
          versions,
        });
      }

      const versionMatch = pathname.match(/^\/monoize\/([^/]+)$/);
      if (versionMatch) {
        const manifest = versions[versionMatch[1]];
        return manifest ? Response.json(manifest) : new Response("not found", { status: 404 });
      }

      const tarballMatch = pathname.match(/^\/monoize\/-\/(monoize-(.+)\.tgz)$/);
      if (tarballMatch) {
        const archive = archiveByFilename.get(tarballMatch[1]);
        if (!archive) {
          return new Response("not found", { status: 404 });
        }
        downloadedVersions.push(tarballMatch[2]);
        return new Response(Bun.file(archive));
      }
      return new Response("not found", { status: 404 });
    },
  });

  registryUrl = `http://${server.hostname}:${server.port}`;
  versions = Object.fromEntries(
    await Promise.all(
      archives.map(async (filename) => {
        const archive = archiveByFilename.get(filename)!;
        const manifest = await registryManifest(
          archive,
          `${registryUrl}/monoize/-/${encodeURIComponent(filename)}`,
        );
        return [manifest.version, manifest];
      }),
    ),
  );

  return {
    url: `${registryUrl}/`,
    downloadedVersions,
    resetDownloads() {
      downloadedVersions.length = 0;
    },
    stop() {
      server.stop(true);
    },
  };
}

async function verifyInstallation(project: string): Promise<void> {
  const current = nativeTargetFor(process.platform, process.arch);
  if (!current) {
    throw new Error(`unsupported verification platform: ${process.platform} (${process.arch})`);
  }

  const rootPackage = path.join(project, "node_modules", "monoize", "package.json");
  if (!(await Bun.file(rootPackage).exists())) {
    throw new Error(`client did not install the root package: ${project}`);
  }

  const installed: string[] = [];
  const resolveFromRoot = createRequire(await realpath(rootPackage)).resolve;
  for (const target of NATIVE_TARGETS) {
    try {
      resolveFromRoot(`${target.packageAlias}/package.json`);
      installed.push(target.packageAlias);
    } catch {
      // A non-matching optional dependency must not resolve from the root package.
    }
  }
  if (JSON.stringify(installed) !== JSON.stringify([current.packageAlias])) {
    throw new Error(
      `client installed the wrong platform package set; expected=${current.packageAlias}; actual=${installed.join(",")}`,
    );
  }

  const binaryLink = path.join(
    project,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "monoize.cmd" : "monoize",
  );
  if (!(await Bun.file(binaryLink).exists())) {
    throw new Error(`client did not expose the monoize binary link: ${project}`);
  }
}

async function verifyClient(
  name: string,
  command: string[],
  rootTarball: string,
  workingDirectory: string,
  registry: RegistryHarness,
  expectedPlatformVersion: string,
): Promise<void> {
  const project = path.join(workingDirectory, name);
  await mkdir(project, { recursive: true });
  await Bun.write(
    path.join(project, "package.json"),
    `${JSON.stringify({ name: `monoize-${name}-verification`, private: true })}\n`,
  );
  const environment = {
    ...process.env,
    npm_config_cache: path.join(workingDirectory, "npm-cache"),
    npm_config_registry: registry.url,
    BUN_INSTALL_CACHE_DIR: path.join(workingDirectory, "bun-cache"),
  };
  registry.resetDownloads();
  await run([...command, rootTarball], project, environment);
  await verifyInstallation(project);

  const downloads = [...new Set(registry.downloadedVersions)];
  if (JSON.stringify(downloads) !== JSON.stringify([expectedPlatformVersion])) {
    throw new Error(
      `${name} downloaded the wrong native package set; expected=${expectedPlatformVersion}; actual=${downloads.join(",")}`,
    );
  }
}

async function main(): Promise<void> {
  const options = parseOptions(process.argv.slice(2));
  const tag = requiredOption(options, "tag");
  const version = tag.startsWith("v") ? tag.slice(1) : "";
  if (!version) {
    throw new Error(`invalid release tag: ${tag}`);
  }

  const current = nativeTargetFor(process.platform, process.arch);
  if (!current) {
    throw new Error(`unsupported verification platform: ${process.platform} (${process.arch})`);
  }
  const expectedPlatformVersion = platformVersion(version, current);
  const packageDirectory = path.resolve(requiredOption(options, "directory"));
  const pnpm = path.resolve(requiredOption(options, "pnpm"));
  const rootTarball = path.join(packageDirectory, `monoize-${version}.tgz`);
  const workingDirectory = path.join(PROJECT_ROOT, "target", "npm-client-verification");
  await rm(workingDirectory, { recursive: true, force: true });
  await mkdir(workingDirectory, { recursive: true });

  const registry = await startRegistry(packageDirectory);
  try {
    await verifyClient(
      "npm",
      ["npm", "install", "--ignore-scripts", "--no-audit", "--no-fund"],
      rootTarball,
      workingDirectory,
      registry,
      expectedPlatformVersion,
    );
    await verifyClient(
      "bun",
      [process.execPath, "add", "--ignore-scripts"],
      rootTarball,
      workingDirectory,
      registry,
      expectedPlatformVersion,
    );
    await verifyClient(
      "pnpm",
      [pnpm, "add", "--ignore-scripts", "--store-dir", path.join(workingDirectory, "pnpm-store")],
      rootTarball,
      workingDirectory,
      registry,
      expectedPlatformVersion,
    );
  } finally {
    registry.stop();
  }

  const projects = (await readdir(workingDirectory, { withFileTypes: true }))
    .filter((entry) => entry.isDirectory() && ["bun", "npm", "pnpm"].includes(entry.name))
    .map((entry) => entry.name)
    .sort();
  console.log(`verified package installation with ${projects.join(", ")}`);
}

try {
  await main();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`npm client verification error: ${message}`);
  process.exit(1);
}
