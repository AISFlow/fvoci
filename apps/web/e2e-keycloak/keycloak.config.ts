import { defineConfig, devices } from "@playwright/test";

// Opt-in real-Keycloak OIDC check, run only by scripts/keycloak-oidc-e2e.sh.
// It lives outside e2e/ so the CI group discovery (scripts/web-e2e-groups.py)
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
    // A trace, screenshot or video would keep typed test passwords and the
    // URLs that carry codes and state.
    trace: "off",
    screenshot: "off",
    video: "off",
  },
});
