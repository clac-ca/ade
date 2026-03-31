import { createHmac } from "node:crypto";
import { fileURLToPath } from "node:url";
import process from "node:process";
import {
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
} from "./lib/dev-config";
import { createHostBlobEnv } from "./lib/blob-env";
import { createHostSessionPoolEnv } from "./lib/session-pool-env";
import { createConsoleLogger, formatError, runMain } from "./lib/runtime";
import { runCommand, spawnCommand, waitForReady } from "./lib/shell";
import { downLocalDependencies, upLocalDependencies } from "./local-deps";

const cargoCommand = process.platform === "win32" ? "cargo.exe" : "cargo";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";
const storageServiceVersion = "2024-11-04";

type UploadResponse = {
  filePath: string;
  upload: {
    expiresAt: string;
    headers: Record<string, string>;
    method: string;
    url: string;
  };
};

type RunCreatedResponse = {
  inputPath: string;
  outputPath: string | null;
  runId: string;
  status: string;
};

type RunDetailResponse = {
  errorMessage?: string | null;
  inputPath: string;
  outputPath?: string | null;
  phase?: string | null;
  runId: string;
  status: string;
  validationIssues: unknown[];
};

function apiEnv(): Record<string, string> {
  const { values: blobEnv } = createHostBlobEnv();
  return {
    ...blobEnv,
    ...createHostSessionPoolEnv({
      appUrl: `http://host.docker.internal:${String(localApiPort)}`,
    }),
    [sqlConnectionStringName]: createLocalSqlConnectionString(),
  };
}

function hostBlobConfig() {
  const { values } = createHostBlobEnv();
  const accountKey = values["ADE_BLOB_ACCOUNT_KEY"];
  const container = values["ADE_BLOB_CONTAINER"];
  const configuredAccountUrl =
    values["ADE_BLOB_PUBLIC_ACCOUNT_URL"] ?? values["ADE_BLOB_ACCOUNT_URL"];

  if (
    accountKey === undefined ||
    container === undefined ||
    configuredAccountUrl === undefined
  ) {
    throw new Error("Managed local Blob config was incomplete.");
  }

  const accountUrl = new URL(configuredAccountUrl);
  const accountName =
    accountUrl.hostname === "127.0.0.1" || accountUrl.hostname === "localhost"
      ? accountUrl.pathname.split("/").filter(Boolean)[0]
      : accountUrl.hostname.split(".")[0];
  if (accountName === undefined) {
    throw new Error("Local Blob account URL did not include an account name.");
  }

  return {
    accountKey,
    accountName,
    accountUrl,
    container,
  };
}

function scopeBasePath(workspaceId: string, configVersionId: string): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}`;
}

function apiUrl(path: string): string {
  return `http://${localApiHost}:${String(localApiPort)}${path}`;
}

async function jsonFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(apiUrl(path), init);
  const body = await response.text();

  if (!response.ok) {
    throw new Error(
      `${String(response.status)} ${response.statusText}: ${body}`,
    );
  }

  return JSON.parse(body) as T;
}

async function directUpload(
  baseUrl: string,
  upload: UploadResponse["upload"],
  content: Uint8Array,
): Promise<void> {
  const headers = new Headers();
  for (const [name, value] of Object.entries(upload.headers)) {
    headers.set(name, value);
  }

  const target = new URL(upload.url, baseUrl).toString();
  const response = await fetch(target, {
    body: Buffer.from(content),
    headers,
    method: upload.method,
  });
  const body = await response.text();

  if (!response.ok) {
    throw new Error(
      `Direct upload failed with ${String(response.status)} ${response.statusText}: ${body}`,
    );
  }
}

async function createUpload(
  workspaceId: string,
  configVersionId: string,
  filename: string,
  contentType: string,
  content: Uint8Array,
): Promise<UploadResponse> {
  const response = await jsonFetch<UploadResponse>(
    `${scopeBasePath(workspaceId, configVersionId)}/uploads`,
    {
      body: JSON.stringify({
        contentType,
        filename,
      }),
      headers: {
        "content-type": "application/json",
      },
      method: "POST",
    },
  );
  await directUpload(apiUrl("/"), response.upload, content);
  return response;
}

