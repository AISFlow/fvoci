import { defineConfig, devices } from '@playwright/test';

import { playwrightWorkers } from './tools/test-cpus.ts';

/**
 * End-to-end tests.
 *
 * `FVOCI_E2E_URL` is the address of a running server that serves this build
 * (its `web_app_path` set to `dist/`). Without it the tests start tools/preview.ts,
 * which serves `dist/` with the server's rules but has no API: enough for the
 * tests of the static shell, and only for those.
 *
 * `FVOCI_E2E_PORT` moves the preview to another port, so that two checkouts
 * can run their tests at once. `FVOCI_TEST_CPUS` sets the workers
 * (tools/test-cpus.ts).
 */
const external = process.env.FVOCI_E2E_URL;
const PREVIEW_PORT = Number(process.env.FVOCI_E2E_PORT ?? 18310);

export default defineConfig({
  testDir: 'e2e',
  // A folder of its own: a run empties its output folder first, and the
  // other configurations keep theirs beside this one.
  outputDir: 'test-results/app',
  forbidOnly: Boolean(process.env.CI),
  reporter: [['list']],
  workers: playwrightWorkers(),
  use: {
    baseURL: external ?? `http://127.0.0.1:${PREVIEW_PORT}`,
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'] } },
    { name: 'mobile', use: { ...devices['Pixel 7'] } },
    // Between a phone and a desktop: an unfolded device held upright, touched.
    // The size is a rough figure, not one measured on a device.
    {
      name: 'middle',
      use: { viewport: { width: 626, height: 890 }, hasTouch: true, deviceScaleFactor: 3 },
    },
  ],
  ...(external
    ? {}
    : {
        webServer: {
          command: `node tools/preview.ts ${PREVIEW_PORT}`,
          // No `url` or `port`: with one, Playwright first connects to see
          // whether the address is free, and on a host that drops connections
          // to closed ports instead of refusing them that takes minutes.
          wait: { stdout: /preview: http/ },
          stdout: 'pipe',
          timeout: 20_000,
        },
      }),
});
