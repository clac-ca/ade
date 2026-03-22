import { existsSync, mkdirSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { buildArtifacts } from "./build-artifacts.mjs";
import { runCommand } from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function ensureDocker() {
  try {
    await runCommand(dockerCommand, ["info"], {
      stdio: "ignore",
    });
  } catch {
    throw new Error(
      "Docker is required for `pnpm build`, and the Docker daemon must be running.",
    );
  }
}

async function buildImage(tag, contextPath, cacheKey) {
  const cacheRoot = join(rootDir, ".buildx-cache");
  const currentCache = join(cacheRoot, cacheKey);
  const nextCache = join(cacheRoot, `${cacheKey}-next`);
  const args = ["buildx", "build", "--load"];

  mkdirSync(cacheRoot, {
    recursive: true,
  });
  rmSync(nextCache, {
    force: true,
    recursive: true,
  });

  if (existsSync(currentCache)) {
    args.push("--cache-from", `type=local,src=${currentCache}`);
  }

  args.push(
    "--cache-to",
    `type=local,dest=${nextCache},mode=max`,
    "-t",
    tag,
    contextPath,
  );

  await runCommand(dockerCommand, args, {
    cwd: rootDir,
  });

  rmSync(currentCache, {
    force: true,
    recursive: true,
  });

  if (existsSync(nextCache)) {
    renameSync(nextCache, currentCache);
  }
}

async function main() {
  await ensureDocker();
  await buildArtifacts();
  await buildImage("ade-web:local", "apps/web", "web");
  await buildImage("ade-api:local", "apps/api", "api");
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
