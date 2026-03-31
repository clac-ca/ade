import process from "node:process";
import { parseStartArgs } from "./lib/args";
import { openBrowser } from "./lib/browser";
import { loadOptionalEnvFile } from "./lib/env-files";
import {
  createConsoleLogger,
  formatError,
  readOptionalTrimmedString,
  runMain,
} from "./lib/runtime";
import { startLocalRuntime } from "./lib/local-runtime";
import { registerShutdown, waitForReady } from "./lib/shell";

const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

async function main(logger = createConsoleLogger()): Promise<void> {
  loadOptionalEnvFile();
  const { image, noOpen, port } = parseStartArgs(process.argv.slice(2));
  const sqlConnectionString = readOptionalTrimmedString(
    process.env,
    sqlConnectionStringName,
  );
  const runtime = await startLocalRuntime({
    containerName: `ade-local-${String(port)}`,
    hostPort: port,
    image,
    logger,
    sqlConnectionString,
    usage: "`pnpm start`",
  });
  let shuttingDown = false;

  const shutdown = registerShutdown(async () => {
    shuttingDown = true;
    await runtime.stop();
  });

  runtime.container.on("exit", (code, signal) => {
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
    await waitForReady([`${runtime.appUrl}/`, `${runtime.appUrl}/api/readyz`], {
      isAlive: () => runtime.isAlive() && !shuttingDown,
      timeoutMs: 60_000,
    });
  } catch (error) {
    logger.error(formatError(error));
    const logs = await runtime.dumpLogs();

    if (logs !== "") {
      logger.error(logs);
    }

    await shutdown(1);
    process.exit(1);
  }

  if (!noOpen) {
    openBrowser(runtime.appUrl);
  }

  logger.info(
    sqlConnectionString
      ? `ADE is running at ${runtime.appUrl}`
      : `ADE is running at ${runtime.appUrl} using managed local SQL`,
  );
  await new Promise(() => undefined);
}

void runMain(async () => {
  await main();
});
