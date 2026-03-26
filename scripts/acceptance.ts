import process from "node:process";
import { parseAcceptanceArgs } from "./lib/args";
import { writeGitHubOutput } from "./lib/github";
import { runAcceptanceChecks } from "./lib/http-checks";
import { runMain } from "./lib/runtime";

async function main(): Promise<void> {
  const startedAt = Date.now();
  const config = parseAcceptanceArgs(process.argv.slice(2));

  await runAcceptanceChecks(config.url.toString());

  writeGitHubOutput(process.env, {
    duration_seconds: Math.round((Date.now() - startedAt) / 1000),
  });
}

void runMain(async () => {
  await main();
});
