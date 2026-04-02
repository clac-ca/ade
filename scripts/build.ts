import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { buildSandboxEnvironmentAssets } from "../apps/ade-api/sandbox-environment/build";
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
    readOptionalTrimmedString(env, "ADE_GIT_SHA") ??
    readOptionalTrimmedString(env, "GITHUB_SHA") ??
    readGitValue(["rev-parse", "HEAD"]) ??
    "local";
  const builtAt =
    readOptionalTrimmedString(env, "ADE_BUILT_AT") ??
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
    version:
      readOptionalTrimmedString(env, "ADE_SERVICE_VERSION") ??
      apiPackage.version,
  };
}

function readBuildCacheSettings(
  env: Record<string, string | undefined>,
  name: string,
): string[] {
  const value = readOptionalTrimmedString(env, name);
  if (value === undefined) {
    return [];
  }

  return value
    .split("\n")
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
}

function readBuildImage(env: Record<string, string | undefined>): string {
  return (
    readOptionalTrimmedString(env, "ADE_BUILD_IMAGE") ?? "ade-platform:local"
  );
}

function shouldPushBuild(env: Record<string, string | undefined>): boolean {
  const value = readOptionalTrimmedString(env, "ADE_BUILD_PUSH");
  if (value === undefined) {
    return false;
  }

  return value.toLowerCase() === "true";
}

async function buildImage(
  image: string,
  contextPath: string,
  buildArgs: Record<string, string>,
  options: {
    cacheFrom?: readonly string[];
    cacheTo?: readonly string[];
    push?: boolean;
  } = {},
): Promise<void> {
  const args = ["buildx", "build"];

  if (options.push) {
    args.push("--push");
  } else {
    args.push("--load");
  }

  for (const [name, value] of Object.entries(buildArgs)) {
    args.push("--build-arg", `${name}=${value}`);
  }

  for (const cacheFrom of options.cacheFrom ?? []) {
    args.push("--cache-from", cacheFrom);
  }

  for (const cacheTo of options.cacheTo ?? []) {
    args.push("--cache-to", cacheTo);
  }

  args.push("-t", image, contextPath);

  await runCommand(dockerCommand, args, {
    cwd: rootDir,
  });
}

async function buildInfrastructureArtifacts(): Promise<void> {
  await runCommand(
    "az",
    ["bicep", "build", "--file", "infra/main.bicep", "--stdout"],
    {
      cwd: rootDir,
      stdio: "ignore",
    },
  );
  await runCommand(
    "az",
    [
      "bicep",
      "build-params",
      "--file",
      "infra/environments/main.prod.bicepparam",
      "--stdout",
    ],
    {
      cwd: rootDir,
      stdio: "ignore",
    },
  );
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
  const image = readBuildImage(process.env);
  const push = shouldPushBuild(process.env);
  const cacheFrom = readBuildCacheSettings(process.env, "ADE_BUILD_CACHE_FROM");
  const cacheTo = readBuildCacheSettings(process.env, "ADE_BUILD_CACHE_TO");

  await ensureDocker(dockerCommand, "`pnpm build`");
  buildSandboxEnvironmentAssets();
  await buildInfrastructureArtifacts();
  await buildImage(
    image,
    ".",
    {
      BUILT_AT: metadata.builtAt,
      GIT_SHA: metadata.gitSha,
      SERVICE_VERSION: metadata.version,
    },
    {
      cacheFrom,
      cacheTo,
      push,
    },
  );
}

void runMain(async () => {
  await main();
});
