import { stageLocalConfigMounts } from "./lib/local-config-mounts";
import { createConsoleLogger, runMain } from "./lib/runtime";

function main(): void {
  stageLocalConfigMounts(createConsoleLogger());
}

void runMain(() => {
  main();
  return Promise.resolve();
});
