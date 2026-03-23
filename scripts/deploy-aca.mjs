import { fileURLToPath } from "node:url";
import process from "node:process";
import { resolve } from "node:path";
import {
  requireEnv,
  runCommandCapture,
  writeGitHubOutput,
  writeJsonFile,
} from "./shared.mjs";

const rootDir = fileURLToPath(new URL("..", import.meta.url));

function readArg(name) {
  const flag = `--${name}`;
  const index = process.argv.indexOf(flag);

  if (index === -1) {
    return null;
  }

  return process.argv[index + 1] ?? null;
}

function resolvePath(path) {
  return resolve(rootDir, path);
}

async function runAzureJsonCommand(args) {
  const { stdout } = await runCommandCapture(
    "az",
    [...args, "--only-show-errors", "--output", "json"],
    {
      cwd: rootDir,
    },
  );
  const trimmed = stdout.trim();
  const jsonStart = Math.min(
    ...["{", "["]
      .map((marker) => trimmed.indexOf(marker))
      .filter((index) => index !== -1),
  );

  if (!Number.isFinite(jsonStart)) {
    throw new Error(`Azure CLI did not return JSON output.\n${trimmed}`);
  }

  return JSON.parse(trimmed.slice(jsonStart));
}

async function runAzureTextCommand(args) {
  const { stdout } = await runCommandCapture(
    "az",
    [...args, "--only-show-errors", "--output", "tsv"],
    {
      cwd: rootDir,
    },
  );
  return stdout.trim();
}

async function main() {
  const environmentName = readArg("environment");

  if (!environmentName) {
    throw new Error("Missing required argument: --environment");
  }

  const resourceGroup = readArg("resource-group");

  if (!resourceGroup) {
    throw new Error("Missing required argument: --resource-group");
  }

  const parametersFile = readArg("parameters-file");

  if (!parametersFile) {
    throw new Error("Missing required argument: --parameters-file");
  }

  const deploymentName =
    readArg("deployment-name") ??
    `ade-${environmentName}-${new Date().toISOString().replaceAll(/[:.]/g, "-")}`;
  const manifestPath = resolvePath(
    readArg("manifest-path") ?? `${environmentName}-deployment-manifest.json`,
  );
  const image = requireEnv("ADE_IMAGE");

  const startedAt = Date.now();
  const outputs = await runAzureJsonCommand([
    "deployment",
    "group",
    "create",
    "--name",
    deploymentName,
    "--resource-group",
    resourceGroup,
    "--parameters",
    resolvePath(parametersFile),
    "--query",
    "properties.outputs",
  ]);

  const appUrl = outputs.appUrl?.value;
  const appName = outputs.appName?.value;

  if (typeof appUrl !== "string" || appUrl.trim() === "") {
    throw new Error("Azure deployment did not return appUrl.");
  }

  if (typeof appName !== "string") {
    throw new Error("Azure deployment did not return the container app name.");
  }

  const appRevision = await runAzureTextCommand([
    "containerapp",
    "show",
    "--resource-group",
    resourceGroup,
    "--name",
    appName,
    "--query",
    "properties.latestRevisionName",
  ]);
  const manifest = {
    deployedAt: new Date().toISOString(),
    deploymentDurationSeconds: Math.round((Date.now() - startedAt) / 1000),
    deploymentName,
    environment: environmentName,
    image,
    parametersFile: resolvePath(parametersFile),
    resourceGroup,
    appName,
    appRevision,
    appUrl,
  };

  writeJsonFile(manifestPath, manifest);
  writeGitHubOutput({
    deployment_duration_seconds: manifest.deploymentDurationSeconds,
    deployment_manifest: manifestPath,
    app_revision: appRevision,
    app_url: appUrl,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
