import { join } from "node:path";
import process from "node:process";
import type { FastifyInstance } from "fastify";
import { createApp } from "./app";
import {
  defaultDevHost,
  defaultPort,
  defaultRuntimeHost,
  type BundledBuildInfo,
  readConfig,
  readServerConfig,
} from "./config";
import {
  createDatabaseService,
  type DatabaseService,
  type DatabaseServiceFactory,
} from "./db/service";
import { StartupError } from "./errors";
import { createConsoleLogger, formatError, runMain } from "./process";
import {
  createReadinessController,
  type ReadinessSnapshot,
} from "./readiness";

type ProcessHandle = {
  exit: (code: number) => void;
  on: (event: "SIGINT" | "SIGTERM", handler: () => void) => void;
};

export type ServerOptions = {
  buildInfo: BundledBuildInfo;
  host: string;
  logger?: boolean;
  port: number;
  probeIntervalMs?: number;
  sqlConnectionString: string;
  sqlServiceFactory?: DatabaseServiceFactory;
  staleAfterMs?: number;
  webRoot?: string;
};

export type ServerInstance = {
  app: FastifyInstance;
  readiness: () => ReadinessSnapshot;
  start: () => Promise<void>;
  stop: () => Promise<void>;
};

type ManagedServer = Pick<ServerInstance, "start" | "stop">;

function createServer({
  buildInfo,
  host,
  logger = true,
  port,
  probeIntervalMs,
  sqlConnectionString,
  sqlServiceFactory,
  staleAfterMs,
  webRoot,
}: ServerOptions): ServerInstance {
  const readiness = createReadinessController(
    staleAfterMs === undefined
      ? {}
      : {
          staleAfterMs,
        },
  );
  const app = createApp({
    buildInfo,
    getReadinessSnapshot: () => readiness.snapshot(),
    logger,
    ...(webRoot
      ? {
          webRoot,
        }
      : {}),
  });
  const createService = sqlServiceFactory ?? createDatabaseService;
  const probeDelayMs = probeIntervalMs ?? 5_000;
  let databaseService: DatabaseService | null = null;
  let probeInFlight: Promise<void> | null = null;
  let probeInterval: NodeJS.Timeout | null = null;
  let closing = false;

  async function closeService(): Promise<void> {
    if (!databaseService) {
      return;
    }

    const service = databaseService;
    databaseService = null;
    await service.close();
  }

  async function runProbe(source: "interval" | "startup"): Promise<void> {
    if (!databaseService) {
      throw new StartupError(
        "SQL service is not available for readiness probing.",
      );
    }

    if (probeInFlight) {
      return probeInFlight;
    }

    probeInFlight = (async () => {
      const previousPhase = readiness.snapshot().phase;

      try {
        await databaseService.ping();
        readiness.recordDatabaseSuccess();
        readiness.markReady();

        if (source === "interval" && previousPhase === "degraded") {
          app.log.info("SQL readiness probe recovered.");
        }
      } catch (error) {
        readiness.recordDatabaseFailure(error);
        readiness.markDegraded(error);

        if (source === "startup") {
          throw new StartupError(
            "Failed to verify SQL connectivity during startup.",
            error,
          );
        }

        app.log.error(
          {
            err: error,
          },
          "SQL readiness probe failed.",
        );
      } finally {
        probeInFlight = null;
      }
    })();

    return probeInFlight;
  }

  async function start() {
    closing = false;
    readiness.markStarting();

    try {
      databaseService = await createService(sqlConnectionString);
    } catch (error) {
      throw new StartupError("Failed to initialize SQL.", error);
    }

    try {
      await runProbe("startup");
      await app.listen({
        host,
        port,
      });
      probeInterval = setInterval(() => {
        if (closing) {
          return;
        }

        void runProbe("interval");
      }, probeDelayMs);
      probeInterval.unref();
    } catch (error) {
      await closeService().catch((closeError: unknown) => {
        app.log.error(
          {
            err: closeError,
          },
          "Failed to close SQL after startup failure.",
        );
      });
      throw error instanceof StartupError
        ? error
        : new StartupError("Failed to start ADE runtime.", error);
    }
  }

  async function stop() {
    closing = true;
    readiness.markStopping();

    if (probeInterval) {
      clearInterval(probeInterval);
      probeInterval = null;
    }

    await probeInFlight?.catch(() => undefined);
    await app.close();
    await closeService();
  }

  return {
    app,
    readiness: () => readiness.snapshot(),
    start,
    stop,
  };
}

function createProductionServer(
  argv: readonly string[] = process.argv.slice(2),
  env = process.env,
): ServerInstance {
  const config = readConfig(env, {
    requireSql: true,
  });
  const serverConfig = readServerConfig(argv, {
    host: env["NODE_ENV"] === "production" ? defaultRuntimeHost : defaultDevHost,
    port: defaultPort,
  });

  return createServer({
    buildInfo: config.buildInfo,
    host: serverConfig.host,
    port: serverConfig.port,
    sqlConnectionString: config.sqlConnectionString,
    webRoot: join(__dirname, "..", "public"),
  });
}

async function runServer(
  processHandle: ProcessHandle = process,
  runtime: ManagedServer = createProductionServer(),
  logger = createConsoleLogger(),
) {
  let shuttingDown = false;

  async function stop(exitCode: number) {
    if (shuttingDown) {
      return;
    }

    shuttingDown = true;

    try {
      await runtime.stop();
      processHandle.exit(exitCode);
    } catch (error) {
      logger.error(formatError(error));
      processHandle.exit(1);
    }
  }

  processHandle.on("SIGINT", () => {
    void stop(0);
  });

  processHandle.on("SIGTERM", () => {
    void stop(0);
  });

  try {
    await runtime.start();
  } catch (error) {
    logger.error(formatError(error));
    await stop(1);
  }
}

if (require.main === module) {
  void runMain(async () => {
    await runServer();
  });
}

export { createProductionServer, createServer, runServer };
