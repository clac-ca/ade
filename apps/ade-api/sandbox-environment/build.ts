import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createConsoleLogger, runMain } from "../../../scripts/lib/runtime";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const tarCommand = process.platform === "win32" ? "tar.exe" : "tar";
const rootDir = fileURLToPath(new URL("../../../", import.meta.url));
const sandboxEnvironmentSourceDir = fileURLToPath(new URL("./", import.meta.url));
const sandboxEnvironmentSourceRootfs = join(
  sandboxEnvironmentSourceDir,
  "rootfs",
);
const sandboxEnvironmentOutputArchive = fileURLToPath(
  new URL("../../../.package/sandbox-environment.tar.gz", import.meta.url),
);
const sandboxEnvironmentOutputStamp = fileURLToPath(
  new URL("../../../.package/sandbox-environment.stamp", import.meta.url),
);
const legacySandboxEnvironmentOutputDir = fileURLToPath(
  new URL("../../../.package/sandbox-environment", import.meta.url),
);
const configFixtureRoot = fileURLToPath(
  new URL("../../../.package/configs", import.meta.url),
);
const pythonVersionPath = join(
  sandboxEnvironmentSourceDir,
  "python-version.txt",
);
const sandboxEnvironmentBuildScriptPath = fileURLToPath(new URL("./build.ts", import.meta.url));
const pythonToolchainImage = "python:3.12.11-slim-bullseye";
const sampleScopes = [
  ["workspace-a", "config-v1"],
  ["workspace-b", "config-v2"],
] as const;
const configFixtureStamp = join(configFixtureRoot, ".stamp");

function readPinnedPythonVersion(): string {
  const version = readFileSync(pythonVersionPath, "utf8").trim();
  if (version === "") {
    throw new Error(
      `Pinned Python version file ${pythonVersionPath} must not be empty.`,
    );
  }
  return version;
}

function pythonToolchainName(version: string): string {
  return `python-${version}-linux-x86_64.tar.gz`;
}

function newestWheel(directoryPath: string, prefix: string): string {
  const candidates = readdirSync(directoryPath)
    .filter((name) => name.startsWith(prefix) && name.endsWith(".whl"))
    .map((name) => ({
      modifiedMs: statSync(join(directoryPath, name)).mtimeMs,
      path: join(directoryPath, name),
    }))
    .sort((left, right) => right.modifiedMs - left.modifiedMs);

  const latest = candidates[0];
  if (latest === undefined) {
    throw new Error(`No wheel found in ${directoryPath} with prefix ${prefix}`);
  }

  return latest.path;
}

function newestMtime(path: string): number {
  const stats = statSync(path);
  if (!stats.isDirectory()) {
    return stats.mtimeMs;
  }

  let latest = stats.mtimeMs;
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    if (
      entry.name === "dist" ||
      entry.name === "__pycache__" ||
      entry.name === ".pytest_cache" ||
      entry.name === ".venv"
    ) {
      continue;
    }
    latest = Math.max(latest, newestMtime(join(path, entry.name)));
  }
  return latest;
}

function wheelNeedsBuild(packageDir: string, wheelPrefix: string): boolean {
  const distDir = join(packageDir, "dist");
  if (!existsSync(distDir)) {
    return true;
  }

  try {
    const wheelPath = newestWheel(distDir, wheelPrefix);
    return newestMtime(packageDir) > statSync(wheelPath).mtimeMs;
  } catch {
    return true;
  }
}

function ensureWheel(packageDir: string): void {
  if (
    !wheelNeedsBuild(packageDir, basename(packageDir).replace(/-/g, "_") + "-")
  ) {
    return;
  }

  execFileSync(
    pnpmCommand,
    ["exec", "uv", "build", "--directory", packageDir],
    {
      cwd: rootDir,
      stdio: "inherit",
    },
  );
}

function buildPythonToolchain(
  outputDirectory: string,
  archiveName: string,
): void {
  execFileSync(
    dockerCommand,
    [
      "run",
      "--rm",
      "--platform",
      "linux/amd64",
      "--volume",
      `${outputDirectory}:/out`,
      pythonToolchainImage,
      "sh",
      "-lc",
      `tar -C /usr/local -czf /out/${archiveName} .`,
    ],
    {
      cwd: rootDir,
      stdio: "inherit",
    },
  );
}

function stagePythonToolchain(
  outputDirectory: string,
  version: string,
): void {
  const tempRoot = mkdtempSync(join(tmpdir(), "ade-python-toolchain-"));
  const archiveName = pythonToolchainName(version);
  const archivePath = join(tempRoot, archiveName);

  try {
    buildPythonToolchain(tempRoot, archiveName);
    mkdirSync(outputDirectory, {
      recursive: true,
    });
    execFileSync(tarCommand, ["-xzf", archivePath, "-C", outputDirectory], {
      cwd: rootDir,
      stdio: "inherit",
    });
  } finally {
    rmSync(tempRoot, {
      force: true,
      recursive: true,
    });
  }
}

function buildBaseWheelhouse(
  engineWheelPath: string,
  outputDirectory: string,
): void {
  execFileSync(
    dockerCommand,
    [
      "run",
      "--rm",
      "--volume",
      `${dirname(engineWheelPath)}:/wheel-src`,
      "--volume",
      `${outputDirectory}:/out`,
      pythonToolchainImage,
      "sh",
      "-lc",
      `python -m pip download --dest /out /wheel-src/${basename(engineWheelPath)}`,
    ],
    {
      cwd: rootDir,
      stdio: "inherit",
    },
  );
}

