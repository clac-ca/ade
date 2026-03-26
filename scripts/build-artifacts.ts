import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { runCommand } from "./lib/shell";
import { readOptionalTrimmedString, runMain } from "./lib/runtime";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

function readGitValue(args: readonly string[]): string | null {
  try {
    return execFileSync("git", args, {
      cwd: rootDir,
      encoding: "utf8",
    }).trim();
  } catch {
    return null;
  }
}

function readGitMetadata(env: Record<string, string | undefined>) {
  const gitSha =
    readOptionalTrimmedString(env, "GITHUB_SHA") ??
    readGitValue(["rev-parse", "HEAD"]) ??
    "local";
  const builtAt =
    readGitValue(["show", "--no-patch", "--format=%cI", gitSha]) ??
    new Date().toISOString();

  return {
    builtAt,
    gitSha,
  };
}

async function buildArtifacts(
  env: Record<string, string | undefined> = process.env,
): Promise<void> {
  const { builtAt, gitSha } = readGitMetadata(env);
  const apiPackage = JSON.parse(
    readFileSync(
      new URL("../apps/ade-api/package.json", import.meta.url),
      "utf8",
    ),
  ) as {
    version?: unknown;
  };

  if (
    typeof apiPackage.version !== "string" ||
    apiPackage.version.trim() === ""
  ) {
    throw new Error(
      "apps/ade-api/package.json must contain a non-empty version string.",
    );
  }

  const apiDistPath = join(rootDir, "apps", "ade-api", "dist");
  const buildInfoPath = join(apiDistPath, "build-info.json");

  await runCommand(pnpmCommand, ["--filter", "@ade/web", "build"], {
    cwd: rootDir,
  });
  rmSync(apiDistPath, {
    force: true,
    recursive: true,
  });
  mkdirSync(dirname(buildInfoPath), {
    recursive: true,
  });
  writeFileSync(
    buildInfoPath,
    JSON.stringify(
      {
        builtAt,
        gitSha,
        service: "ade",
        version: apiPackage.version,
      },
      null,
      2,
    ) + "\n",
  );
}

export { buildArtifacts };

if (
  process.argv[1] &&
  pathToFileURL(process.argv[1]).href === import.meta.url
) {
  void runMain(async () => {
    await buildArtifacts();
  });
}
