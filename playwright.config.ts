import { defineConfig } from "@playwright/test";

const baseURL = process.env["PLAYWRIGHT_BASE_URL"]?.trim().replace(/\/+$/, "");

if (!baseURL) {
  throw new Error(
    "PLAYWRIGHT_BASE_URL is required. Use `pnpm test:acceptance --url <base-url>` or `pnpm test:acceptance --image <image>`.",
  );
}

export default defineConfig({
  outputDir: "test-results/playwright",
  reporter: process.env["CI"]
    ? [["github"], ["html", { open: "never" }]]
    : [["list"]],
  retries: process.env["CI"] ? 1 : 0,
  testDir: "./tests/acceptance",
  timeout: 90_000,
  use: {
    baseURL,
    screenshot: "only-on-failure",
    trace: "on-first-retry",
  },
  workers: 1,
});
