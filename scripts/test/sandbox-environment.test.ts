import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const sandboxEnvironmentDir = join(
  repoRoot,
  "apps/ade-api/sandbox-environment",
);

test("sandbox environment stays app-owned and does not become a package", () => {
  assert.ok(existsSync(join(sandboxEnvironmentDir, "README.md")));
  assert.ok(existsSync(join(sandboxEnvironmentDir, "python-version.txt")));
  assert.ok(
    existsSync(join(sandboxEnvironmentDir, "rootfs/app/ade/bin/setup.sh")),
  );
  assert.ok(existsSync(join(sandboxEnvironmentDir, "build.ts")));

  assert.equal(existsSync(join(sandboxEnvironmentDir, "package.json")), false);
  assert.equal(existsSync(join(sandboxEnvironmentDir, "Cargo.toml")), false);
  assert.equal(
    existsSync(join(repoRoot, "packages/sandbox-environment")),
    false,
  );
});

test("root sandbox-environment command stays a thin wrapper", () => {
  const packageJson = JSON.parse(
    readFileSync(join(repoRoot, "package.json"), "utf8"),
  ) as {
    scripts?: Record<string, string>;
  };
  const wrapperSource = readFileSync(
    join(repoRoot, "scripts/build-sandbox-environment.ts"),
    "utf8",
  );

  assert.equal(
    packageJson.scripts?.["build:sandbox-environment"],
    "tsx scripts/build-sandbox-environment.ts",
  );
  assert.match(
    wrapperSource,
    /\.\.\/apps\/ade-api\/sandbox-environment\/build/,
  );
  assert.doesNotMatch(wrapperSource, /dockerCommand|pythonToolchainImage/);
});

test("sandbox environment build stays focused on the shared runtime tarball", () => {
  const buildSource = readFileSync(
    join(sandboxEnvironmentDir, "build.ts"),
    "utf8",
  );

  assert.doesNotMatch(buildSource, /ADE_CONFIG_FIXTURE_ROOT|configFixtureRoot/);
  assert.doesNotMatch(buildSource, /packages\/ade-config/);
  assert.match(buildSource, /sandbox-environment\.tar\.gz/);
});
