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
  assert.ok(
    existsSync(join(repoRoot, "apps/ade-api/crates/reverse-connect/Cargo.toml")),
  );
  assert.equal(
    existsSync(join(repoRoot, "packages/reverse-connect/Cargo.toml")),
    false,
  );
});

test("sandbox-environment build stays internal to the main build flow", () => {
  const packageJson = JSON.parse(
    readFileSync(join(repoRoot, "package.json"), "utf8"),
  ) as {
    scripts?: Record<string, string>;
  };
  const buildSource = readFileSync(join(repoRoot, "scripts/build.ts"), "utf8");

  assert.equal(
    "build:sandbox-environment" in (packageJson.scripts ?? {}),
    false,
  );
  assert.match(buildSource, /readPinnedPythonVersion/);
  assert.doesNotMatch(buildSource, /buildSandboxEnvironmentAssets/);
});

test("sandbox environment build stays focused on the shared runtime tarball", () => {
  const buildSource = readFileSync(
    join(sandboxEnvironmentDir, "build.ts"),
    "utf8",
  );
  const dockerfileSource = readFileSync(join(repoRoot, "Dockerfile"), "utf8");

  assert.doesNotMatch(buildSource, /ADE_CONFIG_FIXTURE_ROOT|configFixtureRoot/);
  assert.doesNotMatch(buildSource, /packages\/ade-config/);
  assert.match(buildSource, /buildx",\s*"build/);
  assert.match(buildSource, /--platform",\s*readSandboxBuildPlatform/);
  assert.match(buildSource, /--target",\s*"sandbox-environment-artifact/);
  assert.match(dockerfileSource, /FROM scratch AS sandbox-environment-artifact/);
  assert.match(
    dockerfileSource,
    /COPY --from=sandbox-environment-builder \/out\/sandbox-environment\.tar\.gz \/sandbox-environment\.tar\.gz/,
  );
  assert.match(dockerfileSource, /--mount=type=cache,id=ade-rust-target/);
  assert.doesNotMatch(dockerfileSource, /packages\/reverse-connect\/Dockerfile\.build/);
  assert.doesNotMatch(dockerfileSource, /COPY --chown=ade:ade \.package\/sandbox-environment\.tar\.gz/);
  assert.match(buildSource, /sandbox-environment\.tar\.gz/);
});
