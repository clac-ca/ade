import process from "node:process";

export type Logger = {
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

export { createConsoleLogger, formatError, runMain };
