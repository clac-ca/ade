import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";
import { runCommand } from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

function readArtifactDir() {
  const flagIndex = process.argv.indexOf("--artifact-dir");

  if (flagIndex === -1) {
    return join(rootDir, "candidate-build-artifact");
  }

  const value = process.argv[flagIndex + 1];

  if (!value) {
    throw new Error("Missing value for --artifact-dir");
  }

  return value;
}

async function main() {
  const artifactDir = readArtifactDir();
  const imagesTarPath = join(artifactDir, "candidate-images.tar");

  if (!existsSync(imagesTarPath)) {
    throw new Error(`Missing candidate image archive: ${imagesTarPath}`);
  }

  await runCommand(dockerCommand, ["load", "--input", imagesTarPath], {
    cwd: rootDir,
  });
  await runCommand(
    dockerCommand,
    ["image", "inspect", "ade-web:local", "ade-api:local"],
    {
      cwd: rootDir,
      stdio: "ignore",
    },
  );
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
