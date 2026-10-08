import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

// GitHub runners also have Node on PATH. `bun --bun` runs Playwright's
// `#!/usr/bin/env node` binary through a node -> bun shim in
// /tmp/bun-node-<revision> and silently skips the shim when that directory is
// unusable, so CI fails here rather than running on Node.
if (process.env.CI && !process.versions.bun) {
  throw new Error("Playwright must run under Bun in CI (bun --bun x playwright)");
}

export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: "./e2e",
  workers: 1,
  retries: 0,
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    trace: "retain-on-failure",
  },
});
