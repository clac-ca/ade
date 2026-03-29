import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
} from "./lib/dev-config";
import { createHostSessionPoolEnv } from "./lib/session-pool-env";
import { createConsoleLogger, formatError, runMain } from "./lib/runtime";
import { runCommand, spawnCommand, waitForReady } from "./lib/shell";
import { downLocalDependencies, upLocalDependencies } from "./local-deps";

const cargoCommand = process.platform === "win32" ? "cargo.exe" : "cargo";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

type SessionFile = {
  filename: string;
  size: number;
};

type CommandExecutionResponse = {
  durationMs: number;
  exitCode: number;
  stderr: string;
  stdout: string;
};

type RunResponse = {
  outputPath: string;
  validationIssues: unknown[];
};

function apiEnv(): Record<string, string> {
  return {
    ...createHostSessionPoolEnv(),
    [sqlConnectionStringName]: createLocalSqlConnectionString(),
  };
}

function sessionBasePath(workspaceId: string, configVersionId: string): string {
  return `/api/workspaces/${encodeURIComponent(workspaceId)}/configs/${encodeURIComponent(configVersionId)}`;
}

function encodeSessionFilePath(path: string): string {
  return path
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

async function jsonFetch(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(
    `http://${localApiHost}:${String(localApiPort)}${path}`,
    init,
  );
  const body = await response.text();

  if (!response.ok) {
    throw new Error(
      `${String(response.status)} ${response.statusText}: ${body}`,
    );
  }

  return body === "" ? null : JSON.parse(body);
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

    const firstScope = sessionBasePath("workspace-a", "config-v1");
    const secondScope = sessionBasePath("workspace-b", "config-v2");

    const uploadForm = new FormData();
    uploadForm.append(
      "file",
      new Blob(["hello from scope a"], { type: "text/plain" }),
      "notes.txt",
    );
    const uploadedNotes = (await jsonFetch(`${firstScope}/files`, {
      body: uploadForm,
      method: "POST",
    })) as SessionFile;
    if (uploadedNotes.filename !== "uploads/notes.txt") {
      throw new Error("Session file upload did not return the expected path.");
    }

    const firstFiles = (await jsonFetch(
      `${firstScope}/files`,
    )) as SessionFile[];
    const firstFilenames = firstFiles.map((entry) => entry.filename);
    if (!firstFilenames.includes(uploadedNotes.filename)) {
      throw new Error("Session file upload did not appear in the first scope.");
    }

    const secondFiles = (await jsonFetch(
      `${secondScope}/files`,
    )) as SessionFile[];
    if (
      secondFiles.some((entry) => entry.filename === uploadedNotes.filename)
    ) {
      throw new Error("Session files leaked across workspace/config scopes.");
    }

    const commandExecution = (await jsonFetch(`${firstScope}/executions`, {
      body: JSON.stringify({
        shellCommand: "pwd",
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as CommandExecutionResponse;
    const commandStdout = commandExecution.stdout;

    if (commandExecution.exitCode !== 0 || commandStdout.trim() === "") {
      throw new Error(
        `Command execution failed: ${commandExecution.stderr || commandExecution.stdout || "unknown error"}`,
      );
    }

    const sessionFileResponse = await fetch(
      `http://${localApiHost}:${String(localApiPort)}${firstScope}/files/${encodeSessionFilePath(uploadedNotes.filename)}/content`,
    );
    const sessionFileText = await sessionFileResponse.text();
    if (!sessionFileResponse.ok || sessionFileText !== "hello from scope a") {
      throw new Error(
        "Session file download did not return the uploaded content.",
      );
    }

    const runUploadForm = new FormData();
    runUploadForm.append(
      "file",
      new Blob(["name,email\nalice,alice@example.com\n"], { type: "text/csv" }),
      "input.csv",
    );
    const uploadedRunInput = (await jsonFetch(`${firstScope}/files`, {
      body: runUploadForm,
      method: "POST",
    })) as SessionFile;
    const firstRun = (await jsonFetch(`${firstScope}/runs`, {
      body: JSON.stringify({
        inputPath: uploadedRunInput.filename,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as RunResponse;

    if (
      !firstRun.outputPath.endsWith(".xlsx") ||
      firstRun.validationIssues.length !== 0
    ) {
      throw new Error("ADE run did not return the expected result metadata.");
    }

    const firstWorkbook = await fetch(
      `http://${localApiHost}:${String(localApiPort)}${firstScope}/files/${encodeSessionFilePath(firstRun.outputPath)}/content`,
    );
    const firstWorkbookBytes = await firstWorkbook.arrayBuffer();
    if (!firstWorkbook.ok || firstWorkbookBytes.byteLength === 0) {
      throw new Error(
        "First run workbook download did not return any content.",
      );
    }

    const secondRunUploadForm = new FormData();
    secondRunUploadForm.append(
      "file",
      new Blob(["name,email\nbob,bob@example.com\n"], { type: "text/csv" }),
      "input.csv",
    );
    const secondUploadedRunInput = (await jsonFetch(`${secondScope}/files`, {
      body: secondRunUploadForm,
      method: "POST",
    })) as SessionFile;
    const secondRun = (await jsonFetch(`${secondScope}/runs`, {
      body: JSON.stringify({
        inputPath: secondUploadedRunInput.filename,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as RunResponse;

    if (secondRun.outputPath === firstRun.outputPath) {
      throw new Error("Parallel ADE runs reused the same output path.");
    }

    const isolatedFiles = (await jsonFetch(
      `${secondScope}/files`,
    )) as SessionFile[];
    if (
      isolatedFiles.some((entry) => entry.filename === uploadedNotes.filename)
    ) {
      throw new Error("Second scope can see files from the first scope.");
    }

    const overlappingUploadA = new FormData();
    overlappingUploadA.append(
      "file",
      new Blob(["name,email\ncarol,carol@example.com\n"], { type: "text/csv" }),
      "input-a.csv",
    );
    const overlappingUploadB = new FormData();
    overlappingUploadB.append(
      "file",
      new Blob(["name,email\ndave,dave@example.com\n"], { type: "text/csv" }),
      "input-b.csv",
    );
    const [uploadedA, uploadedB] = (await Promise.all([
      jsonFetch(`${firstScope}/files`, {
        body: overlappingUploadA,
        method: "POST",
      }),
      jsonFetch(`${firstScope}/files`, {
        body: overlappingUploadB,
        method: "POST",
      }),
    ])) as [SessionFile, SessionFile];
    const [overlapA, overlapB] = (await Promise.all([
      jsonFetch(`${firstScope}/runs`, {
        body: JSON.stringify({
          inputPath: uploadedA.filename,
        }),
        headers: { "content-type": "application/json" },
        method: "POST",
      }),
      jsonFetch(`${firstScope}/runs`, {
        body: JSON.stringify({
          inputPath: uploadedB.filename,
        }),
        headers: { "content-type": "application/json" },
        method: "POST",
      }),
    ])) as [RunResponse, RunResponse];

    if (overlapA.outputPath === overlapB.outputPath) {
      throw new Error(
        "Overlapping runs for the same scope reused the same output path.",
      );
    }

    logger.info("Local session smoke test passed.");
  } finally {
    if (
      apiProcess &&
      apiProcess.exitCode === null &&
      apiProcess.signalCode === null
    ) {
      apiProcess.kill("SIGINT");
    }

    await downLocalDependencies({
      stdio: "ignore",
    }).catch(() => undefined);
  }
}

void runMain(async () => {
  try {
    await main();
  } catch (error) {
    console.error(formatError(error));
    process.exit(1);
  }
});
