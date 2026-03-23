import { execFileSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import process from "node:process";
import { runCommand } from "./shared.mjs";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const apiPackage = JSON.parse(
  readFileSync(new URL("../apps/api/package.json", import.meta.url), "utf8"),
);

function readGitMetadata() {
  const gitSha =
    process.env.GITHUB_SHA ?? readGitValue(["rev-parse", "HEAD"]) ?? "local";
  const builtAt =
    readGitValue(["show", "--no-patch", "--format=%cI", gitSha]) ??
    new Date().toISOString();

  return {
    builtAt,
    gitSha,
  };
}

function readGitValue(args) {
  try {
    return execFileSync("git", args, {
      cwd: rootDir,
      encoding: "utf8",
    }).trim();
  } catch {
    return null;
  }
}

async function buildArtifacts() {
  const { builtAt, gitSha } = readGitMetadata();
  const packageRoot = join(rootDir, "apps", "api", ".package");
  const buildInfoPath = join(packageRoot, "dist", "build-info.json");
  const publicPath = join(packageRoot, "public");
  const webDistPath = join(rootDir, "apps", "web", "dist");

  await runCommand(pnpmCommand, ["--filter", "@ade/web", "build"], {
    cwd: rootDir,
  });
  await runCommand(pnpmCommand, ["--filter", "@ade/api", "build"], {
    cwd: rootDir,
  });
  rmSync(packageRoot, {
    force: true,
    recursive: true,
  });
  await runCommand(
    pnpmCommand,
    ["--filter", "@ade/api", "deploy", "--prod", "apps/api/.package"],
    {
      cwd: rootDir,
    },
  );

  mkdirSync(dirname(buildInfoPath), {
    recursive: true,
  });
  rmSync(publicPath, {
    force: true,
    recursive: true,
  });
  cpSync(webDistPath, publicPath, {
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
  void buildArtifacts().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
