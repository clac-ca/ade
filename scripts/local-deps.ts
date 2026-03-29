import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { localComposeProjectName } from "./lib/dev-config";
import { createConsoleLogger, runMain } from "./lib/runtime";
import { ensureDocker, runCommand, runCommandCapture } from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const composeFile = fileURLToPath(
  new URL("../infra/local/compose.yaml", import.meta.url),
);

async function runCompose(
  args: readonly string[],
  options: {
    stdio?: "ignore" | "inherit";
  } = {},
): Promise<void> {
  await runCommand(
    dockerCommand,
    ["compose", "-f", composeFile, "-p", localComposeProjectName, ...args],
    {
      cwd: rootDir,
      ...(options.stdio
        ? {
            stdio: options.stdio,
          }
        : {}),
    },
  );
}

async function upLocalDependencies(): Promise<void> {
  await ensureDocker(dockerCommand, "`pnpm deps:up`");
  await runCompose(["down", "-v", "--remove-orphans"]).catch(() => undefined);
  await runCompose([
    "up",
    "-d",
    "--build",
    "--wait",
    "azurite",
    "sqlserver",
    "sessionpool",
  ]);
}

async function downLocalDependencies(
  options: {
    stdio?: "ignore" | "inherit";
  } = {},
): Promise<void> {
  await runCompose(["down", "-v", "--remove-orphans"], options);
}

async function readLocalDependencyLogs(
  services: readonly string[] = ["azurite", "sqlserver", "sessionpool"],
): Promise<string> {
  const { stdout } = await runCommandCapture(
    dockerCommand,
    [
      "compose",
      "-f",
      composeFile,
      "-p",
      localComposeProjectName,
      "logs",
      "--no-color",
      ...services,
    ],
    {
      cwd: rootDir,
    },
  );

  return stdout.trim();
}

async function main(logger = createConsoleLogger()): Promise<void> {
  const [command, ...args] = process.argv.slice(2);

  if (command !== "up" && command !== "down") {
    throw new Error("Usage: tsx scripts/local-deps.ts <up|down>");
  }

  if (args.length > 0) {
    throw new Error("scripts/local-deps.ts does not accept extra arguments.");
  }

  if (command === "up") {
    await upLocalDependencies();
    logger.info("ADE local Blob Storage, SQL, and session pool are running");
    return;
  }

  await downLocalDependencies();
  logger.info("ADE local Blob Storage, SQL, and session pool are stopped");
}

export { downLocalDependencies, readLocalDependencyLogs, upLocalDependencies };

if (
  process.argv[1] &&
  pathToFileURL(process.argv[1]).href === import.meta.url
) {
  void runMain(async () => {
    await main();
  });
}
