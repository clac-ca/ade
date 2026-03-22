import { fileURLToPath } from "node:url";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { runDeploySmoke } from "./http-checks.mjs";
import {
  findAvailablePort,
  runCommand,
  spawnCommand,
  writeGitHubOutput,
} from "./shared.mjs";

const dockerCommand = process.platform === "win32" ? "docker.exe" : "docker";

async function waitForExit(child, timeoutMs = 15_000) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  await Promise.race([
    new Promise((resolve) => {
      child.once("exit", () => resolve(undefined));
    }),
    delay(timeoutMs),
  ]);
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  child.kill("SIGINT");
  await waitForExit(child);

  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await waitForExit(child, 1_000);
  }
}

async function composeDown(rootDir, projectName) {
  try {
    await runCommand(
      dockerCommand,
      ["compose", "--project-name", projectName, "down", "--remove-orphans"],
      {
        cwd: rootDir,
        stdio: "ignore",
      },
    );
  } catch {
    // Ignore cleanup failures so smoke failures preserve their original cause.
  }
}

async function waitForStartupLine(child, expectedLine, timeoutMs = 120_000) {
  const output = [];

  function mirror(stream, sink) {
    stream.setEncoding("utf8");
    stream.on("data", (chunk) => {
      output.push(chunk);
      sink.write(chunk);
    });
  }

  if (!child.stdout || !child.stderr) {
    throw new Error("Smoke runner requires piped child output.");
  }

  mirror(child.stdout, process.stdout);
  mirror(child.stderr, process.stderr);

  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(
        new Error(`Timed out waiting for startup output: ${expectedLine}`),
      );
    }, timeoutMs);

    function cleanup() {
      clearTimeout(timeout);
      child.stdout?.off("data", onData);
      child.stderr?.off("data", onData);
      child.off("exit", onExit);
      child.off("error", onError);
    }

    function hasExpectedLine() {
      return output.join("").includes(expectedLine);
    }

    function onData() {
      if (!hasExpectedLine()) {
        return;
      }

      cleanup();
      resolve(undefined);
    }

    function onExit(code, signal) {
      cleanup();
      reject(
        new Error(
          `Start command exited before it reported readiness (code: ${code ?? "unknown"}, signal: ${signal ?? "none"}).`,
        ),
      );
    }

    function onError(error) {
      cleanup();
      reject(error);
    }

    child.stdout.on("data", onData);
    child.stderr.on("data", onData);
    child.on("exit", onExit);
    child.on("error", onError);

    if (hasExpectedLine()) {
      cleanup();
      resolve(undefined);
    }
  });
}

async function runAgainstLocalRuntime() {
  const rootDir = fileURLToPath(new URL("..", import.meta.url));
  const startScript = fileURLToPath(new URL("./start.mjs", import.meta.url));
  const port = await findAvailablePort();
  const appUrl = `http://localhost:${port}`;
  const projectName = `ade-local-${port}`;
  const child = spawnCommand(
    process.execPath,
    [startScript, "--port", String(port), "--no-open"],
    {
      cwd: rootDir,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  try {
    await waitForStartupLine(child, `ADE is running at ${appUrl}`);
    await runDeploySmoke(appUrl);
  } finally {
    await composeDown(rootDir, projectName);
    await stopChild(child);
  }
}

async function main() {
  const startedAt = Date.now();
  const baseUrl = process.env.ADE_BASE_URL?.trim();

  if (baseUrl) {
    await runDeploySmoke(baseUrl);
  } else {
    await runAgainstLocalRuntime();
  }

  writeGitHubOutput({
    duration_seconds: Math.round((Date.now() - startedAt) / 1000),
  });
}

export { main };

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  void main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
