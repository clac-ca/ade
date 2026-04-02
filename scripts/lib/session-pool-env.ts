import { existsSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  localContainerAppUrl,
  localSessionPoolSecret,
} from "./dev-config";
import { readOptionalTrimmedString } from "./runtime";

const appUrlEnvName = "ADE_PUBLIC_API_URL";
const managementEndpointEnvName = "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT";
const legacySessionSecretEnvName = "ADE_SCOPE_SESSION_SECRET";
const sessionSecretEnvName = "ADE_SANDBOX_ENVIRONMENT_SECRET";

function ensureLocalSessionArtifacts(): void {
  const sandboxEnvironmentArchive = fileURLToPath(
    new URL("../../.package/sandbox-environment.tar.gz", import.meta.url),
  );

  if (!existsSync(sandboxEnvironmentArchive)) {
    throw new Error(
      "Missing local sandbox environment archive under .package/. Run `pnpm dev` first.",
    );
  }
}

function readRequiredEnv(env: NodeJS.ProcessEnv, name: string): string {
  const value =
    readOptionalTrimmedString(env, name) ??
    (name === sessionSecretEnvName
      ? readOptionalTrimmedString(env, legacySessionSecretEnvName)
      : undefined);
  if (value === undefined) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

function createSessionPoolValues(options: {
  appUrl: string;
  managementEndpoint: string;
  sessionSecret: string;
}): Record<string, string> {
  return {
    [appUrlEnvName]: options.appUrl,
    [managementEndpointEnvName]: options.managementEndpoint,
    [sessionSecretEnvName]: options.sessionSecret,
  };
}

function createHostSessionPoolEnv(
  options: {
    appUrl?: string;
  } = {},
): Record<string, string> {
  ensureLocalSessionArtifacts();

  return createSessionPoolValues({
    appUrl: options.appUrl ?? localContainerAppUrl,
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
        managementEndpoint: createLocalContainerSessionPoolManagementEndpoint(),
        sessionSecret: localSessionPoolSecret,
      }),
    };
  }

  return {
    usesManagedLocalSessionPool: false,
    values: createSessionPoolValues({
      appUrl,
      managementEndpoint: configuredManagementEndpoint,
      sessionSecret: readRequiredEnv(env, sessionSecretEnvName),
    }),
  };
}

export { createContainerSessionPoolEnv, createHostSessionPoolEnv };
