import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { readOptionalTrimmedString, runMain } from "./lib/runtime";
import { ensureDocker, runCommand } from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
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

function readBuildMetadata(env: Record<string, string | undefined>) {
  const gitSha =
    readOptionalTrimmedString(env, "GITHUB_SHA") ??
    readGitValue(["rev-parse", "HEAD"]) ??
    "local";
  const builtAt =
    readGitValue(["show", "--no-patch", "--format=%cI", gitSha]) ??
    new Date().toISOString();
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

  return {
    builtAt,
    gitSha,
    version: apiPackage.version,
  };
}

async function buildImage(
  tag: string,
  contextPath: string,
  buildArgs: Record<string, string>,
): Promise<void> {
  const args = ["buildx", "build", "--load"];

  for (const [name, value] of Object.entries(buildArgs)) {
    args.push("--build-arg", `${name}=${value}`);
  }

  args.push("-t", tag, contextPath);

  await runCommand(dockerCommand, args, {
    cwd: rootDir,
  });
}

async function main(): Promise<void> {
  const extraArgs = process.argv.slice(2).filter((arg) => arg !== "--");
  const firstExtraArg = extraArgs[0];

  if (firstExtraArg !== undefined) {
    throw new Error(
      `Unknown argument for \`pnpm build\`: ${firstExtraArg}. \`pnpm build\` does not accept extra arguments.`,
    );
  }

  const metadata = readBuildMetadata(process.env);

  await ensureDocker(dockerCommand, "`pnpm build`");
  await buildImage("ade-platform:local", ".", {
    BUILT_AT: metadata.builtAt,
    GIT_SHA: metadata.gitSha,
    SERVICE_VERSION: metadata.version,
  });
}

void runMain(async () => {
  await main();
});
