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
  return JSON.parse(stdout);
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
  const registryServer = process.env.ADE_REGISTRY_SERVER?.trim() ?? "";
  const registryUsername = process.env.ADE_REGISTRY_USERNAME?.trim() ?? "";
  const registryPassword = process.env.ADE_REGISTRY_PASSWORD?.trim() ?? "";
  const hasRegistryServer = registryServer !== "";
  const hasRegistryUsername = registryUsername !== "";
  const hasRegistryPassword = registryPassword !== "";
  const usesRegistryCredentials = hasRegistryUsername && hasRegistryPassword;

  if (!hasRegistryServer && (hasRegistryUsername || hasRegistryPassword)) {
    throw new Error(
      "ADE_REGISTRY_SERVER is required when ADE_REGISTRY_USERNAME or ADE_REGISTRY_PASSWORD is set.",
    );
  }

  if (hasRegistryServer && hasRegistryUsername !== hasRegistryPassword) {
    throw new Error(
      "ADE_REGISTRY_USERNAME and ADE_REGISTRY_PASSWORD must either both be set or both be empty.",
    );
  }

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

  const webUrl = outputs.webUrl?.value;
  const webAppName = outputs.webAppName?.value;

  if (typeof webUrl !== "string" || webUrl.trim() === "") {
    throw new Error("Azure deployment did not return webUrl.");
  }

  if (typeof webAppName !== "string") {
    throw new Error("Azure deployment did not return the container app name.");
  }

  const webRevision = await runAzureTextCommand([
    "containerapp",
    "show",
    "--resource-group",
    resourceGroup,
    "--name",
    webAppName,
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
    registryConfigured: hasRegistryServer,
    registryUsesCredentials: usesRegistryCredentials,
    resourceGroup,
    webAppName,
    webRevision,
    webUrl,
  };

  writeJsonFile(manifestPath, manifest);
  writeGitHubOutput({
    deployment_duration_seconds: manifest.deploymentDurationSeconds,
    deployment_manifest: manifestPath,
    web_revision: webRevision,
    web_url: webUrl,
  });
}

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
