import test from "node:test";
import assert from "node:assert/strict";
import { createContainerBlobEnv, createHostBlobEnv } from "../lib/blob-env";
import { createContainerSessionPoolEnv } from "../lib/session-pool-env";
import {
  createLocalBlobAccountUrl,
  createLocalContainerBlobAccountUrl,
  createLocalContainerSessionPoolManagementEndpoint,
  createLocalSessionPoolManagementEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localBlobAccountKey,
  localBlobAccountName,
  localBlobContainerName,
  localBlobPort,
  localContainerAppUrl,
  localComposeProjectName,
  localSessionPoolPort,
  localSessionPoolSecret,
  localSqlPassword,
  localSqlPort,
  localWebHost,
  localWebPort,
} from "../lib/dev-config";
import { readOptionalTrimmedString } from "../lib/runtime";

test("local development defaults are fixed and predictable", () => {
  assert.equal(localApiHost, "127.0.0.1");
  assert.equal(localApiPort, 8000);
  assert.equal(
    localBlobAccountKey,
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==",
  );
  assert.equal(localBlobAccountName, "devstoreaccount1");
  assert.equal(localBlobContainerName, "documents");
  assert.equal(localBlobPort, 10000);
  assert.equal(localComposeProjectName, "ade-local");
  assert.equal(localContainerAppUrl, "http://host.docker.internal:8000");
  assert.equal(localSessionPoolPort, 8014);
  assert.equal(localSessionPoolSecret, "ade-local-session-secret");
  assert.equal(localSqlPort, 8013);
  assert.equal(localSqlPassword, "AdeLocal1!adeclean");
  assert.equal(localWebHost, "0.0.0.0");
  assert.equal(localWebPort, 5173);
  assert.equal(
    createLocalSqlConnectionString(),
    "Server=127.0.0.1,8013;Database=ade;User Id=sa;Password=AdeLocal1!adeclean;Encrypt=false;TrustServerCertificate=true",
  );
  assert.equal(
    createLocalBlobAccountUrl(),
    "http://127.0.0.1:10000/devstoreaccount1",
  );
  assert.equal(
    createLocalContainerBlobAccountUrl(),
    "http://host.docker.internal:10000/devstoreaccount1",
  );
  assert.equal(
    createLocalSessionPoolManagementEndpoint(),
    "http://127.0.0.1:8014",
  );
  assert.equal(
    createLocalContainerSessionPoolManagementEndpoint(),
    "http://host.docker.internal:8014",
  );
});

test("readOptionalTrimmedString ignores missing and blank values", () => {
  assert.equal(readOptionalTrimmedString({}, "MISSING"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: "   " }, "VALUE"), undefined);
  assert.equal(readOptionalTrimmedString({ VALUE: " ok " }, "VALUE"), "ok");
});

test("managed local blob env keeps browser and runtime endpoints explicit", () => {
  assert.deepEqual(createHostBlobEnv(), {
    usesManagedLocalBlobStorage: true,
    values: {
      ADE_BLOB_ACCOUNT_KEY:
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==",
      ADE_BLOB_ACCOUNT_URL: "http://127.0.0.1:10000/devstoreaccount1",
      ADE_BLOB_CONTAINER: "documents",
      ADE_BLOB_CORS_ALLOWED_ORIGINS:
        "http://127.0.0.1:5173,http://localhost:5173",
      ADE_BLOB_PUBLIC_ACCOUNT_URL: "http://127.0.0.1:10000/devstoreaccount1",
    },
  });

  assert.deepEqual(createContainerBlobEnv(4100), {
    usesManagedLocalBlobStorage: true,
    values: {
      ADE_BLOB_ACCOUNT_KEY:
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==",
      ADE_BLOB_ACCOUNT_URL:
        "http://host.docker.internal:10000/devstoreaccount1",
      ADE_BLOB_CONTAINER: "documents",
      ADE_BLOB_CORS_ALLOWED_ORIGINS:
        "http://127.0.0.1:4100,http://localhost:4100",
      ADE_BLOB_PUBLIC_ACCOUNT_URL: "http://127.0.0.1:10000/devstoreaccount1",
    },
  });
});

test("configured session pool env keeps the app url fallback boring and local", () => {
  assert.deepEqual(
    createContainerSessionPoolEnv(
      {
        ADE_SESSION_POOL_MANAGEMENT_ENDPOINT:
          "https://example.dynamicsessions.io",
        ADE_SANDBOX_ENVIRONMENT_SECRET: "secret",
      },
      {},
    ),
    {
      usesManagedLocalSessionPool: false,
      values: {
        ADE_PUBLIC_API_URL: "http://host.docker.internal:8000",
        ADE_SESSION_POOL_MANAGEMENT_ENDPOINT:
          "https://example.dynamicsessions.io",
        ADE_SANDBOX_ENVIRONMENT_SECRET: "secret",
      },
    },
  );
});
