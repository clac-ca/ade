import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  localSessionPoolSecret,
} from "./dev-config";
import { readOptionalTrimmedString } from "./runtime";

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

function createHostSessionPoolEnv(): Record<string, string> {
  const configWheelPath = newestWheel(
    fileURLToPath(new URL("../../packages/ade-config/dist", import.meta.url)),
    "ade_config-",
  );
  const engineWheelPath = newestWheel(
    fileURLToPath(new URL("../../packages/ade-engine/dist", import.meta.url)),
    "ade_engine-",
  );

  return {
    [configTargetsEnvName]: createConfigTargetsValue(configWheelPath),
    [sessionSecretEnvName]: localSessionPoolSecret,
    [managementEndpointEnvName]: createLocalSessionPoolManagementEndpoint(),
    [engineWheelEnvName]: engineWheelPath,
  };
}

function createContainerSessionPoolEnv(env: NodeJS.ProcessEnv = process.env): {
  usesManagedLocalSessionPool: boolean;
  values: Record<string, string>;
} {
  const values: Record<string, string> = {
    [engineWheelEnvName]:
      readOptionalTrimmedString(env, engineWheelEnvName) ??
      "/app/python/ade_engine.whl",
  };

  const configuredManagementEndpoint = readOptionalTrimmedString(
    env,
    managementEndpointEnvName,
  );
  if (configuredManagementEndpoint === undefined) {
    return {
      usesManagedLocalSessionPool: true,
      values: {
        ...values,
        [configTargetsEnvName]: createConfigTargetsValue(
          "/app/python/ade_config.whl",
        ),
        [sessionSecretEnvName]: localSessionPoolSecret,
        [managementEndpointEnvName]:
          createLocalContainerSessionPoolManagementEndpoint(),
      },
    };
  }

  return {
    usesManagedLocalSessionPool: false,
    values: {
      ...values,
      [configTargetsEnvName]: readRequiredEnv(env, configTargetsEnvName),
      [managementEndpointEnvName]: configuredManagementEndpoint,
      [sessionSecretEnvName]: readRequiredEnv(env, sessionSecretEnvName),
    },
  };
}

export { createContainerSessionPoolEnv, createHostSessionPoolEnv };