async function createRun(
  workspaceId: string,
  configVersionId: string,
  inputPath: string,
): Promise<RunCreatedResponse> {
  const response = await fetch(
    apiUrl(`${scopeBasePath(workspaceId, configVersionId)}/runs`),
    {
      body: JSON.stringify({
        inputPath,
      }),
      headers: {
        "content-type": "application/json",
      },
      method: "POST",
    },
  );
  const body = await response.text();
  if (response.status !== 202) {
    throw new Error(
      `Run creation failed with ${String(response.status)} ${response.statusText}: ${body}`,
    );
  }

  return JSON.parse(body) as RunCreatedResponse;
}

async function waitForRun(
  workspaceId: string,
  configVersionId: string,
  runId: string,
): Promise<RunDetailResponse> {
  const path = `${scopeBasePath(workspaceId, configVersionId)}/runs/${encodeURIComponent(runId)}`;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const detail = await jsonFetch<RunDetailResponse>(path);
    if (detail.status === "succeeded") {
      return detail;
    }
    if (detail.status === "failed" || detail.status === "cancelled") {
      throw new Error(
        `Run ${runId} finished with ${detail.status}: ${detail.errorMessage ?? "no error message"}`,
      );
    }

    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`Run ${runId} did not finish before the smoke timeout.`);
}

async function fetchRunEvents(
  workspaceId: string,
  configVersionId: string,
  runId: string,
  after?: number,
): Promise<string> {
  const url = new URL(
    apiUrl(
      `${scopeBasePath(workspaceId, configVersionId)}/runs/${encodeURIComponent(runId)}/events`,
    ),
  );
  if (after !== undefined) {
    url.searchParams.set("after", String(after));
  }

  const response = await fetch(url, {
    headers: {
      accept: "text/event-stream",
    },
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `Run events fetch failed with ${String(response.status)} ${response.statusText}: ${body}`,
    );
  }
  return body;
}

function blobIso8601(value: Date): string {
  return value.toISOString().replace(/\.\d{3}Z$/, "Z");
}

function blobReadUrl(path: string): string {
  const { accountKey, accountName, accountUrl, container } = hostBlobConfig();
  const canonicalizedResource = `/blob/${accountName}/${container}/${path}`;
  const start = blobIso8601(new Date(Date.now() - 5 * 60_000));
  const expiry = blobIso8601(new Date(Date.now() + 60 * 60_000));
  const stringToSign = [
    "r",
    start,
    expiry,
    canonicalizedResource,
    "",
    "",
    "https,http",
    storageServiceVersion,
    "b",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
  ].join("\n");
  const signature = createHmac("sha256", Buffer.from(accountKey, "base64"))
    .update(stringToSign, "utf8")
    .digest("base64");
  const url = new URL(accountUrl.toString());
  url.pathname = `${url.pathname.replace(/\/$/, "")}/${container}/${path}`;
  url.searchParams.set("sv", storageServiceVersion);
  url.searchParams.set("sp", "r");
  url.searchParams.set("sr", "b");
  url.searchParams.set("st", start);
  url.searchParams.set("se", expiry);
  url.searchParams.set("spr", "https,http");
  url.searchParams.set("sig", signature);
  return url.toString();
}

async function fetchBlobBytes(path: string): Promise<Uint8Array> {
  const response = await fetch(blobReadUrl(path));
  if (!response.ok) {
    throw new Error(
      `Blob read failed with ${String(response.status)} ${response.statusText}: ${await response.text()}`,
    );
  }

  return new Uint8Array(await response.arrayBuffer());
}

function parseSseIds(body: string): number[] {
  return [...body.matchAll(/^id:\s*(\d+)$/gm)].map((match) =>
    Number.parseInt(match[1] ?? "0", 10),
  );
}

