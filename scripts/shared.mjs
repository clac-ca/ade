import { createHash } from "node:crypto";
import { basename } from "node:path";
import { spawn } from "node:child_process";
import { appendFileSync, existsSync, writeFileSync } from "node:fs";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";

const devPortOffsets = {
  api: 1,
  azuriteBlob: 10,
  azuriteQueue: 11,
  azuriteTable: 12,
  sql: 13,
  web: 0,
};
const maxDevPortOffset = Math.max(...Object.values(devPortOffsets));

export function loadOptionalEnvFile(path = ".env") {
  if (!existsSync(path)) {
    return;
  }

  process.loadEnvFile(path);
}

export function readOptionalPort(value, name = "PORT") {
  if (value === undefined) {
    return undefined;
  }

  const rawValue = value.trim();

  if (rawValue === "") {
    return undefined;
  }

  if (!/^[1-9]\d*$/.test(rawValue)) {
    throw new Error(
      `${name} must be a positive integer, received: ${rawValue}`,
    );
  }

  const parsed = Number.parseInt(rawValue, 10);

  if (parsed > 65_535) {
    throw new Error(`${name} must be 65535 or lower, received: ${rawValue}`);
  }

  return parsed;
}

export function parseArgs(argv, options = {}) {
  const { defaultPort, allowNoOpen = false } = options;

  let port = defaultPort;
  let noOpen = false;

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];

    if (arg === "--") {
      continue;
    }

    if (arg === "--port") {
      const value = argv[index + 1];

      if (value === undefined) {
        throw new Error("Missing value for --port");
      }

      if (!/^[1-9]\d*$/.test(value)) {
        throw new Error(`Invalid port: ${value}`);
      }

      const parsed = Number.parseInt(value, 10);

      if (parsed > 65_535) {
        throw new Error(`Invalid port: ${value}`);
      }

      port = parsed;
      index += 1;
      continue;
    }

    if (allowNoOpen && arg === "--no-open") {
      noOpen = true;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return {
    port,
    noOpen,
  };
}

export function spawnCommand(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    detached: options.detached ?? false,
    env: {
      ...process.env,
      ...options.env,
    },
    stdio: options.stdio ?? "inherit",
  });

  child.adeDetached = options.detached ?? false;

  return child;
}

export async function runCommand(command, args, options = {}) {
  await new Promise((resolve, reject) => {
    const child = spawnCommand(command, args, {
      cwd: options.cwd,
      detached: options.detached,
      env: options.env,
      stdio: options.stdio ?? "inherit",
    });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} exited with signal ${signal}`));
        return;
      }

      if (code !== 0) {
        reject(new Error(`${command} exited with code ${code ?? "unknown"}`));
        return;
      }

      resolve(undefined);
    });
  });
}

export async function runCommandCapture(command, args, options = {}) {
  return await new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const child = spawnCommand(command, args, {
      cwd: options.cwd,
      detached: options.detached,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    child.stdout?.setEncoding("utf8");
    child.stdout?.on("data", (chunk) => {
      stdout += chunk;
    });

    child.stderr?.setEncoding("utf8");
    child.stderr?.on("data", (chunk) => {
      stderr += chunk;
    });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (signal !== null) {
        reject(new Error(`${command} exited with signal ${signal}`));
        return;
      }

      if (code !== 0) {
        const detail = stderr.trim();
        reject(
          new Error(
            detail
              ? detail
              : `${command} exited with code ${code ?? "unknown"}`,
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

export async function waitForReady(urls, options = {}) {
  const timeoutMs = options.timeoutMs ?? 30_000;
  const startedAt = Date.now();

  while (Date.now() - startedAt < timeoutMs) {
    if (options.isAlive && !options.isAlive()) {
      throw new Error("A required process exited before ADE became ready.");
    }

    const checks = await Promise.all(
      urls.map(async (url) => {
        try {
          const response = await fetch(url);
          return response.ok;
        } catch {
          return false;
        }
      }),
    );

    if (checks.every(Boolean)) {
      return;
    }

    await delay(250);
  }

  throw new Error(`Timed out waiting for: ${urls.join(", ")}`);
}

export async function ensureDocker(command) {
  try {
    await runCommand(command, ["info"], {
      stdio: "ignore",
    });
  } catch {
    throw new Error(
      "Docker is required for this command, and the Docker daemon must be running.",
    );
  }
}

export function createDevProjectName(rootDir) {
  const safeBase =
    basename(rootDir)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 20) || "ade";
  const hash = createHash("sha256").update(rootDir).digest("hex").slice(0, 8);

  return `ade-${safeBase}-${hash}`;
}

export function createDevPorts(basePort) {
  if (!Number.isInteger(basePort) || basePort < 1) {
    throw new Error(`Invalid port: ${basePort}`);
  }

  if (basePort > 65_535 - maxDevPortOffset) {
    throw new Error(
      `Invalid port: ${basePort}. ADE dev requires --port ${65_535 - maxDevPortOffset} or lower.`,
    );
  }

  return {
    api: basePort + devPortOffsets.api,
    azuriteBlob: basePort + devPortOffsets.azuriteBlob,
    azuriteQueue: basePort + devPortOffsets.azuriteQueue,
    azuriteTable: basePort + devPortOffsets.azuriteTable,
    sql: basePort + devPortOffsets.sql,
    web: basePort + devPortOffsets.web,
  };
}

export function createLocalSqlPassword(projectName) {
  return `AdeLocal1!${projectName.slice(-8)}`;
}

export function createAzuriteBlobConnectionString(ports) {
  return [
    "DefaultEndpointsProtocol=http",
    "AccountName=devstoreaccount1",
    "AccountKey=Eby8vdM02xNOcqFeqCnf2fV4i+7VQ8Jtq/K1SZFPTOtr/KBHBeksoGMGwP/IQ+J4MJBxRcQ6vL6gE0Gv6hA==",
    `BlobEndpoint=http://127.0.0.1:${ports.azuriteBlob}/devstoreaccount1`,
    `QueueEndpoint=http://127.0.0.1:${ports.azuriteQueue}/devstoreaccount1`,
    `TableEndpoint=http://127.0.0.1:${ports.azuriteTable}/devstoreaccount1`,
  ].join(";");
}

