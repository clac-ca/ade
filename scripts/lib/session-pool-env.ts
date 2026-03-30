import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  localContainerAppUrl,
  localSessionPoolSecret,
} from "./dev-config";
import { readOptionalTrimmedString } from "./runtime";

const appUrlEnvName = "ADE_APP_URL";
const configTargetsEnvName = "ADE_CONFIG_TARGETS";
const engineWheelEnvName = "ADE_ENGINE_WHEEL_PATH";
const managementEndpointEnvName = "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT";
const sessionSecretEnvName = "ADE_SESSION_SECRET";

function newestWheel(directoryPath: string, prefix: string): string {
  const candidates = readdirSync(directoryPath)
    .filter((name) => name.startsWith(prefix) && name.endsWith(".whl"))
    .map((name) => ({
      modifiedMs: statSync(join(directoryPath, name)).mtimeMs,
      path: join(directoryPath, name),
    }))
    .sort((left, right) => right.modifiedMs - left.modifiedMs);

  const latest = candidates[0];
  if (latest === undefined) {
    throw new Error(`No wheel found in ${directoryPath} with prefix ${prefix}`);
  }

  return latest.path;
}

function readRequiredEnv(env: NodeJS.ProcessEnv, name: string): string {
  const value = readOptionalTrimmedString(env, name);
  if (value === undefined) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

function createConfigTargetsValue(configWheelPath: string): string {
  return JSON.stringify([
    {
      configVersionId: "config-v1",
      wheelPath: configWheelPath,
      workspaceId: "workspace-a",
    },
    {
      configVersionId: "config-v2",
      wheelPath: configWheelPath,
      workspaceId: "workspace-b",
    },
  ]);
}

function createSessionPoolValues(options: {
  appUrl: string;
  configTargets: string;
  engineWheelPath: string;
  managementEndpoint: string;
  sessionSecret: string;
}): Record<string, string> {
  return {
    [appUrlEnvName]: options.appUrl,
    [configTargetsEnvName]: options.configTargets,
    [engineWheelEnvName]: options.engineWheelPath,
    [managementEndpointEnvName]: options.managementEndpoint,
    [sessionSecretEnvName]: options.sessionSecret,
  };
}

function createHostSessionPoolEnv(
  options: {
    appUrl?: string;
  } = {},
): Record<string, string> {
  const configWheelPath = newestWheel(
    fileURLToPath(new URL("../../packages/ade-config/dist", import.meta.url)),
    "ade_config-",
  );
  const engineWheelPath = newestWheel(
    fileURLToPath(new URL("../../packages/ade-engine/dist", import.meta.url)),
    "ade_engine-",
  );

  return createSessionPoolValues({
    appUrl: options.appUrl ?? localContainerAppUrl,
    configTargets: createConfigTargetsValue(configWheelPath),
    engineWheelPath,
    managementEndpoint: createLocalSessionPoolManagementEndpoint(),
    sessionSecret: localSessionPoolSecret,
  });
}

function createContainerSessionPoolEnv(
  env: NodeJS.ProcessEnv = process.env,
  options: {
    appUrl?: string;
  } = {},
): {
  usesManagedLocalSessionPool: boolean;
  values: Record<string, string>;
} {
  const engineWheelPath =
    readOptionalTrimmedString(env, engineWheelEnvName) ??
    "/app/python/ade_engine.whl";
  const appUrl =
    readOptionalTrimmedString(env, appUrlEnvName) ??
    options.appUrl ??
    localContainerAppUrl;

  const configuredManagementEndpoint = readOptionalTrimmedString(
    env,
    managementEndpointEnvName,
  );
  if (configuredManagementEndpoint === undefined) {
    return {
      usesManagedLocalSessionPool: true,
      values: createSessionPoolValues({
        appUrl,
        configTargets: createConfigTargetsValue("/app/python/ade_config.whl"),
        engineWheelPath,
        managementEndpoint: createLocalContainerSessionPoolManagementEndpoint(),
        sessionSecret: localSessionPoolSecret,
      }),
    };
  }

  return {
    usesManagedLocalSessionPool: false,
    values: createSessionPoolValues({
      appUrl,
      configTargets: readRequiredEnv(env, configTargetsEnvName),
      engineWheelPath,
      managementEndpoint: configuredManagementEndpoint,
      sessionSecret: readRequiredEnv(env, sessionSecretEnvName),
    }),
  };
}

export { createContainerSessionPoolEnv, createHostSessionPoolEnv };
