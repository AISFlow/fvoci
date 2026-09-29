import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

// The same Bun-in-CI guard as ../playwright.config.ts.
if (process.env.CI && !process.versions.bun) {
  throw new Error("Playwright must run under Bun in CI (bun --bun x playwright)");
}

export default defineConfig({
  testDir: ".",
  testMatch: /.*\.spec\.ts/,
  workers: 1,
  retries: 0,
  outputDir: process.env.FVOCI_E2E_RESULT_DIR
    ? `${process.env.FVOCI_E2E_RESULT_DIR}/playwright-output`
    : "test-results-collab",
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    trace: "retain-on-failure",
  },
});
