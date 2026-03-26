import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { downLocalDependencies } from "./local-deps";
import { runMain } from "./lib/runtime";
import { runCommand } from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));

async function tryRun(command: string, args: readonly string[]): Promise<void> {
  try {
    await runCommand(command, args, {
      cwd: rootDir,
    });
  } catch {
    return;
  }
}

function readAdeContainers(): string[] {
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

async function main(): Promise<void> {
  await tryRun(pnpmCommand, ["-r", "--if-present", "run", "clean"]);
  rmSync(
    fileURLToPath(new URL("../packages/ade-engine/dist", import.meta.url)),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(
    fileURLToPath(new URL("../packages/ade-config/dist", import.meta.url)),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(
    fileURLToPath(new URL("../packages/ade-engine/.venv", import.meta.url)),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(
    fileURLToPath(new URL("../packages/ade-config/.venv", import.meta.url)),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(
    fileURLToPath(new URL("../packages/ade-engine/uv.lock", import.meta.url)),
    {
      force: true,
    },
  );
  rmSync(
    fileURLToPath(new URL("../packages/ade-config/uv.lock", import.meta.url)),
    {
      force: true,
    },
  );
  rmSync(
    fileURLToPath(
      new URL(
        "../packages/ade-engine/src/ade_engine/__pycache__",
        import.meta.url,
      ),
    ),
    {
      force: true,
      recursive: true,
    },
  );
  rmSync(
    fileURLToPath(
      new URL(
        "../packages/ade-config/src/ade_config/__pycache__",
        import.meta.url,
      ),
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
    await tryRun(dockerCommand, ["container", "rm", "--force", containerName]);
  }

  await downLocalDependencies({
    stdio: "ignore",
  }).catch(() => undefined);
  await tryRun(dockerCommand, ["image", "rm", "--force", "ade:local"]);
}

void runMain(async () => {
  await main();
});
