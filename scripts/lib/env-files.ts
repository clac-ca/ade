import { existsSync } from "node:fs";
import process from "node:process";

function loadOptionalEnvFile(path = ".env") {
  if (!existsSync(path)) {
    return;
  }

  process.loadEnvFile(path);
}

export { loadOptionalEnvFile };
