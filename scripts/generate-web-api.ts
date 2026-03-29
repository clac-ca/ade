import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runMain } from "./lib/runtime";
import { runCommand, runCommandCapture } from "./lib/shell";

const cargoCommand = process.platform === "win32" ? "cargo.exe" : "cargo";
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const apiDir = fileURLToPath(new URL("../apps/ade-api", import.meta.url));
const webDir = fileURLToPath(new URL("../apps/ade-web", import.meta.url));
const schemaOutputPath = fileURLToPath(
  new URL("../apps/ade-web/src/api/schema.d.ts", import.meta.url),
);

async function main(): Promise<void> {
  const checkMode = process.argv.includes("--check");
  const { stdout } = await runCommandCapture(
    cargoCommand,
    ["run", "--locked", "--quiet", "--bin", "ade-openapi"],
    {
      cwd: apiDir,
      env: {
        CARGO_TERM_COLOR: "never",
      },
    },
  );
  const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "ade-openapi-"));
  const tempOpenApiPath = path.join(tempDir, "openapi.json");

  try {
    await fs.writeFile(tempOpenApiPath, stdout, "utf8");
    await runCommand(
      pnpmCommand,
      [
        "exec",
        "openapi-typescript",
        tempOpenApiPath,
        "--output",
        schemaOutputPath,
        ...(checkMode ? ["--check"] : []),
      ],
      {
        cwd: webDir,
      },
    );
  } finally {
    await fs.rm(tempDir, {
      force: true,
      recursive: true,
    });
  }
}

void runMain(main);
