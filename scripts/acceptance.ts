import process from "node:process";
import { runCommand } from "./lib/shell";
import { parseAcceptanceArgs } from "./lib/args";
import { loadOptionalEnvFile } from "./lib/env-files";
import { writeGitHubOutput } from "./lib/github";
import { startLocalRuntime } from "./lib/local-runtime";
import {
  createConsoleLogger,
  formatError,
  type Logger,
  readOptionalTrimmedString,
  runMain,
} from "./lib/runtime";
import { registerShutdown, waitForReady } from "./lib/shell";

const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

type AcceptanceTarget = {
  baseUrl: string;
  dumpLogs: () => Promise<string>;
  onUnexpectedExit: (
    onUnexpectedExit: (
      code: number | null,
      signal: NodeJS.Signals | null,
    ) => Promise<void>,
  ) => void;
  stop: () => Promise<void>;
  waitUntilReady: () => Promise<void>;
};

function normalizeBaseUrl(value: string): string {
  const trimmed = value.trim();

  if (trimmed === "") {
    throw new Error("--url must not be empty.");
  }

  return trimmed.replace(/\/+$/, "");
}

async function runPlaywrightAcceptance(baseUrl: string): Promise<void> {
  await runCommand(pnpmCommand, ["exec", "playwright", "test"], {
    env: {
      PLAYWRIGHT_BASE_URL: normalizeBaseUrl(baseUrl),
    },
  });
}

function createAttachedAcceptanceTarget(url: URL): AcceptanceTarget {
  return {
    baseUrl: normalizeBaseUrl(url.toString()),
    dumpLogs: () => Promise.resolve(""),
    onUnexpectedExit: () => undefined,
    stop: () => Promise.resolve(),
    waitUntilReady: () => Promise.resolve(),
  };
}

async function createManagedAcceptanceTarget(
  config: Extract<ReturnType<typeof parseAcceptanceArgs>, { mode: "managed" }>,
  logger: Logger,
): Promise<AcceptanceTarget> {
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

  return {
    baseUrl: runtime.appUrl,
    dumpLogs: runtime.dumpLogs,
    onUnexpectedExit: (onUnexpectedExit) => {
      runtime.container.on("exit", (code, signal) => {
        if (
          signal === "SIGINT" ||
          signal === "SIGTERM" ||
          signal === "SIGKILL"
        ) {
          return;
        }

        void onUnexpectedExit(code, signal);
      });
    },
    stop: runtime.stop,
    waitUntilReady: async () => {
      await waitForReady([`${runtime.appUrl}/`, `${runtime.appUrl}/api/readyz`], {
        isAlive: runtime.isAlive,
        timeoutMs: 60_000,
      });
    },
  };
}

async function createAcceptanceTarget(
  config: ReturnType<typeof parseAcceptanceArgs>,
  logger: Logger,
): Promise<AcceptanceTarget> {
  if (config.mode === "attach") {
    return createAttachedAcceptanceTarget(config.url);
  }

  return createManagedAcceptanceTarget(config, logger);
}

async function main(logger = createConsoleLogger()): Promise<void> {
  const startedAt = Date.now();
  loadOptionalEnvFile();
  const config = parseAcceptanceArgs(process.argv.slice(2));
  const target = await createAcceptanceTarget(config, logger);
  let stopping = false;

  const stopTarget = async (): Promise<void> => {
    if (stopping) {
      return;
    }

    stopping = true;
    await target.stop();
  };

  const shutdown = registerShutdown(async () => {
    await stopTarget();
  });

  target.onUnexpectedExit(async (code, signal) => {
    if (stopping) {
      return;
    }

    logger.error(
      `Acceptance target exited with code ${String(code ?? "unknown")}${signal ? ` and signal ${signal}` : ""}.`,
    );
    await shutdown(code ?? 1);
  });

  try {
    logger.info(`Running acceptance against ${target.baseUrl}.`);
    await target.waitUntilReady();
    await runPlaywrightAcceptance(target.baseUrl);
  } catch (error) {
    logger.error(formatError(error));
    const logs = await target.dumpLogs();

    if (logs !== "") {
      logger.error(logs);
    }

    await shutdown(1);
    process.exit(1);
  }

  await stopTarget();

  writeGitHubOutput(process.env, {
    duration_seconds: Math.round((Date.now() - startedAt) / 1000),
  });
}

void runMain(async () => {
  await main();
});
