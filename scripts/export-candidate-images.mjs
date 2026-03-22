import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";
import { runCommand, writeJsonFile } from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const artifactDir = join(rootDir, "candidate-build-artifact");
const imagesTarPath = join(artifactDir, "candidate-images.tar");
const buildInfoPath = join(
  rootDir,
  "apps",
  "api",
  ".package",
  "dist",
  "build-info.json",
);

async function main() {
  if (!existsSync(buildInfoPath)) {
    throw new Error(
      "Missing build metadata at apps/api/.package/dist/build-info.json. Run `pnpm build` first.",
    );
  }

  rmSync(artifactDir, {
    force: true,
    recursive: true,
  });
  mkdirSync(artifactDir, {
    recursive: true,
  });

  await runCommand(
    dockerCommand,
    ["image", "inspect", "ade-web:local", "ade-api:local"],
    {
      cwd: rootDir,
      stdio: "ignore",
    },
  );
  await runCommand(
    dockerCommand,
    ["save", "--output", imagesTarPath, "ade-web:local", "ade-api:local"],
    {
      cwd: rootDir,
    },
  );

  copyFileSync(buildInfoPath, join(artifactDir, "build-info.json"));
  writeJsonFile(join(artifactDir, "candidate-build.json"), {
    artifactCreatedAt: new Date().toISOString(),
    buildInfoPath: "build-info.json",
    images: ["ade-web:local", "ade-api:local"],
    imagesTarPath: "candidate-images.tar",
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
