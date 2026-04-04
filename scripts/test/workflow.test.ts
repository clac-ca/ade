import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

test("acceptance stage reuses prebuilt candidate and fixtures", () => {
  const workflow = readFileSync(
    join(repoRoot, ".github/workflows/platform-development-pipeline.yml"),
    "utf8",
  );

  assert.match(workflow, /Build acceptance session-pool-emulator image/);
  assert.match(workflow, /Prepare acceptance fixtures/);
  assert.match(workflow, /Upload acceptance fixtures/);
  assert.match(workflow, /Download acceptance fixtures/);
  assert.match(workflow, /path: \.package\/configs/);
  assert.match(workflow, /Upload acceptance session-pool-emulator image/);
  assert.match(workflow, /Download acceptance session-pool-emulator image/);
  assert.match(workflow, /Load acceptance session-pool-emulator image/);
  assert.match(workflow, /ADE_SESSION_POOL_EMULATOR_IMAGE/);
  assert.match(workflow, /uses: docker\/build-push-action@/);
  assert.match(workflow, /context: infra\/local\/session-pool-emulator/);
  assert.match(
    workflow,
    /file: infra\/local\/session-pool-emulator\/Dockerfile/,
  );
  assert.doesNotMatch(workflow, /ghcr\.io\/.*ade-sessionpool/);
  assert.doesNotMatch(workflow, /acceptance_stage:[\s\S]*pnpm build/);
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
  assert.match(
    workflow,
    /PRODUCTION_MIGRATION_JOB_NAME: job-ade-migrate-prod-cc-002/,
  );
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
