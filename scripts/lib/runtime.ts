import process from "node:process";

type Logger = {
  error(message: string): void;
  info(message: string): void;
};

type ProcessHandle = Pick<typeof process, "exit">;

function createConsoleLogger(
  consoleLike: Pick<typeof console, "error" | "log"> = console,
): Logger {
  return {
    error(message: string) {
      consoleLike.error(message);
    },
    info(message: string) {
      consoleLike.log(message);
    },
  };
}

function formatError(error: unknown): string {
  if (error instanceof Error) {
    return error.stack ?? error.message;
  }

  return typeof error === "string" ? error : JSON.stringify(error);
}

function readOptionalTrimmedString(
  env: Record<string, string | undefined>,
  name: string,
): string | undefined {
  const value = env[name];

  if (value === undefined) {
    return undefined;
  }

  const trimmed = value.trim();
  return trimmed === "" ? undefined : trimmed;
}

async function runMain(
  main: () => Promise<void>,
  options: {
    logger?: Logger;
    processHandle?: ProcessHandle;
  } = {},
): Promise<void> {
  const logger = options.logger ?? createConsoleLogger();
  const processHandle = options.processHandle ?? process;

  try {
    await main();
  } catch (error) {
    logger.error(formatError(error));
    processHandle.exit(1);
  }
}

export { createConsoleLogger, formatError, readOptionalTrimmedString, runMain };

export type { Logger };
