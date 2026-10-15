import { defineConfig, devices } from "@playwright/test";
import { isAbsolute } from "node:path";
import { lstatSync } from "node:fs";

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:5173";

// The same Bun-in-CI guard as ../playwright.config.ts.
if (process.env.CI && !process.versions.bun) {
  throw new Error("Playwright must run under Bun in CI (bun --bun x playwright)");
}

// A selected normal-server run has an allocated migrate --start lifecycle.
// The pending runner instead starts its test servers per spec.
const selectedBackend = process.env.FVOCI_E2E_SELECTED_BACKEND;

/** Selection only. The admitted parent verifies this executable's full seal. */
export function finalChromiumLaunch() {
  const executablePath = process.env.FVOCI_E2E_FINAL_BROWSER_BIN;
  if (executablePath === undefined) return undefined;
  const restoreDestination =
    selectedBackend === "sqlite" &&
    isAbsolute(process.env.FVOCI_E2E_RESTORE_INPUT ?? "") &&
    isAbsolute(process.env.FVOCI_E2E_RESTORE_SQLITE_ROOT ?? "");
  if (
    process.env.FVOCI_TURSO_UI_FINAL !== "1" ||
    (selectedBackend !== "libsql-remote" && !restoreDestination) ||
    !isAbsolute(executablePath)
  )
    throw new Error("selected final Chromium requires the admitted final allocation");
  const file = lstatSync(executablePath);
  if (!file.isFile() || file.isSymbolicLink())
    throw new Error("selected final Chromium must be a regular executable");
  return {
    executablePath,
    env: Object.fromEntries(
      ["PATH", "LANG", "TZ", "TMPDIR"].flatMap((name) => {
        const value = process.env[name];
        return value === undefined ? [] : [[name, value]];
      }),
    ),
  };
}
const finalLaunch = finalChromiumLaunch();
const selectedFlow = process.env.FVOCI_E2E_SELECTED_FLOW ?? "on";
if (selectedFlow !== "on" && selectedFlow !== "off") {
  throw new Error("selected normal-server flow must be on or off");
}
if (selectedBackend === undefined && process.env.FVOCI_E2E_SELECTED_FLOW !== undefined) {
  throw new Error("selected flow requires its allocated backend startup driver");
}
if (selectedBackend !== undefined) {
  if (!/^(postgres|sqlite|libsql-remote)$/.test(selectedBackend)) {
    throw new Error("selected normal-server backend must be postgres, sqlite or libsql-remote");
  }
  if (process.env.FVOCI_E2E_PENDING === "1") {
    throw new Error("selected normal-server spec requires its allocated startup driver");
  }
}

export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: ".",
  testMatch:
    selectedBackend === undefined
      ? /.*\.spec\.ts/
      : selectedFlow === "off"
        ? /workspace-off-selected-backend\.spec\.ts/
        : /workspace-wiki-selected-backend\.spec\.ts/,
  // Both normal-startup specs are mandatory in the selected companion. Pending
  // specs own their startup and cannot supply either allocated normal server.
  testIgnore:
    selectedBackend === undefined
      ? [
          /workspace-(wiki|off)-selected-backend\.spec\.ts/,
          /v060-sqlite-revision-restore\.spec\.ts/,
          /v060-equal-work\.spec\.ts/,
        ]
      : [],
  metadata: {
    selectedBackend: selectedBackend ?? null,
    selectedFlow: selectedBackend ? selectedFlow : null,
  },
  workers: 1,
  retries: 0,
  outputDir: process.env.FVOCI_E2E_RESULT_DIR
    ? `${process.env.FVOCI_E2E_RESULT_DIR}/playwright-output`
    : "test-results-collab",
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    ...(finalLaunch ? { launchOptions: finalLaunch } : {}),
    trace: selectedBackend === "libsql-remote" ? "off" : "retain-on-failure",
  },
});
