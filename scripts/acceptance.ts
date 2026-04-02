import process from "node:process";
import { parseAcceptanceArgs } from "./lib/args";
import { runAcceptanceChecks } from "./lib/acceptance-checks";
import { loadOptionalEnvFile } from "./lib/env-files";
import { writeGitHubOutput } from "./lib/github";
import { startLocalRuntime } from "./lib/local-runtime";
import {
  createConsoleLogger,
  formatError,
  readOptionalTrimmedString,
  runMain,
} from "./lib/runtime";
import { registerShutdown, waitForReady } from "./lib/shell";

const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

async function main(logger = createConsoleLogger()): Promise<void> {
  const startedAt = Date.now();
  loadOptionalEnvFile();
  const config = parseAcceptanceArgs(process.argv.slice(2));

  if (config.mode === "attach") {
    await runAcceptanceChecks(config.url.toString());
  } else {
    const runtime = await startLocalRuntime({
      containerName: `ade-acceptance-${String(config.port)}`,
      hostPort: config.port,
      image: config.image,
      logger,
      sqlConnectionString: readOptionalTrimmedString(
        process.env,
        sqlConnectionStringName,
      ),
      usage: "`pnpm test:acceptance`",
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
      await waitForReady(
        [`${runtime.appUrl}/`, `${runtime.appUrl}/api/readyz`],
        {
          isAlive: () => runtime.isAlive() && !shuttingDown,
          timeoutMs: 60_000,
        },
      );
      await runAcceptanceChecks(runtime.appUrl);
    } catch (error) {
      logger.error(formatError(error));
      const logs = await runtime.dumpLogs();

      if (logs !== "") {
        logger.error(logs);
      }

      await shutdown(1);
      process.exit(1);
    }

    shuttingDown = true;
    await runtime.stop();
  }

  writeGitHubOutput(process.env, {
    duration_seconds: Math.round((Date.now() - startedAt) / 1000),
  });
}

void runMain(async () => {
  await main();
});
