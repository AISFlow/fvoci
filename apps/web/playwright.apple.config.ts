import { availableParallelism } from 'node:os';

import { defineConfig, devices } from '@playwright/test';

import base from './playwright.config.ts';
import { playwrightWorkers, testCpus } from './tools/test-cpus.ts';

/**
 * One WebKit worker for every four processors, two at most: on four (GitHub's
 * runner) two workers failed 10 of the 96 tests of the files that failed
 * there, and one worker 3. Eight processors or more run two, as this config
 * always did; fewer than eight now run one. The processors are those
 * `FVOCI_TEST_CPUS` gives the tests, and the machine's when it is not set.
 */
const processors = testCpus() ?? availableParallelism();
const workers = playwrightWorkers(Math.min(2, Math.max(1, Math.floor(processors / 4))));

/** Apple engines run separately so the default Chromium suite keeps its cost. */
export default defineConfig({
  ...base,
  outputDir: 'test-results/apple',
  workers,
  // Names describe the three layout stages used by the shared browser specs.
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Safari'], deviceScaleFactor: 2 } },
    { name: 'mobile', use: { ...devices['iPhone 17e'] } },
    { name: 'middle', use: { ...devices['iPad Pro 11'] } },
  ],
});