export function createLocalSqlConnectionString({ database, password, port }) {
  return [
    `Server=127.0.0.1,${port}`,
    `Database=${database}`,
    "User Id=sa",
    `Password=${password}`,
    "Encrypt=false",
    "TrustServerCertificate=true",
  ].join(";");
}

export async function waitForDockerServiceHealth(
  dockerCommand,
  projectName,
  services,
  options = {},
) {
  const timeoutMs = options.timeoutMs ?? 60_000;
  const startedAt = Date.now();
  const env = {
    ...process.env,
    ...options.env,
  };
  const composeArgs = options.composeArgs ?? [];

  while (Date.now() - startedAt < timeoutMs) {
    const statuses = [];

    for (const service of services) {
      const { stdout: containerId } = await runCommandCapture(
        dockerCommand,
        ["compose", ...composeArgs, "-p", projectName, "ps", "-q", service],
        {
          cwd: options.cwd,
          env,
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
          cwd: options.cwd,
          env,
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

export function registerShutdown(handler) {
  let shuttingDown = false;

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
    console.error(error);
    void run(1);
  });

  process.on("unhandledRejection", (error) => {
    console.error(error);
    void run(1);
  });

  return run;
}

export function openBrowser(url) {
  const platform = process.platform;

  if (platform === "darwin") {
    const child = spawn("open", [url], {
      detached: true,
      stdio: "ignore",
    });
    child.on("error", () => {});
    child.unref();
    return;
  }

  if (platform === "win32") {
    const child = spawn("cmd", ["/c", "start", "", url], {
      detached: true,
      stdio: "ignore",
    });
    child.on("error", () => {});
    child.unref();
    return;
  }

  const child = spawn("xdg-open", [url], {
    detached: true,
    stdio: "ignore",
  });
  child.on("error", () => {});
  child.unref();
}

export function requireEnv(name, options = {}) {
  const value = process.env[name];

  if (typeof value === "string" && value.trim() !== "") {
    return value.trim();
  }

  if (options.allowEmpty) {
    return "";
  }

  throw new Error(`Missing required environment variable: ${name}`);
}

export function writeGitHubOutput(values) {
  const outputPath = process.env.GITHUB_OUTPUT;

  if (!outputPath) {
    return;
  }

  const lines = Object.entries(values).map(
    ([key, value]) => `${key}=${String(value)}`,
  );
  appendFileSync(outputPath, `${lines.join("\n")}\n`);
}

export function writeJsonFile(path, value) {
  writeFileSync(path, JSON.stringify(value, null, 2) + "\n");
}
