import { defineConfig, devices } from '@playwright/test';

/**
 * End-to-end tests against the real server: `pnpm --filter web run e2e:live`.
 *
 * tools/live-server.ts starts the server built in this checkout (`cargo
 * build -p fvoci-server --bins --examples`, which builds the fake OpenID
 * Connect provider beside it, on the next port, and the fake search server it
 * searches through, nine ports up) on SQLite, in a new data folder under
 * `target/web-live/`, serving this app's build (`pnpm run build` first). The
 * server stops when the run ends, passed or failed.
 *
 * `FVOCI_LIVE_PORT` moves it to another port (18510 by default), so that
 * two checkouts can run at once. The tests share the one server: each makes
 * the accounts and boards it needs, under names of its own.
 */
const PORT = Number(process.env.FVOCI_LIVE_PORT ?? 18510);
/** The tests of the first use of a server (e2e-live/first-use.spec.ts). */
const FIRST_USE = /first-use\.spec\.ts$/;
/** The tests of the open PostgreSQL server, which has no search server: its database is searched. */
const ORGANIZATIONS = /(organizations|search-database)\.spec\.ts$/;
/** The tests of a server that confirms the address of a registration by mail. */
const VERIFYING = /verify-email\.spec\.ts$/;

/** The tests that close the server's one organization to passwords. */
const SSO = /(^|\/)sso(-operator|-pending|-provider-move)?\.spec\.ts$/;

export default defineConfig({
  testDir: 'e2e-live',
  outputDir: 'test-results/live',
  forbidOnly: Boolean(process.env.CI),
  reporter: [['list']],
  // One server, one database: the tests run one after another.
  workers: 1,
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'retain-on-failure',
  },
  // The tests of the first use start on a server that nobody has used: they
  // run first, and make the administrator the others work beside. The test
  // that closes that administrator's organization to passwords runs after the
  // others are done, whose passwords it would refuse meanwhile (alone:
  // `e2e:live --project=sso --no-deps`).
  projects: [
    { name: 'first-use', testMatch: FIRST_USE, use: { ...devices['Desktop Chrome'] } },
    {
      name: 'desktop',
      testIgnore: [FIRST_USE, ORGANIZATIONS, SSO, VERIFYING],
      dependencies: ['first-use'],
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'sso',
      testMatch: SSO,
      dependencies: ['desktop'],
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'organizations',
      testMatch: ORGANIZATIONS,
      use: { ...devices['Desktop Chrome'], baseURL: `http://localhost:${PORT + 4}` },
    },
    {
      name: 'verifying',
      testMatch: VERIFYING,
      use: { ...devices['Desktop Chrome'], baseURL: `http://localhost:${PORT + 8}` },
    },
  ],
  webServer: [
    {
      command: `node tools/live-server.ts ${PORT}`,
      // No `url` or `port`: on this host a connection to a closed port hangs
      // instead of being refused (playwright.config.ts says more).
      wait: { stdout: /live: http/ },
      stdout: 'pipe',
      stderr: 'pipe',
      timeout: 60_000,
      gracefulShutdown: { signal: 'SIGTERM', timeout: 15_000 },
    },
    {
      command: `node tools/live-server.ts ${PORT + 4} --organizations`,
      wait: { stdout: /live: http/ },
      stdout: 'pipe',
      stderr: 'pipe',
      timeout: 60_000,
      gracefulShutdown: { signal: 'SIGTERM', timeout: 15_000 },
    },
    {
      command: `node tools/live-server.ts ${PORT + 8} --verifying`,
      wait: { stdout: /live: http/ },
      stdout: 'pipe',
      stderr: 'pipe',
      timeout: 60_000,
      gracefulShutdown: { signal: 'SIGTERM', timeout: 15_000 },
    },
  ],
});
