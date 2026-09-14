#!/usr/bin/env bun
import { fileURLToPath } from "node:url";
import path from "node:path";
import { HARNESSES, options, protocol, redact } from "./core";
import { run } from "./runner";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const [subcommand, ...args] = process.argv.slice(2);
if (subcommand === "list") {
  if (args.length) {
    console.error("list accepts no arguments");
    process.exit(2);
  }
  for (const h of HARNESSES) console.log(`${h}\t${protocol(h)}`);
} else if (subcommand === "run") {
  const controller = new AbortController();
  const interrupt = () => controller.abort(new Error("interrupted"));
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", interrupt);
  try {
    process.exitCode = await run(
      await options(args, root),
      root,
      controller.signal,
    );
  } catch (e) {
    console.error(redact(String(e), [process.env.API_KEY ?? ""]));
    process.exitCode = controller.signal.aborted ? 130 : 2;
  } finally {
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", interrupt);
  }
} else {
  console.log(
    "Usage: bun e2e/cli.ts list | run [options]\n--harness ID[,ID] (repeatable; default all)\n--base-url URL --api-key KEY --model MODEL --upstream-type TYPE\n--jobs N --timeout SECONDS --max-requests N\n--task TEXT | --task-file FILE\n--repo HTTPS_URL --ref REF --monoize-image IMAGE --output DIRECTORY",
  );
  if (subcommand !== undefined && subcommand !== "--help") process.exitCode = 2;
}
