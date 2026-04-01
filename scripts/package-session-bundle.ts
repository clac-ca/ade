import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runMain } from "./lib/runtime";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const bundleRoot = fileURLToPath(
  new URL("../.package/session-bundle", import.meta.url),
);
const configRoot = fileURLToPath(
  new URL("../.package/session-configs", import.meta.url),
);
const prepareScriptPath = fileURLToPath(
  new URL(
    "../apps/ade-api/assets/session-bundle/bin/prepare.sh",
    import.meta.url,
  ),
);
const runScriptPath = fileURLToPath(
  new URL("../apps/ade-api/assets/session-bundle/bin/run.py", import.meta.url),
);
const engineWheelPath = newestWheel(
  fileURLToPath(new URL("../packages/ade-engine/dist", import.meta.url)),
  "ade_engine-",
);
const configWheelPath = newestWheel(
  fileURLToPath(new URL("../packages/ade-config/dist", import.meta.url)),
  "ade_config-",
);
const pythonToolchainImage = "python:3.12.11-slim-bullseye";
const pythonToolchainName = "python-3.12.11-linux-x86_64.tar.gz";
const sampleScopes = [
  ["workspace-a", "config-v1"],
  ["workspace-b", "config-v2"],
] as const;

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

function buildPythonToolchain(outputDirectory: string): void {
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
      `tar -C /usr/local -czf /out/${pythonToolchainName} .`,
    ],
    {
      cwd: rootDir,
      stdio: "inherit",
    },
  );
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

function main(): void {
  rmSync(bundleRoot, {
    force: true,
    recursive: true,
  });
  rmSync(configRoot, {
    force: true,
    recursive: true,
  });
  mkdirSync(`${bundleRoot}/bin`, {
    recursive: true,
  });
  mkdirSync(`${bundleRoot}/python`, {
    recursive: true,
  });
  mkdirSync(`${bundleRoot}/wheelhouse/base`, {
    recursive: true,
  });

  buildReverseConnectBinary(`${bundleRoot}/bin`);
  copyFileSync(prepareScriptPath, `${bundleRoot}/bin/prepare.sh`);
  copyFileSync(runScriptPath, `${bundleRoot}/bin/run.py`);
  buildBaseWheelhouse(engineWheelPath, `${bundleRoot}/wheelhouse/base`);
  buildPythonToolchain(`${bundleRoot}/python`);

  for (const [workspaceId, configVersionId] of sampleScopes) {
    const scopeDirectory = join(configRoot, workspaceId, configVersionId);
    mkdirSync(scopeDirectory, { recursive: true });
    copyFileSync(configWheelPath, join(scopeDirectory, basename(configWheelPath)));
  }
}

void runMain(() => {
  main();
});
