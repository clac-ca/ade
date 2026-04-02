import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

test("root package keeps one fast test command and one acceptance command", () => {
  const packageJson = JSON.parse(
    readFileSync(join(repoRoot, "package.json"), "utf8"),
  ) as {
    scripts?: Record<string, string>;
  };
  const scripts = packageJson.scripts ?? {};
  const testScript = scripts["test"] ?? "";

  assert.match(testScript, /pnpm --filter @ade\/api test/);
  assert.match(testScript, /pnpm --filter @ade\/web test/);
  assert.match(testScript, /tsx --test scripts\/test\/\*\.test\.ts/);
  assert.match(
    testScript,
    /uv run --directory packages\/ade-engine --group test pytest/,
  );
  assert.match(testScript, /az bicep lint --file infra\/main\.bicep/);
  assert.doesNotMatch(testScript, /az bicep build --file infra\/main\.bicep/);
  assert.doesNotMatch(
    testScript,
    /az bicep build-params --file infra\/environments\/main\.prod\.bicepparam/,
  );
  assert.doesNotMatch(testScript, /build:python-artifacts/);

  assert.equal(scripts["test:acceptance"], "tsx scripts/acceptance.ts");
  assert.equal(scripts["build"], "tsx scripts/build.ts");

  assert.equal("check" in scripts, false);
  assert.equal("lint" in scripts, false);
  assert.equal("lint:python" in scripts, false);
  assert.equal("test:unit" in scripts, false);
  assert.equal("test:scripts" in scripts, false);
  assert.equal("test:python" in scripts, false);
  assert.equal("test:session:local" in scripts, false);
  assert.equal("test:session:parity" in scripts, false);
  assert.equal("typecheck" in scripts, false);
  assert.equal("build:python-artifacts" in scripts, false);
  assert.equal("build:sandbox-environment" in scripts, false);
});
