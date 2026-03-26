import process from "node:process";
import { parseStartArgs } from "./lib/args";
import { openBrowser } from "./lib/browser";
import { loadOptionalEnvFile } from "./lib/env-files";
import { createConsoleLogger, formatError, readOptionalTrimmedString, runMain } from "./lib/runtime";
import { createContainerRunArgs } from "./lib/start";
import {
  ensureDocker,
  registerShutdown,
  runCommand,
  runCommandCapture,
  spawnCommand,
  waitForReady,
} from "./lib/shell";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

async function removeContainer(name: string): Promise<void> {
  try {
    await runCommand(dockerCommand, ["container", "rm", "--force", name], {
      stdio: "ignore",
    });
  } catch {
    return;
  }
}

async function isContainerRunning(name: string): Promise<boolean> {
  try {
    const { stdout } = await runCommandCapture(dockerCommand, [
      "container",
      "inspect",
      "--format",
      "{{.State.Running}}",
      name,
    ]);

    return stdout.trim() === "true";
  } catch {
    return false;
  }
}

async function main(logger = createConsoleLogger()): Promise<void> {
  loadOptionalEnvFile();

  const sqlConnectionString = readOptionalTrimmedString(
    process.env,
    sqlConnectionStringName,
  );

  if (!sqlConnectionString) {
    throw new Error(
      `Missing required environment variable: ${sqlConnectionStringName}`,
    );
  }

  const { image, noOpen, port } = parseStartArgs(process.argv.slice(2));
  const containerName = `ade-local-${String(port)}`;
  const appUrl = `http://127.0.0.1:${String(port)}`;

  await ensureDocker(dockerCommand, "`pnpm start`");

  await runCommand(dockerCommand, ["image", "inspect", image], {
    stdio: "ignore",
  }).catch(() => {
    throw new Error(
      image === "ade:local"
        ? "Run `pnpm build` first."
        : `The configured image is not available locally: ${image}. Run \`docker pull ${image}\` or choose a local image.`,
    );
  });

  await removeContainer(containerName);

  const container = spawnCommand(
    dockerCommand,
    createContainerRunArgs({
      containerName,
      hostPort: port,
      image,
    }),
    {
      env: {
        [sqlConnectionStringName]: sqlConnectionString,
      },
    },
  );
  let shuttingDown = false;

  const shutdown = registerShutdown(async () => {
    shuttingDown = true;
    await removeContainer(containerName);
  });

  container.on("exit", (code, signal) => {
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

  try {
    await waitForReady([`${appUrl}/`, `${appUrl}/api/readyz`], {
      isAlive: () =>
        container.exitCode === null &&
        container.signalCode === null &&
        !shuttingDown,
      timeoutMs: 60_000,
    });
  } catch (error) {
    logger.error(formatError(error));

    if (await isContainerRunning(containerName)) {
      try {
        const { stdout } = await runCommandCapture(dockerCommand, [
          "logs",
          containerName,
        ]);

        if (stdout.trim() !== "") {
          logger.error(stdout.trim());
        }
      } catch {
        return;
      }
    }

    await shutdown(1);
    process.exit(1);
  }

  if (!noOpen) {
    openBrowser(appUrl);
  }

  logger.info(`ADE is running at ${appUrl}`);
  await new Promise(() => undefined);
}

void runMain(async () => {
  await main();
});
