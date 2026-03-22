import process from "node:process";
import { runAcceptanceChecks } from "./http-checks.mjs";
import { requireEnv, writeGitHubOutput } from "./shared.mjs";

async function main() {
  const startedAt = Date.now();
  const baseUrl = requireEnv("ADE_BASE_URL");

  await runAcceptanceChecks(baseUrl);

  writeGitHubOutput({
    duration_seconds: Math.round((Date.now() - startedAt) / 1000),
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
