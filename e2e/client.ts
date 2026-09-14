import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { launch, type ClientConfig } from "./harness";
import { BUDGET } from "./core";
const config: ClientConfig = JSON.parse(
  await readFile("/work/client-config.json", "utf8"),
);
const { args, env, files } = launch(config);
for (const [file, contents] of Object.entries(files)) {
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, contents, { mode: 0o600 });
}
let total = 0;
let exceeded = false;
const child = Bun.spawn(args, {
  env,
  cwd: "/work",
  stdin: "ignore",
  stdout: "pipe",
  stderr: "pipe",
});
const drain = async (stream: ReadableStream<Uint8Array>, file: string) => {
  const writer = Bun.file(file).writer();
  try {
    for await (const chunk of stream) {
      total += chunk.length;
      if (total > BUDGET) {
        exceeded = true;
        child.kill("SIGKILL");
        break;
      }
      writer.write(chunk);
    }
  } finally {
    await writer.end();
  }
};
await Promise.all([
  drain(child.stdout, "/work/client.jsonl"),
  drain(child.stderr, "/work/client.stderr"),
]);
const exitCode = await child.exited;
await writeFile(
  "/work/client-exit.json",
  JSON.stringify({ exitCode, exceeded }),
);
process.exit(exitCode);
