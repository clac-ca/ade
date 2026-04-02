import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

test("acceptance stage reuses prebuilt candidate and fixtures", () => {
  const workflow = readFileSync(
    join(
      repoRoot,
      ".github/workflows/platform-development-pipeline.yml",
    ),
    "utf8",
  );

  assert.match(workflow, /Build acceptance session-pool image/);
  assert.match(workflow, /Prepare acceptance fixtures/);
  assert.match(workflow, /Upload acceptance fixtures/);
  assert.match(workflow, /Download acceptance fixtures/);
  assert.match(workflow, /Pull acceptance session-pool image/);
  assert.match(workflow, /ADE_SESSIONPOOL_IMAGE/);
  assert.doesNotMatch(workflow, /acceptance_stage:[\s\S]*pnpm build/);
});

test("local dependency launcher chooses session-pool compose mode from env", () => {
  const localDeps = readFileSync(join(repoRoot, "scripts/local-deps.ts"), "utf8");

  assert.match(localDeps, /compose\.sessionpool\.build\.yaml/);
  assert.match(localDeps, /compose\.sessionpool\.image\.yaml/);
  assert.match(localDeps, /ADE_SESSIONPOOL_IMAGE/);
});
