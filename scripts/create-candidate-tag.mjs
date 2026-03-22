import { fileURLToPath } from "node:url";
import process from "node:process";
import { createJsonTag, fetchTags, tagExists } from "./git-tags.mjs";
import { requireEnv, writeGitHubOutput } from "./shared.mjs";

const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function main() {
  const candidateSha = requireEnv("CANDIDATE_SHA");
  const candidateTag = `candidate-${candidateSha}`;

  await fetchTags(rootDir);

  if (await tagExists(candidateTag, rootDir)) {
    console.log(`${candidateTag} already exists`);
    writeGitHubOutput({
      candidate_tag: candidateTag,
    });
    return;
  }

  await createJsonTag({
    cwd: rootDir,
    payload: {
      acceptance: {
        apiRevision: process.env.ACCEPTANCE_API_REVISION ?? "",
        environment: "acceptance",
        webRevision: process.env.ACCEPTANCE_WEB_REVISION ?? "",
        webUrl: process.env.ACCEPTANCE_WEB_URL ?? "",
      },
      acceptedAt: new Date().toISOString(),
      apiRef: requireEnv("API_REF"),
      candidateSha,
      webRef: requireEnv("WEB_REF"),
    },
    tag: candidateTag,
    targetRef: candidateSha,
  });

  writeGitHubOutput({
    candidate_tag: candidateTag,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
