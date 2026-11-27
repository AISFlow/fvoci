import { defineConfig, devices } from '@playwright/test';

import base from './playwright.live.config.ts';

/** The same fresh-server onboarding and live scenarios, through Safari's engine. */
export default defineConfig({
  ...base,
  outputDir: 'test-results/live-apple',
  // The scenarios are long, and Linux's WebKit reached over the network (its image) runs them
  // about two and a half times as slow as Chromium here (the SSO move: 12.8s against past 30):
  // the time a whole test may take, not what any step waits for.
  timeout: 60_000,
  projects: base.projects?.map((project) => ({
    ...project,
    // The project's own address stays: the organizations and verifying servers are others.
    use: { ...project.use, ...devices['Desktop Safari'], deviceScaleFactor: 2 },
  })),
});
