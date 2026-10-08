import { defineConfig, devices } from "@playwright/test";

// Presigned attachment transfer (#149 B) against a real MinIO storage origin.
// Run through scripts/run-web-e2e-s3.sh, never by the normal e2e shards
// (they have no MinIO).
const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: ".",
  testMatch: /.*\.spec\.ts/,
  workers: 1,
  retries: 0,
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    trace: "retain-on-failure",
  },
});
