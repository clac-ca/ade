import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createConsoleLogger, runMain } from "../../../scripts/lib/runtime";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("../../../", import.meta.url));
const sandboxEnvironmentSourceDir = fileURLToPath(new URL("./", import.meta.url));
const sandboxEnvironmentOutputArchive = fileURLToPath(
  new URL("../../../.package/sandbox-environment.tar.gz", import.meta.url),
);
const defaultSandboxBuildPlatform = "linux/amd64";
const pythonVersionPath = join(
  sandboxEnvironmentSourceDir,
  "python-version.txt",
);
const dockerfilePath = join(rootDir, "Dockerfile");

function readPinnedPythonVersion(): string {
  const version = readFileSync(pythonVersionPath, "utf8").trim();
  if (version === "") {
    throw new Error(
      `Pinned Python version file ${pythonVersionPath} must not be empty.`,
    );
  }
  return version;
}

function readSandboxBuildPlatform(): string {
  return defaultSandboxBuildPlatform;
}

function buildSandboxEnvironmentAssets(logger = createConsoleLogger()): void {
  const exportDir = mkdtempSync(join(tmpdir(), "ade-sandbox-environment-"));

  try {
    execFileSync(
      dockerCommand,
      [
        "buildx",
        "build",
        "--platform",
        readSandboxBuildPlatform(),
        "--build-arg",
        `SANDBOX_PYTHON_VERSION=${readPinnedPythonVersion()}`,
        "--file",
        dockerfilePath,
        "--target",
        "sandbox-environment-artifact",
        "--output",
        `type=local,dest=${exportDir}`,
        rootDir,
      ],
      {
        cwd: rootDir,
        stdio: "inherit",
      },
    );

    mkdirSync(dirname(sandboxEnvironmentOutputArchive), {
      recursive: true,
    });
    cpSync(
      join(exportDir, "sandbox-environment.tar.gz"),
      sandboxEnvironmentOutputArchive,
    );
    logger.info("Built sandbox environment archive");
  } finally {
    rmSync(exportDir, {
      force: true,
      recursive: true,
    });
  }
}

export {
  buildSandboxEnvironmentAssets,
  readPinnedPythonVersion,
  readSandboxBuildPlatform,
};

if (
  process.argv[1] &&
  pathToFileURL(process.argv[1]).href === import.meta.url
) {
  void runMain(() => {
    buildSandboxEnvironmentAssets();
  });
}
