import { fileURLToPath } from "node:url";
import process from "node:process";
import { fetchTags, readJsonTag, tagExists } from "./git-tags.mjs";
import { writeGitHubOutput } from "./shared.mjs";

const rootDir = fileURLToPath(new URL("..", import.meta.url));

function readCandidateTag() {
  const index = process.argv.indexOf("--tag");

  if (index === -1) {
    throw new Error("Missing required argument: --tag");
  }

  return process.argv[index + 1] ?? "";
}

async function main() {
  const candidateTag = readCandidateTag();

  if (!/^candidate-[0-9a-f]{40}$/.test(candidateTag)) {
    throw new Error(
      "candidate tag must match candidate-<40-character lowercase commit SHA>.",
    );
  }

  await fetchTags(rootDir);

  if (!(await tagExists(candidateTag, rootDir))) {
    throw new Error(`Accepted candidate tag ${candidateTag} does not exist.`);
  }

  const payload = await readJsonTag(candidateTag, rootDir);

  if (!payload.candidateSha || !payload.webRef || !payload.apiRef) {
    throw new Error(
      "Candidate tag metadata must include candidateSha, webRef, and apiRef.",
    );
  }

  writeGitHubOutput({
    api_ref: payload.apiRef,
    candidate_sha: payload.candidateSha,
    candidate_tag: candidateTag,
    web_ref: payload.webRef,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
