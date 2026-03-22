import { fileURLToPath } from "node:url";
import process from "node:process";
import { createJsonTag, fetchTags, tagExists } from "./git-tags.mjs";
import { requireEnv, writeGitHubOutput, writeJsonFile } from "./shared.mjs";

const rootDir = fileURLToPath(new URL("..", import.meta.url));

function createReleaseTag(candidateSha) {
  const timestamp = new Date()
    .toISOString()
    .replaceAll(/[-:]/g, "")
    .replace(/\.\d{3}Z$/, "Z");
  return `release-${timestamp.replace("T", "").replace("Z", "")}-${candidateSha.slice(0, 12)}`;
}

async function main() {
  const candidateSha = requireEnv("CANDIDATE_SHA");
  const releaseTag = createReleaseTag(candidateSha);
  const manifestPath = fileURLToPath(
    new URL("../release-manifest.json", import.meta.url),
  );
  const manifest = {
    apiRef: requireEnv("API_REF"),
    apiRevision: process.env.PRODUCTION_API_REVISION ?? "",
    candidateSha,
    candidateTag: requireEnv("CANDIDATE_TAG"),
    deployedAt: new Date().toISOString(),
    environment: "production",
    releaseTag,
    webRef: requireEnv("WEB_REF"),
    webRevision: process.env.PRODUCTION_WEB_REVISION ?? "",
    webUrl: process.env.PRODUCTION_WEB_URL ?? "",
  };

  await fetchTags(rootDir);

  if (await tagExists(releaseTag, rootDir)) {
    throw new Error(`Release tag ${releaseTag} already exists.`);
  }

  await createJsonTag({
    cwd: rootDir,
    payload: manifest,
    tag: releaseTag,
    targetRef: candidateSha,
  });

  writeJsonFile(manifestPath, manifest);
  writeGitHubOutput({
    release_manifest: manifestPath,
    release_tag: releaseTag,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