async function main(logger = createConsoleLogger()): Promise<void> {
  await runCommand(pnpmCommand, ["package:python"], {
    cwd: rootDir,
  });
  const env = apiEnv();
  let apiProcess: ReturnType<typeof spawnCommand> | undefined;

  try {
    await upLocalDependencies();
    await runCommand(
      cargoCommand,
      [
        "run",
        "--locked",
        "--manifest-path",
        "apps/ade-api/Cargo.toml",
        "--bin",
        "ade-migrate",
      ],
      {
        cwd: rootDir,
        env,
      },
    );

    const api = spawnCommand(
      cargoCommand,
      [
        "run",
        "--locked",
        "--manifest-path",
        "apps/ade-api/Cargo.toml",
        "--bin",
        "ade-api",
        "--",
        "--host",
        localApiHost,
        "--port",
        String(localApiPort),
      ],
      {
        cwd: rootDir,
        env,
      },
    );
    apiProcess = api;

    await waitForReady(
      [`http://${localApiHost}:${String(localApiPort)}/api/readyz`],
      {
        isAlive: () => api.exitCode === null && api.signalCode === null,
        timeoutMs: 60_000,
      },
    );

    const firstUpload = await createUpload(
      "workspace-a",
      "config-v1",
      "input-a.csv",
      "text/csv",
      new TextEncoder().encode("name,email\nalice,alice@example.com\n"),
    );
    if (
      !firstUpload.filePath.startsWith(
        "workspaces/workspace-a/configs/config-v1/uploads/upl_",
      )
    ) {
      throw new Error(
        "First upload path was not scoped under workspace-a/config-v1.",
      );
    }

    const firstRun = await createRun(
      "workspace-a",
      "config-v1",
      firstUpload.filePath,
    );
    if (firstRun.status !== "pending") {
      throw new Error("First run was not accepted in the pending state.");
    }

    const firstDetail = await waitForRun(
      "workspace-a",
      "config-v1",
      firstRun.runId,
    );
    if (
      firstDetail.outputPath === null ||
      firstDetail.outputPath === undefined ||
      !firstDetail.outputPath.endsWith("/normalized.xlsx")
    ) {
      throw new Error("First run did not persist the expected output path.");
    }
    if (firstDetail.validationIssues.length !== 0) {
      throw new Error("First run returned unexpected validation issues.");
    }

    const firstEvents = await fetchRunEvents(
      "workspace-a",
      "config-v1",
      firstRun.runId,
    );
    if (
      !firstEvents.includes("event: run.created") ||
      !firstEvents.includes("event: run.result") ||
      !firstEvents.includes("event: run.completed")
    ) {
      throw new Error("First run SSE replay was missing expected events.");
    }

    const resumedEvents = await fetchRunEvents(
      "workspace-a",
      "config-v1",
      firstRun.runId,
      2,
    );
    if (parseSseIds(resumedEvents).some((id) => id <= 2)) {
      throw new Error(
        "Run SSE resume replay did not honor the requested sequence.",
      );
    }

    const firstOutputBytes = await fetchBlobBytes(firstDetail.outputPath);
    if (firstOutputBytes.byteLength === 0) {
      throw new Error("First run output artifact was empty.");
    }

    const secondUpload = await createUpload(
      "workspace-b",
      "config-v2",
      "input-b.csv",
      "text/csv",
      new TextEncoder().encode("name,email\nbob,bob@example.com\n"),
    );
    if (
      !secondUpload.filePath.startsWith(
        "workspaces/workspace-b/configs/config-v2/uploads/upl_",
      )
    ) {
      throw new Error(
        "Second upload path was not scoped under workspace-b/config-v2.",
      );
    }

    const secondRun = await createRun(
      "workspace-b",
      "config-v2",
      secondUpload.filePath,
    );
    const secondDetail = await waitForRun(
      "workspace-b",
      "config-v2",
      secondRun.runId,
    );
    if (
      firstDetail.outputPath === secondDetail.outputPath ||
      !secondDetail.outputPath?.startsWith(
        "workspaces/workspace-b/configs/config-v2/runs/",
      )
    ) {
      throw new Error(
        "Scoped run outputs were not isolated per workspace/config.",
      );
    }

    logger.info("Local session smoke passed.");
  } catch (error) {
    throw new Error(formatError(error), { cause: error });
  } finally {
    if (apiProcess !== undefined && apiProcess.exitCode === null) {
      apiProcess.kill("SIGTERM");
    }
    await downLocalDependencies().catch(() => undefined);
  }
}

void runMain(async () => {
  await main();
});
