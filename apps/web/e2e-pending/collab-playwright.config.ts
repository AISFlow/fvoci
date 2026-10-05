import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

// The same Bun-in-CI guard as ../playwright.config.ts.
if (process.env.CI && !process.versions.bun) {
  throw new Error("Playwright must run under Bun in CI (bun --bun x playwright)");
}

// A selected normal-server run has an allocated migrate --start lifecycle.
// The pending runner instead starts its test servers per spec.
const selectedBackend = process.env.FVOCI_E2E_SELECTED_BACKEND;
if (selectedBackend !== undefined) {
  if (!/^(postgres|sqlite)$/.test(selectedBackend)) {
    throw new Error("selected normal-server backend must be postgres or sqlite");
  }
  if (process.env.FVOCI_E2E_PENDING === "1") {
    throw new Error("selected normal-server spec requires its allocated startup driver");
  }
}

export default defineConfig({
  testDir: ".",
  testMatch:
    selectedBackend === undefined ? /.*\.spec\.ts/ : /workspace-wiki-selected-backend\.spec\.ts/,
  testIgnore: selectedBackend === undefined ? /workspace-wiki-selected-backend\.spec\.ts/ : [],
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
