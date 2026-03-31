import { existsSync, readdirSync, statSync } from "node:fs";
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
const managementEndpointEnvName = "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT";
const sessionBundleRootEnvName = "ADE_SESSION_BUNDLE_ROOT";
const sessionSecretEnvName = "ADE_SESSION_SECRET";
const defaultContainerSessionBundleRoot = "/app/session-bundle";

function hostSessionBundleRoot(): string {
  const bundleRoot = fileURLToPath(
    new URL("../../.package/session-bundle", import.meta.url),
  );

  if (!existsSync(bundleRoot)) {
    throw new Error(
      "Missing local session bundle at .package/session-bundle. Run `pnpm package:session-bundle` first.",
    );
  }

  return bundleRoot;
}

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
  managementEndpoint: string;
  sessionBundleRoot: string;
  sessionSecret: string;
}): Record<string, string> {
  return {
    [appUrlEnvName]: options.appUrl,
    [configTargetsEnvName]: options.configTargets,
    [managementEndpointEnvName]: options.managementEndpoint,
    [sessionBundleRootEnvName]: options.sessionBundleRoot,
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

  return createSessionPoolValues({
    appUrl: options.appUrl ?? localContainerAppUrl,
    configTargets: createConfigTargetsValue(configWheelPath),
    managementEndpoint: createLocalSessionPoolManagementEndpoint(),
    sessionBundleRoot: hostSessionBundleRoot(),
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
  const sessionBundleRoot =
    readOptionalTrimmedString(env, sessionBundleRootEnvName) ??
    defaultContainerSessionBundleRoot;
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
        managementEndpoint: createLocalContainerSessionPoolManagementEndpoint(),
        sessionBundleRoot,
        sessionSecret: localSessionPoolSecret,
      }),
    };
  }

  return {
    usesManagedLocalSessionPool: false,
    values: createSessionPoolValues({
      appUrl,
      configTargets: readRequiredEnv(env, configTargetsEnvName),
      managementEndpoint: configuredManagementEndpoint,
      sessionBundleRoot,
      sessionSecret: readRequiredEnv(env, sessionSecretEnvName),
    }),
  };
}

export { createContainerSessionPoolEnv, createHostSessionPoolEnv };
