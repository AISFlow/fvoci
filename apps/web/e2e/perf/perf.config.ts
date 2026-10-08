import { defineConfig, devices } from "@playwright/test";

// Opt-in performance baseline. `*.perf.ts` is outside the default e2e
// testMatch and the CI shard planner, so the required suite never runs it.
export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: ".",
  testMatch: /.*\.perf\.ts$/,
  workers: 1,
  retries: 0,
  reporter: "list",
  timeout: 1_800_000,
  outputDir: process.env.FVOCI_PERF_RUN_DIR
    ? `${process.env.FVOCI_PERF_RUN_DIR}/playwright-output`
    : "test-results-perf",
  use: {
    ...devices["Desktop Chrome"],
    // New headless Chromium (full browser build) instead of the headless shell,
    // so compositor frames and presentation timestamps match a real browser.
    channel: "chromium",
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173",
    trace: "off",
    video: "off",
    screenshot: "off",
  },
});
