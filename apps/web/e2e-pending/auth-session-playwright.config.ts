import { defineConfig, devices } from "@playwright/test";
import { isAbsolute } from "node:path";

const backend = process.env.FVOCI_E2E_SELECTED_BACKEND;
const flow = process.env.FVOCI_E2E_SELECTED_FLOW;
const source = process.env.FVOCI_E2E_AUTH_SOURCE;
const tree = process.env.FVOCI_E2E_AUTH_TREE;
const baseURL = process.env.PLAYWRIGHT_BASE_URL;
const capture = process.env.FVOCI_E2E_SMTP_CAPTURE;
if (
  !backend ||
  !["postgres", "sqlite", "libsql-remote"].includes(backend) ||
  !flow ||
  !["on", "off"].includes(flow) ||
  !/^[a-f0-9]{40}$/.test(source ?? "") ||
  !/^[a-f0-9]{40}$/.test(tree ?? "") ||
  !baseURL ||
  !capture ||
  !isAbsolute(capture) ||
  process.env.FVOCI_E2E_PENDING === "1"
)
  throw new Error(
    "Auth acceptance requires an allocated normal server, exact source/tree and SMTP capture",
  );
if (process.env.CI && !process.versions.bun) throw new Error("Playwright must run under Bun in CI");

export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: ".",
  testMatch: "**/auth-session-selected-backend.acceptance.ts",
  workers: 1,
  retries: 0,
  metadata: { authSession: true, selectedBackend: backend, selectedFlow: flow, source, tree },
  outputDir: process.env.FVOCI_E2E_RESULT_DIR
    ? `${process.env.FVOCI_E2E_RESULT_DIR}/auth-playwright-output`
    : "test-results-auth-session",
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    // Reset URLs contain a real one-use token. Capture only explicit safe witnesses.
    trace: "off",
    screenshot: "off",
    video: "off",
  },
});
