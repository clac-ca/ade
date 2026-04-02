import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { localComposeProjectName } from "./lib/dev-config";
import {
  createConsoleLogger,
  readOptionalTrimmedString,
  runMain,
} from "./lib/runtime";
import { ensureDocker, runCommand, runCommandCapture } from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const composeFile = fileURLToPath(
  new URL("../infra/local/compose.yaml", import.meta.url),
);
const sessionPoolBuildComposeFile = fileURLToPath(
  new URL("../infra/local/compose.sessionpool.build.yaml", import.meta.url),
);
const sessionPoolImageComposeFile = fileURLToPath(
  new URL("../infra/local/compose.sessionpool.image.yaml", import.meta.url),
);

function readComposeFiles(
  env: Record<string, string | undefined>,
): readonly string[] {
  const sessionPoolImage = readOptionalTrimmedString(
    env,
    "ADE_SESSIONPOOL_IMAGE",
  );

  return [
    composeFile,
    sessionPoolImage
      ? sessionPoolImageComposeFile
      : sessionPoolBuildComposeFile,
  ];
}

async function runCompose(
  args: readonly string[],
  options: {
    env?: Record<string, string | undefined>;
    stdio?: "ignore" | "inherit";
  } = {},
): Promise<void> {
  const composeFiles = readComposeFiles({
    ...process.env,
    ...(options.env ?? {}),
  });

  await runCommand(
    dockerCommand,
    [
      "compose",
      ...composeFiles.flatMap((file) => ["-f", file]),
      "-p",
      localComposeProjectName,
      ...args,
    ],
    {
      cwd: rootDir,
      ...(options.env
        ? {
            env: options.env,
          }
        : {}),
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
  await runCompose([
    "up",
    "-d",
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
  const composeFiles = readComposeFiles(process.env);
  const { stdout } = await runCommandCapture(
    dockerCommand,
    [
      "compose",
      ...composeFiles.flatMap((file) => ["-f", file]),
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
