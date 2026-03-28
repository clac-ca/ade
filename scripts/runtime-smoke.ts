import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  createLocalSessionPoolManagementEndpoint,
  createLocalSessionPoolMcpEndpoint,
  createLocalSqlConnectionString,
  localApiHost,
  localApiPort,
  localSessionPoolRuntimeSecret,
} from "./lib/dev-config";
import { createConsoleLogger, formatError, runMain } from "./lib/runtime";
import { runCommand, spawnCommand, waitForReady } from "./lib/shell";
import { downLocalDependencies, upLocalDependencies } from "./local-deps";

const cargoCommand = process.platform === "win32" ? "cargo.exe" : "cargo";
const rootDir = fileURLToPath(new URL("..", import.meta.url));
const sqlConnectionStringName = "AZURE_SQL_CONNECTIONSTRING";

function apiEnv(): Record<string, string> {
  return {
    ADE_RUNTIME_SESSION_SECRET: localSessionPoolRuntimeSecret,
    ADE_SESSION_POOL_MANAGEMENT_ENDPOINT:
      createLocalSessionPoolManagementEndpoint(),
    ADE_SESSION_POOL_MCP_ENDPOINT: createLocalSessionPoolMcpEndpoint(),
    [sqlConnectionStringName]: createLocalSqlConnectionString(),
  };
}

async function jsonFetch(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(
    `http://${localApiHost}:${String(localApiPort)}${path}`,
    init,
  );
  const body = await response.text();

  if (!response.ok) {
    throw new Error(`${String(response.status)} ${response.statusText}: ${body}`);
  }

  return body === "" ? null : JSON.parse(body);
}

async function main(logger = createConsoleLogger()): Promise<void> {
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

    const uploadForm = new FormData();
    uploadForm.append(
      "file",
      new Blob(["name,email\nalice,alice@example.com\n"], { type: "text/csv" }),
      "input.csv",
    );
    await jsonFetch("/api/runtime/files/upload", {
      body: uploadForm,
      method: "POST",
    });

    const execution = (await jsonFetch("/api/runtime/code/execute", {
      body: JSON.stringify({
        properties: {
          codeInputType: "inline",
          executionType: "synchronous",
          code: [
            "from pathlib import Path",
            "from ade_engine import load_config, process",
            "",
            "config = load_config('ade_config', name='ade-config')",
            "result = process(",
            "    config=config,",
            "    input_path=Path('/mnt/data/input.csv'),",
            "    output_dir=Path('/mnt/data'),",
            ")",
            "print(result.output_path.name)",
            "print(len(result.validation_issues))",
          ].join("\n"),
        },
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as {
      properties: { exitCode: number; stdout: string; stderr?: string };
    };

    if (execution.properties.exitCode !== 0) {
      throw new Error(
        `Execution failed: ${execution.properties.stderr ?? execution.properties.stdout}`,
      );
    }

    const files = (await jsonFetch("/api/runtime/files")) as {
      value: Array<{ properties: { filename: string } }>;
    };
    const filenames = files.value.map((entry) => entry.properties.filename);
    if (
      !filenames.includes("input.csv") ||
      !filenames.includes("input.normalized.xlsx")
    ) {
      throw new Error(
        `Uploaded file missing from runtime files: ${filenames.join(", ")}`,
      );
    }

    const uploadedFile = await fetch(
      `http://${localApiHost}:${String(localApiPort)}/api/runtime/files/content/input.csv`,
    );
    const uploadedText = await uploadedFile.text();
    if (!uploadedFile.ok || !uploadedText.includes("alice@example.com")) {
      throw new Error(
        "Uploaded file download did not return the expected content.",
      );
    }

    const normalizedWorkbook = await fetch(
      `http://${localApiHost}:${String(localApiPort)}/api/runtime/files/content/input.normalized.xlsx`,
    );
    const workbookBytes = await normalizedWorkbook.arrayBuffer();
    if (!normalizedWorkbook.ok || workbookBytes.byteLength === 0) {
      throw new Error("Normalized workbook download did not return any content.");
    }

    await jsonFetch("/api/runtime/mcp", {
      body: JSON.stringify({
        id: "init",
        jsonrpc: "2.0",
        method: "initialize",
        params: {
          clientInfo: { name: "ade-smoke", version: "0.1.0" },
          protocolVersion: "2025-03-26",
        },
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    });

    await jsonFetch("/api/runtime/mcp", {
      body: JSON.stringify({
        id: "list",
        jsonrpc: "2.0",
        method: "tools/list",
        params: {},
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    });

    const mcpFirst = (await jsonFetch("/api/runtime/mcp", {
      body: JSON.stringify({
        id: "shell-1",
        jsonrpc: "2.0",
        method: "tools/call",
        params: {
          name: "runShellCommandInRemoteEnvironment",
          arguments: {
            shellCommand:
              "printf persisted > state.txt && pwd && cat state.txt",
          },
        },
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as {
      result: { structuredContent: { stdout: string } };
    };

    const mcpSecond = (await jsonFetch("/api/runtime/mcp", {
      body: JSON.stringify({
        id: "shell-2",
        jsonrpc: "2.0",
        method: "tools/call",
        params: {
          name: "runShellCommandInRemoteEnvironment",
          arguments: {
            shellCommand: "pwd && cat state.txt",
          },
        },
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as {
      result: { structuredContent: { stdout: string } };
    };

    if (
      !mcpFirst.result.structuredContent.stdout.includes("persisted") ||
      !mcpSecond.result.structuredContent.stdout.includes("persisted")
    ) {
      throw new Error(
        "MCP environment state was not reused across shell calls.",
      );
    }

    await jsonFetch("/api/runtime/.management/stopSession", {
      body: JSON.stringify({}),
      headers: { "content-type": "application/json" },
      method: "POST",
    });

    const filesAfterReset = (await jsonFetch("/api/runtime/files")) as {
      value: Array<unknown>;
    };
    if (filesAfterReset.value.length !== 0) {
      throw new Error("stopSession did not clear the job-session files.");
    }

    const mcpAfterReset = (await jsonFetch("/api/runtime/mcp", {
      body: JSON.stringify({
        id: "shell-3",
        jsonrpc: "2.0",
        method: "tools/call",
        params: {
          name: "runShellCommandInRemoteEnvironment",
          arguments: {
            shellCommand:
              "pwd && if [ -f state.txt ]; then cat state.txt; else echo cleared; fi",
          },
        },
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    })) as {
      result: { structuredContent: { stdout: string } };
    };

    if (!mcpAfterReset.result.structuredContent.stdout.includes("cleared")) {
      throw new Error(
        "stopSession did not reset the cached MCP console environment.",
      );
    }

    logger.info("Local runtime smoke test passed.");
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
