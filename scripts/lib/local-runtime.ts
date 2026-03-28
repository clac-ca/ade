import process from "node:process";
import { createLocalContainerSqlConnectionString } from "./dev-config";
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
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

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
  const effectiveSqlConnectionString =
    options.sqlConnectionString ?? createLocalContainerSqlConnectionString();

  if (usesManagedLocalSql) {
    options.logger?.info("Starting managed local SQL.");
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
        hostPort: options.hostPort,
        image: options.image,
      }),
      {
        env: {
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

        if (usesManagedLocalSql) {
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

        if (usesManagedLocalSql) {
          await downLocalDependencies({
            stdio: "ignore",
          }).catch(() => undefined);
        }
      },
      usesManagedLocalSql,
    };
  } catch (error) {
    if (usesManagedLocalSql) {
      await downLocalDependencies({
        stdio: "ignore",
      }).catch(() => undefined);
    }

    throw error;
  }
}

export { startLocalRuntime };

export type { StartedLocalRuntime };