function buildReverseConnectBinary(outputDirectory: string): void {
  execFileSync(
    dockerCommand,
    [
      "run",
      "--rm",
      "--platform",
      "linux/amd64",
      "--volume",
      `${rootDir}:/workspace`,
      "--volume",
      `${outputDirectory}:/out`,
      "--workdir",
      "/workspace",
      "--env",
      "CARGO_TARGET_DIR=/tmp/target",
      "rust:1.94.1-alpine",
      "sh",
      "-lc",
      "apk add --no-cache build-base musl-dev pkgconfig perl >/dev/null && /usr/local/cargo/bin/cargo build --locked --package reverse-connect --bin reverse-connect --release && cp /tmp/target/release/reverse-connect /out/reverse-connect",
    ],
    {
      cwd: rootDir,
      stdio: "inherit",
    },
  );
}

function sandboxEnvironmentInputs(): number {
  return Math.max(
    newestMtime(join(rootDir, "Cargo.toml")),
    newestMtime(join(rootDir, "Cargo.lock")),
    newestMtime(join(rootDir, "packages/reverse-connect")),
    newestMtime(join(rootDir, "packages/ade-engine")),
    newestMtime(sandboxEnvironmentSourceRootfs),
    newestMtime(sandboxEnvironmentBuildScriptPath),
    newestMtime(pythonVersionPath),
  );
}

function localConfigFixtureInputs(): number {
  return newestMtime(join(rootDir, "packages/ade-config"));
}

function needsSandboxEnvironmentBuild(): boolean {
  if (
    !existsSync(sandboxEnvironmentOutputArchive) ||
    !existsSync(sandboxEnvironmentOutputStamp)
  ) {
    return true;
  }

  return (
    sandboxEnvironmentInputs() > statSync(sandboxEnvironmentOutputStamp).mtimeMs
  );
}

function needsLocalConfigFixtureBuild(): boolean {
  if (!existsSync(configFixtureStamp)) {
    return true;
  }

  return localConfigFixtureInputs() > statSync(configFixtureStamp).mtimeMs;
}

function stageLocalConfigFixtures(configWheelPath: string): void {
  if (!needsLocalConfigFixtureBuild()) {
    return;
  }

  rmSync(configFixtureRoot, {
    force: true,
    recursive: true,
  });

  for (const [workspaceId, configVersionId] of sampleScopes) {
    const scopeDirectory = join(configFixtureRoot, workspaceId, configVersionId);
    mkdirSync(scopeDirectory, { recursive: true });
    cpSync(configWheelPath, join(scopeDirectory, basename(configWheelPath)));
  }

  writeFileSync(
    configFixtureStamp,
    JSON.stringify({
      builtAt: new Date().toISOString(),
      configWheel: basename(configWheelPath),
    }),
  );
}

function buildSandboxEnvironmentAssets(logger = createConsoleLogger()): void {
  ensureWheel(join(rootDir, "packages/ade-engine"));
  ensureWheel(join(rootDir, "packages/ade-config"));

  const engineWheelPath = newestWheel(
    join(rootDir, "packages/ade-engine/dist"),
    "ade_engine-",
  );
  const configWheelPath = newestWheel(
    join(rootDir, "packages/ade-config/dist"),
    "ade_config-",
  );

  stageLocalConfigFixtures(configWheelPath);

  if (!needsSandboxEnvironmentBuild()) {
    logger.info("Sandbox environment archive is current");
    return;
  }

  const pythonVersion = readPinnedPythonVersion();
  const tempRoot = mkdtempSync(join(tmpdir(), "ade-sandbox-environment-"));
  const outputBinDir = join(tempRoot, "app/ade/bin");
  const outputPythonDir = join(tempRoot, "mnt/data/ade/python/current");
  const outputWheelhouseDir = join(tempRoot, "app/ade/wheelhouse/base");

  rmSync(legacySandboxEnvironmentOutputDir, {
    force: true,
    recursive: true,
  });
  rmSync(sandboxEnvironmentOutputArchive, {
    force: true,
  });
  mkdirSync(dirname(sandboxEnvironmentOutputArchive), {
    recursive: true,
  });
  for (const entry of readdirSync(sandboxEnvironmentSourceRootfs, {
    withFileTypes: true,
  })) {
    cpSync(
      join(sandboxEnvironmentSourceRootfs, entry.name),
      join(tempRoot, entry.name),
      {
        recursive: true,
      },
    );
  }
  mkdirSync(outputBinDir, {
    recursive: true,
  });
  mkdirSync(outputPythonDir, {
    recursive: true,
  });
  mkdirSync(outputWheelhouseDir, {
    recursive: true,
  });

  try {
    buildReverseConnectBinary(outputBinDir);
    buildBaseWheelhouse(engineWheelPath, outputWheelhouseDir);
    stagePythonToolchain(outputPythonDir, pythonVersion);
    execFileSync(
      tarCommand,
      ["-C", tempRoot, "-czf", sandboxEnvironmentOutputArchive, "app", "mnt"],
      {
        cwd: rootDir,
        stdio: "inherit",
      },
    );
  } finally {
    rmSync(tempRoot, {
      force: true,
      recursive: true,
    });
  }

  writeFileSync(
    sandboxEnvironmentOutputStamp,
    JSON.stringify({
      archive: basename(sandboxEnvironmentOutputArchive),
      builtAt: new Date().toISOString(),
      engineWheel: basename(engineWheelPath),
      pythonVersion,
    }),
  );
  logger.info("Built sandbox environment archive");
}

export { buildSandboxEnvironmentAssets };

if (
  process.argv[1] &&
  pathToFileURL(process.argv[1]).href === import.meta.url
) {
  void runMain(async () => {
    buildSandboxEnvironmentAssets();
  });
}
