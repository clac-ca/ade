import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import type { Logger } from "./runtime";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const configFixtureRoot = join(repoRoot, ".package/configs");
const configFixtureStamp = join(configFixtureRoot, ".stamp");
const sampleScopes = [
  ["workspace-a", "config-v1"],
  ["workspace-b", "config-v2"],
] as const;

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

function ensureWheel(packageDir: string): string {
  const wheelPrefix = basename(packageDir).replace(/-/g, "_") + "-";
  if (wheelNeedsBuild(packageDir, wheelPrefix)) {
    execFileSync(
      pnpmCommand,
      ["exec", "uv", "build", "--directory", packageDir],
      {
        cwd: repoRoot,
        stdio: "inherit",
      },
    );
  }

  return newestWheel(join(packageDir, "dist"), wheelPrefix);
}

function needsLocalConfigMountBuild(packageDir: string): boolean {
  if (!existsSync(configFixtureStamp)) {
    return true;
  }

  return newestMtime(packageDir) > statSync(configFixtureStamp).mtimeMs;
}

function stageLocalConfigMounts(logger?: Logger): void {
  const packageDir = join(repoRoot, "packages/ade-config");
  const configWheelPath = ensureWheel(packageDir);

  if (!needsLocalConfigMountBuild(packageDir)) {
    logger?.info("Local config mount fixtures are current");
    return;
  }

  rmSync(configFixtureRoot, {
    force: true,
    recursive: true,
  });

  for (const [workspaceId, configVersionId] of sampleScopes) {
    const scopeDirectory = join(
      configFixtureRoot,
      workspaceId,
      configVersionId,
    );
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
  logger?.info("Staged local config mount fixtures");
}

export { stageLocalConfigMounts };
