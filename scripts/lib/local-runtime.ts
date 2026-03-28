import process from "node:process";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalContainerSessionPoolMcpEndpoint,
  createLocalContainerSqlConnectionString,
  localSessionPoolRuntimeSecret,
} from "./dev-config";
import type { Logger } from "./runtime";
import { createMigrationRunArgs, createContainerRunArgs } from "./start";
import {
  ensureDocker,
  runCommand,
  runCommandCapture,
  spawnCommand,
  type ChildProcessWithAde,
} from "./shell";
import {
  downLocalDependencies,
  readLocalDependencyLogs,
  upLocalDependencies,
} from "../local-deps";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const azureRuntimeEnvNames = [
  "ADE_RUNTIME_SESSION_SECRET",
  "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT",
  "ADE_SESSION_POOL_MCP_ENDPOINT",
  "ADE_SESSION_POOL_RESOURCE_ID",
];
const runtimeSessionSecretName = "ADE_RUNTIME_SESSION_SECRET";
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

function readOptionalTrimmedString(
  env: NodeJS.ProcessEnv,
  name: string,
): string | undefined {
  const value = env[name]?.trim();
  return value === "" || value === undefined ? undefined : value;
}

function readRequiredTrimmedString(
  env: NodeJS.ProcessEnv,
  name: string,
): string {
  const value = readOptionalTrimmedString(env, name);
  if (value === undefined) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

function readContainerRuntimeEnv(): {
  usesManagedLocalSessionPool: boolean;
  values: Record<string, string>;
} {
  const azureRuntimeConfigured =
    readOptionalTrimmedString(process.env, "ADE_SESSION_POOL_RESOURCE_ID") !==
    undefined;

  const values: Record<string, string> = {};
  const runtimeSessionSecret =
    readOptionalTrimmedString(process.env, runtimeSessionSecretName) ??
    localSessionPoolRuntimeSecret;

  if (!azureRuntimeConfigured) {
    values[runtimeSessionSecretName] = runtimeSessionSecret;
    values["ADE_SESSION_POOL_MANAGEMENT_ENDPOINT"] =
      createLocalContainerSessionPoolManagementEndpoint();
    values["ADE_SESSION_POOL_MCP_ENDPOINT"] =
      createLocalContainerSessionPoolMcpEndpoint();
    return {
      usesManagedLocalSessionPool: true,
      values,
    };
  }

  for (const name of azureRuntimeEnvNames) {
    values[name] = readRequiredTrimmedString(process.env, name);
  }

  return {
    usesManagedLocalSessionPool: false,
    values,
  };
}

type StartedLocalRuntime = {
  appUrl: string;
  container: ChildProcessWithAde;
  dumpLogs: () => Promise<string>;
  isAlive: () => boolean;
  stop: () => Promise<void>;
  usesManagedLocalSql: boolean;
};

async function removeContainer(name: string): Promise<void> {
  try {
    await runCommand(dockerCommand, ["container", "rm", "--force", name], {
      stdio: "ignore",
    });
  } catch {
    return;
  }
}

async function ensureImageAvailable(image: string): Promise<void> {
  await runCommand(dockerCommand, ["image", "inspect", image], {
    stdio: "ignore",
  }).catch(() => {
    throw new Error(
      image === "ade-platform:local"
        ? "Run `pnpm build` first."
        : `The configured image is not available locally: ${image}. Run \`docker pull ${image}\` or choose a local image.`,
    );
  });
}

async function readContainerLogs(name: string): Promise<string> {
  try {
    const { stdout } = await runCommandCapture(dockerCommand, ["logs", name]);
    return stdout.trim();
  } catch {
    return "";
  }
}

async function runMigrations(options: {
  image: string;
  sqlConnectionString: string;
}): Promise<void> {
  await runCommand(dockerCommand, createMigrationRunArgs(options), {
    env: {
      [sqlConnectionStringName]: options.sqlConnectionString,
    },
  });
}

async function startLocalRuntime(options: {
  containerName: string;
  hostPort: number;
  image: string;
  logger?: Logger;
  sqlConnectionString: string | undefined;
  usage: string;
}): Promise<StartedLocalRuntime> {
  await ensureDocker(dockerCommand, options.usage);
  await ensureImageAvailable(options.image);

  const usesManagedLocalSql = options.sqlConnectionString === undefined;
  const { usesManagedLocalSessionPool, values: runtimeEnv } =
    readContainerRuntimeEnv();
  const effectiveSqlConnectionString =
    options.sqlConnectionString ?? createLocalContainerSqlConnectionString();

  if (usesManagedLocalSql || usesManagedLocalSessionPool) {
    options.logger?.info(
      usesManagedLocalSql
        ? "Starting managed local SQL and session pool."
        : "Starting the managed local session pool.",
    );
    await upLocalDependencies();
  }

  try {
    if (usesManagedLocalSql) {
      options.logger?.info(
        "Running the separate ade-migrate step before app startup.",
      );
      await runMigrations({
        image: options.image,
        sqlConnectionString: effectiveSqlConnectionString,
      });
    }

    await removeContainer(options.containerName);
    options.logger?.info("Starting the app container.");

    const container = spawnCommand(
      dockerCommand,
      createContainerRunArgs({
        containerName: options.containerName,
        envNames: Object.keys(runtimeEnv),
        hostPort: options.hostPort,
        image: options.image,
      }),
      {
        env: {
          ...runtimeEnv,
          [sqlConnectionStringName]: effectiveSqlConnectionString,
        },
      },
    );

    return {
      appUrl: `http://127.0.0.1:${String(options.hostPort)}`,
      container,
      dumpLogs: async () => {
        const sections: string[] = [];

        const appLogs = await readContainerLogs(options.containerName);

        if (appLogs !== "") {
          sections.push(appLogs);
        }

        if (usesManagedLocalSql || usesManagedLocalSessionPool) {
          const dependencyLogs = await readLocalDependencyLogs().catch(
            () => "",
          );

          if (dependencyLogs !== "") {
            sections.push(dependencyLogs);
          }
        }

        return sections.join("\n\n").trim();
      },
      isAlive: () =>
        container.exitCode === null && container.signalCode === null,
      stop: async () => {
        await removeContainer(options.containerName);

        if (usesManagedLocalSql || usesManagedLocalSessionPool) {
          await downLocalDependencies({
            stdio: "ignore",
          }).catch(() => undefined);
        }
      },
      usesManagedLocalSql,
    };
  } catch (error) {
    if (usesManagedLocalSql || usesManagedLocalSessionPool) {
      await downLocalDependencies({
        stdio: "ignore",
      }).catch(() => undefined);
    }

    throw error;
  }
}

export { startLocalRuntime };

export type { StartedLocalRuntime };
