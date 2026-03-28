import process from "node:process";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { parseDevArgs } from "./lib/args";
import { openBrowser } from "./lib/browser";
import {
  createLocalSessionPoolManagementEndpoint,
  createLocalSessionPoolMcpEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localSessionPoolRuntimeSecret,
} from "./lib/dev-config";
import { createConsoleLogger, formatError, runMain } from "./lib/runtime";
import {
  registerShutdown,
  runCommand,
  spawnCommand,
  waitForReady,
  type ChildProcessWithAde,
} from "./lib/shell";
import { downLocalDependencies, upLocalDependencies } from "./local-deps";

const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

function signalChild(child: ChildProcessWithAde, signal: NodeJS.Signals) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  try {
    if (child.adeDetached && process.platform !== "win32" && child.pid) {
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

async function terminateChildren(
  children: readonly ChildProcessWithAde[],
): Promise<void> {
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

async function main(logger = createConsoleLogger()): Promise<void> {
  const { noOpen, port } = parseDevArgs(process.argv.slice(2));
  const apiEnv = {
    [sqlConnectionStringName]: createLocalSqlConnectionString(),
    ADE_RUNTIME_SESSION_SECRET: localSessionPoolRuntimeSecret,
    ADE_SESSION_POOL_MANAGEMENT_ENDPOINT:
      createLocalSessionPoolManagementEndpoint(),
    ADE_SESSION_POOL_MCP_ENDPOINT: createLocalSessionPoolMcpEndpoint(),
  };
  const detached = process.platform !== "win32";
  const children: ChildProcessWithAde[] = [];
  const appUrl = `http://127.0.0.1:${String(port)}`;
  let shuttingDown = false;

  const shutdown = registerShutdown(async () => {
    shuttingDown = true;
    await terminateChildren(children);
    await downLocalDependencies().catch(() => undefined);
  });

  try {
    await upLocalDependencies();
    await runCommand(pnpmCommand, ["--filter", "@ade/api", "migrate"], {
      cwd: rootDir,
      env: apiEnv,
    });

    const api = spawnCommand(
      pnpmCommand,
      [
        "--filter",
        "@ade/api",
        "dev",
        "--host",
        localApiHost,
        "--port",
        String(localApiPort),
      ],
      {
        detached,
        env: apiEnv,
      },
    );
    const web = spawnCommand(
      pnpmCommand,
      [
        "--filter",
        "@ade/web",
        "dev",
        "--host",
        localApiHost,
        "--port",
        String(port),
        "--strictPort",
      ],
      {
        detached,
      },
    );

    children.push(api, web);

    for (const child of children) {
      child.on("exit", (code, signal) => {
        if (
          shuttingDown ||
          signal === "SIGINT" ||
          signal === "SIGTERM" ||
          signal === "SIGKILL"
        ) {
          return;
        }

        logger.error(
          `Launcher child exited with code ${String(code ?? "unknown")}${signal ? ` and signal ${signal}` : ""}.`,
        );
        void shutdown(code ?? 1);
      });
    }

    await waitForReady([`http://${localApiHost}:${String(localApiPort)}/api/readyz`], {
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
    logger.error(formatError(error));
    await shutdown(1);
    process.exit(1);
  }

  if (!noOpen) {
    openBrowser(appUrl);
  }

  logger.info(`ADE dev is running at ${appUrl}`);
}

void runMain(async () => {
  await main();
});
