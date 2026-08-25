#!/usr/bin/env node

import {
  currentNativeTarget,
  formatLauncherError,
  launchNativeExecutable,
  resolveNativeExecutable,
} from "./launcher.ts";

try {
  const target = currentNativeTarget();
  const executable = resolveNativeExecutable(target);
  const result = await launchNativeExecutable(executable, process.argv.slice(2));

  if (result.type === "signal") {
    process.kill(process.pid, result.signal);
  } else {
    process.exit(result.exitCode);
  }
} catch (error) {
  console.error(formatLauncherError(error));
  process.exit(1);
}
