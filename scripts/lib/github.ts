import { appendFileSync } from "node:fs";
import { readOptionalTrimmedString } from "./runtime";

function writeGitHubOutput(
  env: Record<string, string | undefined>,
  values: Record<string, string | number>,
) {
  const outputPath = readOptionalTrimmedString(env, "GITHUB_OUTPUT");

  if (!outputPath) {
    return;
  }

  const lines = Object.entries(values).map(
    ([key, value]) => `${key}=${String(value)}`,
  );
  appendFileSync(outputPath, `${lines.join("\n")}\n`);
}

export { writeGitHubOutput };
