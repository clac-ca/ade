import process from "node:process";
import {
  createLocalBlobAccountUrl,
  createLocalContainerBlobAccountUrl,
  localBlobAccountKey,
  localBlobContainerName,
  localWebPort,
} from "./dev-config";
import { readOptionalTrimmedString } from "./runtime";

const blobAccountKeyEnvName = "ADE_BLOB_ACCOUNT_KEY";
const blobAccountUrlEnvName = "ADE_BLOB_ACCOUNT_URL";
const blobContainerEnvName = "ADE_BLOB_CONTAINER";
const blobCorsAllowedOriginsEnvName = "ADE_BLOB_CORS_ALLOWED_ORIGINS";
const blobPublicAccountUrlEnvName = "ADE_BLOB_PUBLIC_ACCOUNT_URL";

function configuredBlobEnv(
  env: NodeJS.ProcessEnv,
): Record<string, string> | undefined {
  const accountKey = readOptionalTrimmedString(env, blobAccountKeyEnvName);
  const accountUrl = readOptionalTrimmedString(env, blobAccountUrlEnvName);
  const container = readOptionalTrimmedString(env, blobContainerEnvName);
  const corsAllowedOrigins = readOptionalTrimmedString(
    env,
    blobCorsAllowedOriginsEnvName,
  );
  const publicAccountUrl = readOptionalTrimmedString(
    env,
    blobPublicAccountUrlEnvName,
  );

  if (
    accountKey === undefined &&
    accountUrl === undefined &&
    container === undefined &&
    corsAllowedOrigins === undefined &&
    publicAccountUrl === undefined
  ) {
    return undefined;
  }

  if (accountUrl === undefined || container === undefined) {
    throw new Error(
      "Configure ADE_BLOB_ACCOUNT_URL and ADE_BLOB_CONTAINER together.",
    );
  }

  return {
    ...(accountKey
      ? {
          [blobAccountKeyEnvName]: accountKey,
        }
      : {}),
    [blobAccountUrlEnvName]: accountUrl,
    [blobContainerEnvName]: container,
    ...(publicAccountUrl
      ? {
          [blobPublicAccountUrlEnvName]: publicAccountUrl,
        }
      : {}),
    ...(corsAllowedOrigins
      ? {
          [blobCorsAllowedOriginsEnvName]: corsAllowedOrigins,
        }
      : {}),
  };
}

function localBlobCorsAllowedOrigins(port: number): string {
  return [
    `http://127.0.0.1:${String(port)}`,
    `http://localhost:${String(port)}`,
  ].join(",");
}

function createManagedLocalBlobValues(options: {
  accountUrl: string;
  corsPort: number;
  publicAccountUrl: string;
}): Record<string, string> {
  return {
    [blobAccountKeyEnvName]: localBlobAccountKey,
    [blobAccountUrlEnvName]: options.accountUrl,
    [blobContainerEnvName]: localBlobContainerName,
    [blobCorsAllowedOriginsEnvName]: localBlobCorsAllowedOrigins(
      options.corsPort,
    ),
    [blobPublicAccountUrlEnvName]: options.publicAccountUrl,
  };
}

function createHostBlobEnv(env: NodeJS.ProcessEnv = process.env): {
  usesManagedLocalBlobStorage: boolean;
  values: Record<string, string>;
} {
  const configured = configuredBlobEnv(env);

  if (configured !== undefined) {
    return {
      usesManagedLocalBlobStorage: false,
      values: configured,
    };
  }

  return {
    usesManagedLocalBlobStorage: true,
    values: createManagedLocalBlobValues({
      accountUrl: createLocalBlobAccountUrl(),
      corsPort: localWebPort,
      publicAccountUrl: createLocalBlobAccountUrl(),
    }),
  };
}

function createContainerBlobEnv(
  hostPort: number,
  env: NodeJS.ProcessEnv = process.env,
): {
  usesManagedLocalBlobStorage: boolean;
  values: Record<string, string>;
} {
  const configured = configuredBlobEnv(env);

  if (configured !== undefined) {
    return {
      usesManagedLocalBlobStorage: false,
      values: configured,
    };
  }

  return {
    usesManagedLocalBlobStorage: true,
    values: createManagedLocalBlobValues({
      accountUrl: createLocalContainerBlobAccountUrl(),
      corsPort: hostPort,
      publicAccountUrl: createLocalBlobAccountUrl(),
    }),
  };
}

export {
  createContainerBlobEnv,
  createHostBlobEnv,
  localBlobCorsAllowedOrigins,
};
