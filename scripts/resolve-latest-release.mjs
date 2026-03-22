import { fileURLToPath } from "node:url";
import process from "node:process";
import { fetchTags, listTags, readJsonTag } from "./git-tags.mjs";
import { writeGitHubOutput } from "./shared.mjs";

const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function main() {
  await fetchTags(rootDir);

  const [latestReleaseTag] = await listTags("release-*", rootDir);

  if (!latestReleaseTag) {
    writeGitHubOutput({
      found: "false",
    });
    return;
  }

  const payload = await readJsonTag(latestReleaseTag, rootDir);

  if (!payload.webRef || !payload.apiRef || !payload.candidateSha) {
    throw new Error(
      `Release tag ${latestReleaseTag} is missing required metadata.`,
    );
  }

  writeGitHubOutput({
    api_ref: payload.apiRef,
    candidate_sha: payload.candidateSha,
    found: "true",
    release_tag: latestReleaseTag,
    web_ref: payload.webRef,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
