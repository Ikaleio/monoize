import { afterAll, describe, expect, test } from "bun:test";
import { chmod, mkdir, readdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PROJECT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const TEST_ROOT = path.join(PROJECT_ROOT, "target", `npm-package-set-${process.pid}`);
const STAGE = path.join(TEST_ROOT, "stage");
const DIST = path.join(TEST_ROOT, "dist");
const BINARY = path.join(TEST_ROOT, "monoize");
const PACKAGE_SCRIPT = path.join(PROJECT_ROOT, "npm", "scripts", "package.ts");

async function runPackage(args: string[]): Promise<{ exitCode: number; stderr: string }> {
  const child = Bun.spawn([process.execPath, PACKAGE_SCRIPT, ...args], {
    cwd: PROJECT_ROOT,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { exitCode, stderr };
}

afterAll(async () => {
  await rm(TEST_ROOT, { recursive: true, force: true });
});

describe("partial npm package sets", () => {
  test("accepts one supported platform and rejects unknown or empty platform sets", async () => {
    await mkdir(TEST_ROOT, { recursive: true });
    await Bun.write(BINARY, "test executable\n");
    await chmod(BINARY, 0o755);

    expect(
      await runPackage([
        "stage-platform",
        "--tag",
        "v1.6.1",
        "--target",
        "x86_64-unknown-linux-musl",
        "--binary",
        BINARY,
        "--output-dir",
        STAGE,
      ]),
    ).toMatchObject({ exitCode: 0 });
    expect(
      await runPackage(["stage-root", "--tag", "v1.6.1", "--output-dir", STAGE]),
    ).toMatchObject({ exitCode: 0 });
    expect(
      await runPackage(["pack", "--packages-dir", STAGE, "--output-dir", DIST]),
    ).toMatchObject({ exitCode: 0 });

    expect(
      await runPackage(["verify", "--tag", "v1.6.1", "--directory", DIST]),
    ).toMatchObject({ exitCode: 0 });
    expect((await readdir(DIST)).sort()).toEqual([
      "monoize-1.6.1-linux-x64.tgz",
      "monoize-1.6.1.tgz",
    ]);

    const unknown = path.join(DIST, "monoize-1.6.1-unknown.tgz");
    await Bun.write(unknown, "unknown\n");
    const unknownResult = await runPackage([
      "verify",
      "--tag",
      "v1.6.1",
      "--directory",
      DIST,
    ]);
    expect(unknownResult.exitCode).toBe(1);
    expect(unknownResult.stderr).toContain("npm package set mismatch");

    await rm(unknown);
    await rm(path.join(DIST, "monoize-1.6.1-linux-x64.tgz"));
    const emptyResult = await runPackage([
      "verify",
      "--tag",
      "v1.6.1",
      "--directory",
      DIST,
    ]);
    expect(emptyResult.exitCode).toBe(1);
    expect(emptyResult.stderr).toContain("contains no platform package");
  });
});
