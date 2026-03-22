import { fileURLToPath } from "node:url";
import process from "node:process";
import { runFastChecks } from "./test-fast.mjs";
import { runCommand, writeGitHubOutput } from "./shared.mjs";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function main() {
  const startedAt = Date.now();

  await runFastChecks();
  await runCommand(pnpmCommand, ["run", "build"], {
    cwd: rootDir,
  });
  await runCommand(pnpmCommand, ["run", "test:smoke"], {
    cwd: rootDir,
  });

  const durationSeconds = Math.round((Date.now() - startedAt) / 1000);
  writeGitHubOutput({
    duration_seconds: durationSeconds,
  });
  console.log(`Commit gate finished in ${durationSeconds}s`);
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
