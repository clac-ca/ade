import process from "node:process";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import {
  createAzuriteBlobConnectionString,
  createDevPorts,
  createDevProjectName,
  createLocalSqlConnectionString,
  createLocalSqlPassword,
  ensureDocker,
  openBrowser,
  parseArgs,
  registerShutdown,
  runCommand,
  spawnCommand,
  waitForDockerServiceHealth,
  waitForReady,
} from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const databaseName = "ade";

function signalChild(child, signal) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  try {
    if (child.adeDetached && process.platform !== "win32") {
      process.kill(-child.pid, signal);
      return;
    }

    child.kill(signal);
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes("kill ESRCH")) {
      throw error;
    }
  }
}

async function terminateChildren(children) {
  for (const child of children) {
    signalChild(child, "SIGINT");
  }

  await delay(1_000);

  for (const child of children) {
    signalChild(child, "SIGTERM");
  }

  await delay(250);

  for (const child of children) {
    signalChild(child, "SIGKILL");
  }
}

async function main() {
  const { port, noOpen } = parseArgs(process.argv.slice(2), {
    defaultPort: 8000,
    allowNoOpen: true,
  });

  const projectName = createDevProjectName(rootDir);
  const ports = createDevPorts(port);
  const sqlPassword = createLocalSqlPassword(projectName);
  const composeEnv = {
    ADE_AZURITE_BLOB_PORT: String(ports.azuriteBlob),
    ADE_AZURITE_QUEUE_PORT: String(ports.azuriteQueue),
    ADE_AZURITE_TABLE_PORT: String(ports.azuriteTable),
    ADE_SQL_PORT: String(ports.sql),
    ADE_SQL_SA_PASSWORD: sqlPassword,
  };
  const apiEnv = {
    AZURE_SQL_CONNECTIONSTRING: createLocalSqlConnectionString({
      database: databaseName,
      password: sqlPassword,
      port: ports.sql,
    }),
    AZURE_STORAGEBLOB_CONNECTIONSTRING:
      createAzuriteBlobConnectionString(ports),
    HOST: "127.0.0.1",
    PORT: String(ports.api),
  };
  const detached = process.platform !== "win32";
  const children = [];
  const appUrl = `http://localhost:${port}`;
  let shuttingDown = false;

  const shutdown = registerShutdown(async () => {
    shuttingDown = true;
    await terminateChildren(children);
    await runCommand(
      dockerCommand,
      ["compose", "-p", projectName, "down", "-v", "--remove-orphans"],
      {
        cwd: rootDir,
        env: composeEnv,
      },
    ).catch(() => undefined);
  });

  try {
    await ensureDocker(dockerCommand);
    await runCommand(
      dockerCommand,
      ["compose", "-p", projectName, "down", "-v", "--remove-orphans"],
      {
        cwd: rootDir,
        env: composeEnv,
      },
    ).catch(() => undefined);
    await runCommand(
      dockerCommand,
      ["compose", "-p", projectName, "up", "-d"],
      {
        cwd: rootDir,
        env: composeEnv,
      },
    );
    await waitForDockerServiceHealth(
      dockerCommand,
      projectName,
      ["azurite", "sqlserver"],
      {
        cwd: rootDir,
        env: composeEnv,
      },
    );
    await runCommand(pnpmCommand, ["--filter", "@ade/api", "migrate"], {
      cwd: rootDir,
      env: apiEnv,
    });

    const api = spawnCommand(pnpmCommand, ["--filter", "@ade/api", "dev"], {
      detached,
      env: apiEnv,
    });
    const web = spawnCommand(pnpmCommand, ["--filter", "@ade/web", "dev"], {
      detached,
      env: {
        ADE_API_ORIGIN: `http://127.0.0.1:${ports.api}`,
        ADE_WEB_PORT: String(port),
      },
    });

    children.push(api, web);

    for (const child of children) {
      child.on("exit", (code, signal) => {
        if (shuttingDown) {
          return;
        }

        if (
          signal === "SIGINT" ||
          signal === "SIGTERM" ||
          signal === "SIGKILL"
        ) {
          return;
        }

        console.error(
          `Launcher child exited with code ${code ?? "unknown"}${signal ? ` and signal ${signal}` : ""}.`,
        );
        void shutdown(code ?? 1);
      });
    }

    await waitForReady([`http://127.0.0.1:${ports.api}/api/readyz`], {
      isAlive: () =>
        children.every(
          (child) => child.exitCode === null && child.signalCode === null,
        ),
    });

    await waitForReady([`${appUrl}/`, `${appUrl}/api/readyz`], {
      isAlive: () =>
        children.every(
          (child) => child.exitCode === null && child.signalCode === null,
        ),
    });
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    await shutdown(1);
    process.exit(1);
  }

  if (!noOpen) {
    openBrowser(appUrl);
  }

  console.log(`ADE dev is running at ${appUrl} (project ${projectName})`);
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
