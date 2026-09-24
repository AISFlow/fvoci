import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

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
