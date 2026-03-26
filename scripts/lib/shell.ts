import { spawn, type ChildProcess } from "node:child_process";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { createConsoleLogger, formatError } from "./runtime";

type ChildProcessWithAde = ChildProcess & {
  adeDetached?: boolean;
};

type CommandOptions = {
  cwd?: string;
  detached?: boolean;
  env?: Record<string, string | undefined>;
  stdio?: "ignore" | "inherit" | ["ignore", "pipe", "pipe"];
};

function spawnCommand(
  command: string,
  args: readonly string[],
  options: CommandOptions = {},
): ChildProcessWithAde {
  const child = spawn(command, args, {
    cwd: options.cwd,
    detached: options.detached ?? false,
    env: {
      ...process.env,
      ...options.env,
    },
    stdio: options.stdio ?? "inherit",
  }) as ChildProcessWithAde;

  child.adeDetached = options.detached ?? false;
  return child;
}

async function runCommand(
  command: string,
  args: readonly string[],
  options: CommandOptions = {},
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const child = spawnCommand(command, args, options);

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} exited with signal ${signal}`));
        return;
      }

      if (code !== 0) {
        reject(
          new Error(`${command} exited with code ${String(code ?? "unknown")}`),
        );
        return;
      }

      resolve();
    });
  });
}

async function runCommandCapture(
  command: string,
  args: readonly string[],
  options: Omit<CommandOptions, "stdio"> = {},
): Promise<{
  stderr: string;
  stdout: string;
}> {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const child = spawnCommand(command, args, {
      ...options,
      stdio: ["ignore", "pipe", "pipe"],
    });

    child.stdout?.setEncoding("utf8");
    child.stdout?.on("data", (chunk: string) => {
      stdout += chunk;
    });

    child.stderr?.setEncoding("utf8");
    child.stderr?.on("data", (chunk: string) => {
      stderr += chunk;
    });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} exited with signal ${signal}`));
        return;
      }

      if (code !== 0) {
        reject(
          new Error(
            stderr.trim() ||
              `${command} exited with code ${String(code ?? "unknown")}`,
          ),
        );
        return;
      }

      resolve({
        stderr,
        stdout,
      });
    });
  });
}

async function waitForReady(
  urls: readonly string[],
  options: {
    isAlive?: () => boolean;
    timeoutMs?: number;
  } = {},
): Promise<void> {
  const timeoutMs = options.timeoutMs ?? 30_000;
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (options.isAlive && !options.isAlive()) {
      throw new Error("A required process exited before ADE became ready.");
    }

    const results = await Promise.all(
      urls.map(async (url) => {
        try {
          const response = await fetch(url);
          return response.ok;
        } catch {
          return false;
        }
      }),
    );

    if (results.every(Boolean)) {
      return;
    }

    await delay(250);
  }

  throw new Error(`Timed out waiting for: ${urls.join(", ")}`);
}

async function ensureDocker(
  command: string,
  usage = "this command",
): Promise<void> {
  try {
    await runCommand(command, ["info"], {
      stdio: "ignore",
    });
  } catch {
    throw new Error(
      `Docker is required for ${usage}, and the Docker daemon must be running.`,
    );
  }
}

async function waitForDockerServiceHealth(
  dockerCommand: string,
  projectName: string,
  services: readonly string[],
  options: {
    composeArgs?: readonly string[];
    cwd?: string;
    env?: Record<string, string | undefined>;
    timeoutMs?: number;
  } = {},
): Promise<void> {
  const timeoutMs = options.timeoutMs ?? 60_000;
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    const statuses: string[] = [];

    for (const service of services) {
      const { stdout: containerId } = await runCommandCapture(
        dockerCommand,
        [
          "compose",
          ...(options.composeArgs ?? []),
          "-p",
          projectName,
          "ps",
          "-q",
          service,
        ],
        {
          ...(options.cwd
            ? {
                cwd: options.cwd,
              }
            : {}),
          ...(options.env
            ? {
                env: options.env,
              }
            : {}),
        },
      );
      const id = containerId.trim();

      if (id === "") {
        statuses.push("missing");
        continue;
      }

      const { stdout } = await runCommandCapture(
        dockerCommand,
        [
          "inspect",
          "--format",
          "{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}",
          id,
        ],
        {
          ...(options.cwd
            ? {
                cwd: options.cwd,
              }
            : {}),
          ...(options.env
            ? {
                env: options.env,
              }
            : {}),
        },
      );

      statuses.push(stdout.trim());
    }

    if (
      statuses.every((status) => status === "healthy" || status === "running")
    ) {
      return;
    }

    await delay(500);
  }

  throw new Error(
    `Timed out waiting for Docker services to become healthy: ${services.join(", ")}`,
  );
}

function registerShutdown(handler: () => Promise<void>) {
  let shuttingDown = false;
  const logger = createConsoleLogger();

  const run = async (exitCode = 0) => {
    if (shuttingDown) {
      return;
    }

    shuttingDown = true;

    try {
      await handler();
    } finally {
      process.exit(exitCode);
    }
  };

  process.on("SIGINT", () => {
    void run(0);
  });

  process.on("SIGTERM", () => {
    void run(0);
  });

  process.on("uncaughtException", (error) => {
    logger.error(formatError(error));
    void run(1);
  });

  process.on("unhandledRejection", (error) => {
    logger.error(formatError(error));
    void run(1);
  });

  return run;
}

export {
  ensureDocker,
  registerShutdown,
  runCommand,
  runCommandCapture,
  spawnCommand,
  waitForDockerServiceHealth,
  waitForReady,
};

export type { ChildProcessWithAde, CommandOptions };
