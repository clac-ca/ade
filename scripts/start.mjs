import process from "node:process";
import {
  loadOptionalEnvFile,
  openBrowser,
  parseArgs,
  readOptionalPort,
  registerShutdown,
  runCommand,
  runCommandCapture,
  spawnCommand,
  waitForReady,
} from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";
const defaultImage = process.env.ADE_IMAGE ?? "ade:local";
const runtimeEnvNames = [
  "HOST",
  "PORT",
  "AZURE_SQL_CONNECTIONSTRING",
  "AZURE_STORAGEBLOB_CONNECTIONSTRING",
  "AZURE_STORAGEBLOB_RESOURCEENDPOINT",
];

async function ensureDocker() {
  try {
    await runCommand(dockerCommand, ["info"], {
      stdio: "ignore",
    });
  } catch {
    throw new Error(
      "Docker is required for `pnpm start`, and the Docker daemon must be running.",
    );
  }
}

async function ensureImage(image) {
  try {
    await runCommand(dockerCommand, ["image", "inspect", image], {
      stdio: "ignore",
    });
  } catch {
    if (process.env.ADE_IMAGE) {
      throw new Error(
        `The configured image is not available locally: ${image}. Run \`docker pull ${image}\` or \`pnpm build\`.`,
      );
    }

    throw new Error("Run `pnpm build` first.");
  }
}

async function removeContainer(name) {
  try {
    await runCommand(dockerCommand, ["container", "rm", "--force", name], {
      stdio: "ignore",
    });
  } catch {
    // Ignore missing-container cleanup failures.
  }
}

async function isContainerRunning(name) {
  try {
    const { stdout } = await runCommandCapture(
      dockerCommand,
      ["container", "inspect", "--format", "{{.State.Running}}", name],
      {
        stdio: ["ignore", "pipe", "pipe"],
      },
    );

    return stdout.trim() === "true";
  } catch {
    return false;
  }
}

async function main() {
  loadOptionalEnvFile();

  const runtimePort = readOptionalPort(process.env.PORT) ?? 8000;
  const { port, noOpen } = parseArgs(process.argv.slice(2), {
    defaultPort: 8000,
    allowNoOpen: true,
  });
  const containerName = `ade-local-${port}`;
  const appUrl = `http://localhost:${port}`;
  const runtimeEnvArgs = runtimeEnvNames.flatMap((name) =>
    process.env[name]?.trim() ? ["--env", name] : [],
  );

  await ensureDocker();
  await ensureImage(defaultImage);
  await removeContainer(containerName);

  const container = spawnCommand(
    dockerCommand,
    [
      "run",
      "--rm",
      "--name",
      containerName,
      "--publish",
      `${port}:${runtimePort}`,
      ...runtimeEnvArgs,
      defaultImage,
    ],
    {
      stdio: "inherit",
    },
  );
  let shuttingDown = false;

  const shutdown = registerShutdown(async () => {
    shuttingDown = true;
    await removeContainer(containerName);
  });

  container.on("exit", (code, signal) => {
    if (shuttingDown) {
      return;
    }

    if (signal === "SIGINT" || signal === "SIGTERM" || signal === "SIGKILL") {
      return;
    }

    console.error(
      `Launcher child exited with code ${code ?? "unknown"}${signal ? ` and signal ${signal}` : ""}.`,
    );
    void shutdown(code ?? 1);
  });

  try {
    await waitForReady([`${appUrl}/`, `${appUrl}/api/readyz`], {
      timeoutMs: 60_000,
      isAlive: () =>
        container.exitCode === null &&
        container.signalCode === null &&
        !shuttingDown,
    });
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);

    if (await isContainerRunning(containerName)) {
      try {
        const { stdout } = await runCommandCapture(
          dockerCommand,
          ["logs", containerName],
          {
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        if (stdout.trim() !== "") {
          console.error(stdout.trim());
        }
      } catch {
        // Ignore log collection failures during startup cleanup.
      }
    }

    await shutdown(1);
    process.exit(1);
  }

  if (!noOpen) {
    openBrowser(appUrl);
  }

  console.log(`ADE is running at ${appUrl}`);

  await new Promise(() => {});
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
