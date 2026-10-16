import { defineConfig, devices } from "@playwright/test";

// Opt-in real-Keycloak OIDC check, run only by tools/keycloak/run.ts.
// It lives outside e2e/ so the CI group discovery (tools/web-e2e/groups.ts)
// never schedules it.
const out = process.env.FVOCI_KC_E2E_OUT;
const mode = process.env.FVOCI_KC_E2E_MODE ?? "unset";

export default defineConfig({
  forbidOnly: !!process.env.CI,
  testDir: ".",
  testMatch: "oidc-keycloak-flow.spec.ts",
  workers: 1,
  retries: 0,
  reporter: out
    ? [["list"], ["json", { outputFile: `${out}/playwright-${mode}.json` }]]
    : [["list"]],
  use: {
    ...devices["Desktop Chrome"],
    baseURL: process.env.PLAYWRIGHT_BASE_URL,
    // The spec process needs its private oracle inputs; Chromium only needs
    // runtime/locale/socket settings. In particular it inherits no client
    // secret, observer capsule, database credential or native CA setting.
    launchOptions: {
      executablePath: process.env.FVOCI_KC_CHROMIUM_BIN,
      env: Object.fromEntries(
        ["PATH", "LANG", "TZ", "TMPDIR"].flatMap((name) => {
          const value =
            name === "TMPDIR" ? process.env.FVOCI_KC_CHROMIUM_TMPDIR : process.env[name];
          return value === undefined ? [] : [[name, value]];
        }),
      ),
    },
    // A trace, screenshot or video would keep typed test passwords and the
    // URLs that carry codes and state.
    trace: "off",
    screenshot: "off",
    video: "off",
  },
});
