import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCommand } from "./shell";

const cargoCommand = process.platform === "win32" ? "cargo.exe" : "cargo";
const binarySuffix = process.platform === "win32" ? ".exe" : "";
const rootDir = fileURLToPath(new URL("../..", import.meta.url));
const debugBinaryDir = join(rootDir, "target", "debug");

async function buildAdeApiBinaries(): Promise<void> {
  await runCommand(
    cargoCommand,
    [
      "build",
      "--locked",
      "--manifest-path",
      "apps/ade-api/Cargo.toml",
      "--bin",
      "ade-api",
      "--bin",
      "ade-migrate",
    ],
    {
      cwd: rootDir,
    },
  );
}

function adeApiBinaryPath(name: "ade-api" | "ade-migrate"): string {
  return join(debugBinaryDir, `${name}${binarySuffix}`);
}

export { adeApiBinaryPath, buildAdeApiBinaries, rootDir };
