import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { parseArgs as parseNodeArgs } from "node:util";
import { ConfigError } from "./errors";

type EnvBag = Record<string, string | undefined>;

type BundledBuildInfo = {
  builtAt: string;
  gitSha: string;
  service: "ade";
  version: string;
};

type VersionInfo = BundledBuildInfo & {
  nodeVersion: string;
};

type ApiConfig = {
  buildInfo: BundledBuildInfo;
  sqlConnectionString?: string;
};

type MigrationConfig = {
  sqlConnectionString: string;
};

type ReadConfigOptions = {
  buildInfoPath?: string;
  requireSql?: boolean;
};

type ServerConfig = {
  host: string;
  port: number;
};

const packagePath = join(__dirname, "..", "package.json");
const bundledBuildInfoPath = join(__dirname, "build-info.json");
const defaultDevHost = "127.0.0.1";
const defaultRuntimeHost = "0.0.0.0";
const defaultPort = 8000;
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

function readOptionalTrimmedString(
  env: EnvBag,
  name: string,
): string | undefined {
  const value = env[name];

  if (value === undefined) {
    return undefined;
  }

  const trimmed = value.trim();
  return trimmed === "" ? undefined : trimmed;
}

function readRequiredTrimmedString(env: EnvBag, name: string): string {
  const value = readOptionalTrimmedString(env, name);

  if (value === undefined) {
    throw new ConfigError(`Missing required environment variable: ${name}`);
  }

  return value;
}

function readPort(value: string | undefined, name: string): number {
  if (value === undefined) {
    return defaultPort;
  }

  const trimmed = value.trim();

  if (!/^[1-9]\d*$/.test(trimmed)) {
    throw new ConfigError(`${name} must be a positive integer, received: ${trimmed}`);
  }

  const port = Number.parseInt(trimmed, 10);

  if (port > 65_535) {
    throw new ConfigError(`${name} must be 65535 or lower, received: ${trimmed}`);
  }

  return port;
}

function readDevelopmentBuildInfo(): BundledBuildInfo {
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8")) as {
    version?: unknown;
  };

  if (
    typeof packageJson.version !== "string" ||
    packageJson.version.trim() === ""
  ) {
    throw new ConfigError(
      "ADE package.json version must be a non-empty string.",
    );
  }

  return {
    builtAt:
      readGitValue(["show", "--no-patch", "--format=%cI", "HEAD"]) ?? "dev",
    gitSha: readGitValue(["rev-parse", "HEAD"]) ?? "dev",
    service: "ade",
    version: packageJson.version,
  };
}

function readGitValue(args: string[]) {
  try {
    return execFileSync("git", args, {
      cwd: join(__dirname, ".."),
      encoding: "utf8",
    }).trim();
  } catch {
    return null;
  }
}

function validateBuildInfo(value: unknown): BundledBuildInfo {
  if (!value || typeof value !== "object") {
    throw new ConfigError("ADE build info must be an object.");
  }

  const buildInfo = value as Record<string, unknown>;

  for (const key of ["service", "version", "gitSha", "builtAt"]) {
    const field = buildInfo[key];

    if (typeof field !== "string" || field.trim() === "") {
      throw new ConfigError(
        `ADE build info field "${key}" must be a non-empty string.`,
      );
    }
  }

  if (buildInfo["service"] !== "ade") {
    throw new ConfigError('ADE build info service must be "ade".');
  }

  return {
    builtAt: buildInfo["builtAt"] as string,
    gitSha: buildInfo["gitSha"] as string,
    service: "ade",
    version: buildInfo["version"] as string,
  };
}

function readBuildInfo(
  env: EnvBag,
  options: ReadConfigOptions,
): BundledBuildInfo {
  const buildInfoPath = options.buildInfoPath ?? bundledBuildInfoPath;

  if (existsSync(buildInfoPath)) {
    return validateBuildInfo(JSON.parse(readFileSync(buildInfoPath, "utf8")));
  }

  if (env["NODE_ENV"] === "production") {
    throw new ConfigError(`Missing ADE build info at ${buildInfoPath}.`);
  }

  return readDevelopmentBuildInfo();
}

function readConfig(
  env: EnvBag,
  options: ReadConfigOptions & {
    requireSql: true;
  },
): ApiConfig & {
  sqlConnectionString: string;
};
function readConfig(env?: EnvBag, options?: ReadConfigOptions): ApiConfig;
function readConfig(
  env: EnvBag = process.env,
  options: ReadConfigOptions = {},
): ApiConfig {
  const sqlConnectionString = options.requireSql
    ? readRequiredTrimmedString(env, sqlConnectionStringName)
    : readOptionalTrimmedString(env, sqlConnectionStringName);

  return {
    buildInfo: readBuildInfo(env, options),
    ...(sqlConnectionString
      ? {
          sqlConnectionString,
        }
      : {}),
  };
}

function readMigrationConfig(env: EnvBag = process.env): MigrationConfig {
  return {
    sqlConnectionString: readRequiredTrimmedString(env, sqlConnectionStringName),
  };
}

function readServerConfig(
  argv: readonly string[],
  defaults: {
    host?: string;
    port?: number;
  } = {},
): ServerConfig {
  const { values } = parseNodeArgs({
    allowPositionals: false,
    args: argv,
    options: {
      host: {
        type: "string",
      },
      port: {
        type: "string",
      },
    },
    strict: true,
  });

  return {
    host: values.host?.trim() || defaults.host || defaultDevHost,
    port:
      values.port === undefined
        ? (defaults.port ?? defaultPort)
        : readPort(values.port, "--port"),
  };
}

export { defaultDevHost, defaultRuntimeHost, defaultPort, readConfig, readMigrationConfig, readServerConfig, sqlConnectionStringName };

export type {
  ApiConfig,
  BundledBuildInfo,
  EnvBag,
  MigrationConfig,
  ReadConfigOptions,
  ServerConfig,
  VersionInfo,
};
