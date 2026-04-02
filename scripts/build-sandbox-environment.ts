import { buildSandboxEnvironmentAssets } from "../apps/ade-api/sandbox-environment/build";
import { runMain } from "./lib/runtime";

export { buildSandboxEnvironmentAssets };

void runMain(async () => {
  buildSandboxEnvironmentAssets();
});
