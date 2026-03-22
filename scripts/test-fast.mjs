import { fileURLToPath, pathToFileURL } from "node:url";
import process from "node:process";
import { runCommandsParallel, writeGitHubOutput } from "./shared.mjs";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function runFastChecks() {
  const startedAt = Date.now();

  await runCommandsParallel([
    {
      command: pnpmCommand,
      args: ["run", "lint"],
      cwd: rootDir,
      label: "pnpm run lint",
    },
    {
      command: pnpmCommand,
      args: ["run", "format:check"],
      cwd: rootDir,
      label: "pnpm run format:check",
    },
    {
      command: pnpmCommand,
      args: ["run", "typecheck"],
      cwd: rootDir,
      label: "pnpm run typecheck",
    },
    {
      command: pnpmCommand,
      args: ["run", "test:unit"],
      cwd: rootDir,
      label: "pnpm run test:unit",
    },
    {
      command: pnpmCommand,
      args: ["run", "package:python"],
      cwd: rootDir,
      label: "pnpm run package:python",
    },
  ]);

  const durationSeconds = Math.round((Date.now() - startedAt) / 1000);
  writeGitHubOutput({
    duration_seconds: durationSeconds,
  });
  console.log(`Fast checks finished in ${durationSeconds}s`);
}

export { runFastChecks };

if (
  process.argv[1] &&
  pathToFileURL(process.argv[1]).href === import.meta.url
) {
  void runFastChecks().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
