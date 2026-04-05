import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

test("commit stage uses explicit parallel test and build-candidate jobs", () => {
  const workflow = readFileSync(
    join(repoRoot, ".github/workflows/platform-development-pipeline.yml"),
    "utf8",
  );
  const commitTestMatch = workflow.match(
    /commit_test:[\s\S]*?(?=\n {2}commit_build_candidate:)/,
  );
  const commitBuildMatch = workflow.match(
    /commit_build_candidate:[\s\S]*?(?=\n {2}acceptance_stage:)/,
  );

  assert.ok(commitTestMatch, "commit test job should exist");
  assert.ok(commitBuildMatch, "commit build job should exist");

  const commitTest = commitTestMatch[0];
  const commitBuild = commitBuildMatch[0];

  assert.match(commitTest, /name: Commit Stage \/ Test/);
  assert.match(commitBuild, /name: Commit Stage \/ Build Candidate/);
  assert.match(commitTest, /uses: astral-sh\/setup-uv@/);
  assert.match(commitTest, /run: pnpm test/);
  assert.match(commitBuild, /uses: docker\/setup-buildx-action@/);
  assert.match(commitBuild, /uses: docker\/login-action@/);
  assert.match(
    commitBuild,
    /ADE_BUILD_IMAGE: \$\{\{ format\('\{0\}\/\{1\}:sha-\{2\}', env\.REGISTRY, env\.IMAGE_NAME, github\.sha\) \}\}/,
  );
  assert.match(
    commitBuild,
    /export ADE_SERVICE_VERSION="\$\{release_version\}"/,
  );
  assert.match(commitBuild, /\n\s+pnpm build/);
  assert.doesNotMatch(workflow, /commit_stage:/);
  assert.doesNotMatch(workflow, /\n\s+matrix:/);
  assert.doesNotMatch(commitBuild, /release-candidate-metadata/);
});

test("acceptance stage reuses the SHA-tagged candidate through the command surface", () => {
  const workflow = readFileSync(
    join(repoRoot, ".github/workflows/platform-development-pipeline.yml"),
    "utf8",
  );
  const acceptanceStageMatch = workflow.match(
    /acceptance_stage:[\s\S]*?(?=\n {2}release_stage:)/,
  );

  assert.ok(acceptanceStageMatch, "acceptance stage should exist");
  const acceptanceStage = acceptanceStageMatch[0];

  assert.match(
    acceptanceStage,
    /needs:\n\s+- commit_test\n\s+- commit_build_candidate/,
  );
  assert.match(acceptanceStage, /uses: astral-sh\/setup-uv@/);
  assert.match(acceptanceStage, /Install Playwright Chromium/);
  assert.match(
    acceptanceStage,
    /pnpm exec playwright install --with-deps chromium/,
  );
  assert.match(
    acceptanceStage,
    /CANDIDATE_IMAGE: \$\{\{ format\('ghcr\.io\/\{0\}\/ade-platform:sha-\{1\}', github\.repository_owner, github\.sha\) \}\}/,
  );
  assert.match(acceptanceStage, /docker pull "\$\{CANDIDATE_IMAGE\}"/);
  assert.match(
    acceptanceStage,
    /pnpm test:acceptance --image "\$\{CANDIDATE_IMAGE\}"/,
  );
  assert.doesNotMatch(acceptanceStage, /pnpm build/);
  assert.doesNotMatch(acceptanceStage, /download-artifact/);
  assert.doesNotMatch(acceptanceStage, /release-candidate-metadata/);
  assert.doesNotMatch(acceptanceStage, /ADE_SESSION_POOL_EMULATOR_IMAGE/);
  assert.doesNotMatch(
    acceptanceStage,
    /Load acceptance session-pool-emulator image/,
  );
});

test("release stage deploys without carrying the sandbox secret in GitHub", () => {
  const workflow = readFileSync(
    join(repoRoot, ".github/workflows/platform-development-pipeline.yml"),
    "utf8",
  );

  assert.doesNotMatch(
    workflow,
    /ADE_SANDBOX_ENVIRONMENT_SECRET: \$\{\{ secrets\.ADE_SANDBOX_ENVIRONMENT_SECRET \}\}/,
  );
  assert.doesNotMatch(
    workflow,
    /Skip release when sandbox secret is not configured/,
  );
  assert.doesNotMatch(workflow, /--parameters sandboxEnvironmentSecret=/);
  assert.doesNotMatch(
    workflow,
    /--parameters initialSandboxEnvironmentSecret=/,
  );
  assert.doesNotMatch(workflow, /main\.prod\.bicepparam/);
  assert.doesNotMatch(workflow, /outputs\.migrationJobName/);
  assert.doesNotMatch(workflow, /release-candidate-metadata/);
  assert.match(
    workflow,
    /PRODUCTION_MIGRATION_JOB_NAME: job-ade-migrate-prod-cc-002/,
  );
  assert.match(workflow, /client-id: \$\{\{ vars\.AZURE_DEPLOY_CLIENT_ID \}\}/);
  assert.doesNotMatch(workflow, /client-id: \$\{\{ vars\.AZURE_CLIENT_ID \}\}/);
  assert.match(
    workflow,
    /CANDIDATE_IMAGE: \$\{\{ format\('ghcr\.io\/\{0\}\/ade-platform:sha-\{1\}', github\.repository_owner, github\.sha\) \}\}/,
  );
  assert.match(workflow, /echo "RELEASE_VERSION=\$\{release_version\}"/);
  assert.match(
    workflow,
    /echo "RELEASE_TAG=ade-platform-v\$\{release_version\}"/,
  );
  assert.match(
    workflow,
    /echo "RELEASE_TITLE=ADE Platform \$\{release_version\}"/,
  );
  assert.match(workflow, /--parameters image="\$\{CANDIDATE_IMAGE\}"/);
});

test("local dependency launcher chooses session-pool-emulator compose mode from env", () => {
  const localDeps = readFileSync(
    join(repoRoot, "scripts/local-deps.ts"),
    "utf8",
  );
  const buildCompose = readFileSync(
    join(repoRoot, "infra/local/compose.session-pool-emulator.build.yaml"),
    "utf8",
  );
  const imageCompose = readFileSync(
    join(repoRoot, "infra/local/compose.session-pool-emulator.image.yaml"),
    "utf8",
  );

  assert.match(localDeps, /compose\.session-pool-emulator\.build\.yaml/);
  assert.match(localDeps, /compose\.session-pool-emulator\.image\.yaml/);
  assert.match(localDeps, /ADE_SESSION_POOL_EMULATOR_IMAGE/);
  assert.match(buildCompose, /host\.docker\.internal:host-gateway/);
  assert.match(imageCompose, /host\.docker\.internal:host-gateway/);
});
