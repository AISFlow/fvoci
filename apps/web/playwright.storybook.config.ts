import { defineConfig } from '@playwright/test';

import { playwrightWorkers } from './tools/test-cpus.ts';

const port = Number(process.env.FVOCI_STORYBOOK_PORT ?? 18369);

export default defineConfig({
  testDir: 'tools/storybook',
  outputDir: 'test-results/storybook',
  forbidOnly: true,
  fullyParallel: true,
  workers: playwrightWorkers(4),
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${port}/storybook/`,
    timezoneId: 'Asia/Seoul',
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'en-light', use: { viewport: { width: 1280, height: 800 } } },
    { name: 'ko-dark-390', use: { viewport: { width: 390, height: 844 }, hasTouch: true } },
  ],
  webServer: {
    command: `node tools/serve.ts --port ${port} storybook=storybook-static`,
    wait: { stdout: /serve: http/ },
    stdout: 'pipe',
    timeout: 20_000,
  },
});
