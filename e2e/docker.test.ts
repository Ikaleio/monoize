import { test, expect } from "bun:test";
import path from "node:path";
import { options } from "./core";
import { run, command } from "./runner";
import { fixture } from "./fixture";

// This opt-in suite runs real binaries against deterministic responses, not a paid model.
test.skipIf(!process.env.E2E_DOCKER_IMAGE)(
  "six container clients against three controlled upstream protocols",
  async () => {
    const server = fixture();
    const root = path.resolve(import.meta.dir, "..");
    try {
      for (const endpoint of ["responses", "chat/completions", "messages"]) {
        const opts = await options(
          [
            "--monoize-image",
            process.env.E2E_DOCKER_IMAGE!,
            "--jobs",
            "2",
            "--timeout",
            "180",
            "--max-requests",
            "8",
            "--task",
            "Use a shell tool to run printf fixture-tool-ok. Then confirm completion briefly.",
          ],
          root,
          {
            BASE_URL: `http://host.docker.internal:${server.port}/v1/${endpoint}`,
            API_KEY: "fixture-key",
            MODEL: "fixture-model",
          },
        );
        const code = await run(opts, root, new AbortController().signal);
        const report = await Bun.file(opts.output + "/report.json").json();
        report.verificationKind = "controlled_fixture";
        await Bun.write(
          opts.output + "/report.json",
          JSON.stringify(report, null, 2),
        );
        expect(code, `Fixture report: ${opts.output}/report.json`).toBe(0);
      }
    } finally {
      server.stop(true);
    }
  },
  1800000,
);

test.skipIf(!process.env.E2E_DOCKER_IMAGE)(
  "interruption returns 130 and removes container resources",
  async () => {
    const controller = new AbortController();
    const server = Bun.serve({
      hostname: "0.0.0.0",
      port: 0,
      fetch() {
        setTimeout(() => controller.abort(), 100);
        return new Response(new ReadableStream(), {
          headers: { "content-type": "text/event-stream" },
        });
      },
    });
    const root = path.resolve(import.meta.dir, "..");
    const opts = await options(
      [
        "--monoize-image",
        process.env.E2E_DOCKER_IMAGE!,
        "--harness",
        "codex-responses-ws-v2",
        "--task",
        "Run a shell tool.",
        "--timeout",
        "60",
      ],
      root,
      {
        BASE_URL: `http://host.docker.internal:${server.port}/v1/responses`,
        API_KEY: "fixture-key",
        MODEL: "fixture-model",
      },
    );
    try {
      expect(await run(opts, root, controller.signal)).toBe(130);
      const report = await Bun.file(opts.output + "/report.json").json();
      expect(report.results[0].category).toBe("interrupted");
      expect(report.results[0].cleanupErrors).toEqual([]);
      for (const resource of [
        ["container", "ls", "-a"],
        ["network", "ls"],
        ["volume", "ls"],
      ]) {
        const names = await command([
          "docker",
          ...resource,
          "--format",
          resource[0] === "container" ? "{{.Names}}" : "{{.Name}}",
        ]);
        expect(names.includes(report.resourcePrefix)).toBe(false);
      }
    } finally {
      server.stop(true);
    }
  },
  120000,
);

test.skipIf(!process.env.E2E_DOCKER_IMAGE)(
  "built-in repository acceptance with six real clients and a controlled upstream",
  async () => {
    const root = path.resolve(import.meta.dir, "..");
    const repo = "https://github.com/Ikaleio/Monoize";
    const commit = (await command(["git", "ls-remote", repo, "HEAD"])).split(
      /\s+/,
    )[0]!;
    const document =
      "# Entry points\n`src/main.rs`\n# Modules\n`src/urp/mod.rs`\n# Request flow\n`src/app.rs`\n# Sources\n`src/main.rs` `src/urp/mod.rs` `src/app.rs`\n";
    const server = fixture(0, [
      `git clone ${repo} /work/repo`,
      `git -C /work/repo checkout --detach ${commit}`,
      "cat -- /work/repo/src/main.rs",
      "cat -- /work/repo/src/urp/mod.rs",
      "cat -- /work/repo/src/app.rs",
      `printf '%s' '${document}' > /work/architecture.md`,
    ]);
    try {
      const opts = await options(
        [
          "--monoize-image",
          process.env.E2E_DOCKER_IMAGE!,
          "--ref",
          commit,
          "--jobs",
          "2",
          "--timeout",
          "180",
          "--max-requests",
          "12",
        ],
        root,
        {
          BASE_URL: `http://host.docker.internal:${server.port}/v1/chat/completions`,
          API_KEY: "fixture-key",
          MODEL: "fixture-model",
        },
      );
      expect(
        await run(opts, root, new AbortController().signal),
        opts.output,
      ).toBe(0);
    } finally {
      server.stop(true);
    }
  },
  900000,
);
