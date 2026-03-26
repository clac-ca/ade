import process from "node:process";
import { fileURLToPath } from "node:url";
import { buildArtifacts } from "./build-artifacts";
import { runMain } from "./lib/runtime";
import { ensureDocker, runCommand } from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function buildImage(
  tag: string,
  contextPath: string,
  cacheKey: string,
): Promise<void> {
  const fs = await import("node:fs");
  const path = await import("node:path");
  const cacheRoot = path.join(rootDir, ".buildx-cache");
  const currentCache = path.join(cacheRoot, cacheKey);
  const nextCache = path.join(cacheRoot, `${cacheKey}-next`);
  const args = ["buildx", "build", "--load"];

  fs.mkdirSync(cacheRoot, {
    recursive: true,
  });
  fs.rmSync(nextCache, {
    force: true,
    recursive: true,
  });

  if (fs.existsSync(currentCache)) {
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

  fs.rmSync(currentCache, {
    force: true,
    recursive: true,
  });

  if (fs.existsSync(nextCache)) {
    fs.renameSync(nextCache, currentCache);
  }
}

async function main(): Promise<void> {
  await ensureDocker(dockerCommand, "`pnpm build`");
  await buildArtifacts();
  await buildImage("ade:local", ".", "app");
}

void runMain(async () => {
  await main();
});
