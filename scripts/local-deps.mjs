import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createDevPorts,
  createDevProjectName,
  createLocalSqlPassword,
  ensureDocker,
  parseArgs,
  runCommand,
  waitForDockerServiceHealth,
} from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const composeFile = fileURLToPath(
  new URL("../infra/local/compose.yaml", import.meta.url),
);

export function createLocalDependencyContext(basePort) {
  const projectName = createDevProjectName(rootDir);
  const ports = createDevPorts(basePort);
  const sqlPassword = createLocalSqlPassword(projectName);

  return {
    composeEnv: {
      ADE_AZURITE_BLOB_PORT: String(ports.azuriteBlob),
      ADE_AZURITE_QUEUE_PORT: String(ports.azuriteQueue),
      ADE_AZURITE_TABLE_PORT: String(ports.azuriteTable),
      ADE_SQL_PORT: String(ports.sql),
      ADE_SQL_SA_PASSWORD: sqlPassword,
    },
    ports,
    projectName,
    sqlPassword,
  };
}

async function runCompose(projectName, composeEnv, args, options = {}) {
  await runCommand(
    dockerCommand,
    ["compose", "-f", composeFile, "-p", projectName, ...args],
    {
      cwd: rootDir,
      env: composeEnv,
      stdio: options.stdio,
    },
  );
}

export async function upLocalDependencies(basePort) {
  const { composeEnv, projectName } = createLocalDependencyContext(basePort);

  await ensureDocker(dockerCommand);
  await runCompose(projectName, composeEnv, [
    "down",
    "-v",
    "--remove-orphans",
  ]).catch(() => undefined);
  await runCompose(projectName, composeEnv, ["up", "-d"]);
  await waitForDockerServiceHealth(
    dockerCommand,
    projectName,
    ["azurite", "sqlserver"],
    {
      composeArgs: ["-f", composeFile],
      cwd: rootDir,
      env: composeEnv,
    },
  );
}

export async function downLocalDependencies(basePort, options = {}) {
  const { composeEnv, projectName } = createLocalDependencyContext(basePort);

  await runCompose(
    projectName,
    composeEnv,
    ["down", "-v", "--remove-orphans"],
    options,
  );
}

async function main() {
  const [command, ...args] = process.argv.slice(2);

  if (command !== "up" && command !== "down") {
    throw new Error(
      "Usage: node scripts/local-deps.mjs <up|down> [--port <port>]",
    );
  }

  const { port } = parseArgs(args, {
    defaultPort: 8000,
  });

  if (command === "up") {
    await upLocalDependencies(port);
    console.log(`ADE local dependencies are running for port ${port}`);
    return;
  }

  await downLocalDependencies(port);
  console.log(`ADE local dependencies are stopped for port ${port}`);
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  void main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
