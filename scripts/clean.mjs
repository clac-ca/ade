import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import process from "node:process";
import { runCommand } from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function tryRun(command, args, options = {}) {
  try {
    await runCommand(command, args, options);
  } catch {
    return undefined;
  }
}

function readAdeContainers() {
  try {
    const output = execFileSync(
      dockerCommand,
      ["ps", "-a", "--filter", "name=^ade-local-", "--format", "{{.Names}}"],
      {
        cwd: rootDir,
        encoding: "utf8",
      },
    );

    return output
      .split("\n")
      .map((value) => value.trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

async function main() {
  await tryRun(pnpmCommand, ["-r", "--if-present", "run", "clean"], {
    cwd: rootDir,
  });
  rmSync(fileURLToPath(new URL("../python/ade-engine/dist", import.meta.url)), {
    force: true,
    recursive: true,
  });
  rmSync(
    fileURLToPath(
      new URL("../python/ade-config-template/dist", import.meta.url),
    ),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(fileURLToPath(new URL("../.buildx-cache", import.meta.url)), {
    force: true,
    recursive: true,
  });

  for (const containerName of readAdeContainers()) {
    await tryRun(dockerCommand, ["container", "rm", "--force", containerName], {
      cwd: rootDir,
      stdio: "ignore",
    });
  }

  await tryRun(dockerCommand, ["image", "rm", "--force", "ade:local"], {
    cwd: rootDir,
    stdio: "ignore",
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
