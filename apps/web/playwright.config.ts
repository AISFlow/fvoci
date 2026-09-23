import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

export default defineConfig({
  testDir: "./e2e",
  workers: 1,
  retries: 0,
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    trace: "retain-on-failure",
  },
});
