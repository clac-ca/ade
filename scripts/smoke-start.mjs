import process from "node:process";
import { main } from "./test-smoke.mjs";

void main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
