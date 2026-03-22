import { fileURLToPath } from "node:url";
import process from "node:process";
import { execFileSync } from "node:child_process";
import {
  runCommand,
  runCommandCapture,
  writeGitHubOutput,
  writeJsonFile,
} from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

function readArg(name) {
  const flag = `--${name}`;
  const index = process.argv.indexOf(flag);

  if (index === -1) {
    return null;
  }

  return process.argv[index + 1] ?? null;
}

function readGitSha() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: rootDir,
      encoding: "utf8",
    }).trim();
  } catch {
    throw new Error("Could not determine candidate SHA from git.");
  }
}

function readRepository() {
  const explicit = readArg("repository");

  if (explicit) {
    return explicit;
  }

  const repository = process.env.GITHUB_REPOSITORY;

  if (!repository) {
    throw new Error(
      "GITHUB_REPOSITORY is required unless --repository is provided.",
    );
  }

  return repository;
}

async function inspectDigest(imageRef) {
  const { stdout } = await runCommandCapture(
    dockerCommand,
    ["image", "inspect", imageRef, "--format={{index .RepoDigests 0}}"],
    {
      cwd: rootDir,
    },
  );
  return stdout.trim();
}

async function main() {
  const candidateSha = readArg("sha") ?? process.env.GITHUB_SHA ?? readGitSha();
  const repository = readRepository();
  const [owner, repoName] = repository.split("/");

  if (!owner || !repoName) {
    throw new Error(
      `Expected repository in owner/name form, received: ${repository}`,
    );
  }

  const normalizedOwner = owner.toLowerCase();
  const normalizedRepo = repoName.toLowerCase();
  const webCandidateTag = `ghcr.io/${normalizedOwner}/${normalizedRepo}-web:sha-${candidateSha}`;
  const apiCandidateTag = `ghcr.io/${normalizedOwner}/${normalizedRepo}-api:sha-${candidateSha}`;

  await runCommand(dockerCommand, ["tag", "ade-web:local", webCandidateTag], {
    cwd: rootDir,
  });
  await runCommand(dockerCommand, ["tag", "ade-api:local", apiCandidateTag], {
    cwd: rootDir,
  });
  await runCommand(dockerCommand, ["push", webCandidateTag], {
    cwd: rootDir,
  });
  await runCommand(dockerCommand, ["push", apiCandidateTag], {
    cwd: rootDir,
  });

  const webRef = await inspectDigest(webCandidateTag);
  const apiRef = await inspectDigest(apiCandidateTag);
  const manifestPath = fileURLToPath(
    new URL("../candidate-manifest.json", import.meta.url),
  );
  const manifest = {
    apiRef,
    apiTag: apiCandidateTag,
    candidateSha,
    commitStageDurationSeconds: Number.parseInt(
      process.env.COMMIT_STAGE_DURATION_SECONDS ?? "0",
      10,
    ),
    publishedAt: new Date().toISOString(),
    webRef,
    webTag: webCandidateTag,
  };

  writeJsonFile(manifestPath, manifest);
  writeGitHubOutput({
    api_ref: apiRef,
    candidate_manifest: manifestPath,
    candidate_sha: candidateSha,
    web_ref: webRef,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
