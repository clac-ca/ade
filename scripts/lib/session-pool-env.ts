import { existsSync } from "node:fs";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  localContainerAppUrl,
  localSessionPoolBearerToken,
  localSessionPoolSecret,
} from "./dev-config";
import { readOptionalTrimmedString } from "./runtime";

const appUrlEnvName = "ADE_PUBLIC_API_URL";
const bearerTokenEnvName = "ADE_SESSION_POOL_BEARER_TOKEN";
const managementEndpointEnvName = "ADE_SESSION_POOL_MANAGEMENT_ENDPOINT";
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

function createSessionPoolValues(options: {
  appUrl: string;
  bearerToken?: string;
  managementEndpoint: string;
  sessionSecret?: string;
}): Record<string, string> {
  return {
    [appUrlEnvName]: options.appUrl,
    ...(options.bearerToken
      ? {
          [bearerTokenEnvName]: options.bearerToken,
        }
      : {}),
    [managementEndpointEnvName]: options.managementEndpoint,
    ...(options.sessionSecret
      ? {
          [sessionSecretEnvName]: options.sessionSecret,
        }
      : {}),
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
    bearerToken: localSessionPoolBearerToken,
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
        bearerToken: localSessionPoolBearerToken,
        managementEndpoint: createLocalContainerSessionPoolManagementEndpoint(),
        sessionSecret: localSessionPoolSecret,
      }),
    };
  }

  const bearerToken = readOptionalTrimmedString(env, bearerTokenEnvName);
  const sessionSecret = readOptionalTrimmedString(env, sessionSecretEnvName);

  return {
    usesManagedLocalSessionPool: false,
    values: createSessionPoolValues({
      appUrl,
      managementEndpoint: configuredManagementEndpoint,
      ...(bearerToken ? { bearerToken } : {}),
      ...(sessionSecret ? { sessionSecret } : {}),
    }),
  };
}

export { createContainerSessionPoolEnv, createHostSessionPoolEnv };
