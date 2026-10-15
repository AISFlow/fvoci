import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browsers, flows, boundaryTitle, PRODUCT_TITLES } from "./selected-final-contract";

const namespace = "tui-0123456789abcdef0123";
const actor = (name: string, i: number) => ({
  userId: `00000000-0000-4000-8000-00000000000${String(i)}`,
  email: `${namespace}-${name}@example.invalid`,
  password: "test-only-password",
});
const original = {
  schema: 1,
  backend: "libsql-remote",
  source: "a".repeat(40),
  tree: "b".repeat(40),
  namespace,
  workspaceId: "00000000-0000-4000-8000-000000000001",
  workspaceSlug: namespace,
  setupNeeded: false,
  schemaCurrent: true,
  commit: "confirmed",
  lifecycleDrain: "confirmed",
  leases: 0,
  owner: actor("owner", 2),
  member: actor("member", 3),
  outsider: actor("outsider", 4),
};
const config = resolve(import.meta.dir, "selected-final-playwright.config.ts");
const cli = fileURLToPath(import.meta.resolve("@playwright/test/cli"));

function discovery(
  browser: string,
  flow: string,
  suite: string,
  changedEnv: Record<string, string> = {},
) {
  const directory = mkdtempSync(join(tmpdir(), "selected-final-list-"));
  const path = join(directory, "actors.json");
  try {
    writeFileSync(path, JSON.stringify({ ...original, browser, flow, suite }), { mode: 0o600 });
    // A --list child imports config/spec only; it never launches the deliberately
    // absent browser executable or reaches the normal-server beforeAll.
    const result = Bun.spawnSync(
      [
        process.execPath,
        "--no-env-file",
        cli,
        "test",
        "--config",
        config,
        "--list",
        "--reporter=json",
      ],
      {
        cwd: resolve(import.meta.dir, ".."),
        env: {
          PATH: process.env.PATH ?? "",
          LANG: "C.UTF-8",
          TMPDIR: tmpdir(),
          CI: "true",
          FVOCI_E2E_SELECTED_BACKEND: original.backend,
          FVOCI_E2E_SELECTED_FLOW: flow,
          FVOCI_E2E_SELECTED_SOURCE: original.source,
          FVOCI_E2E_SELECTED_TREE: original.tree,
          FVOCI_E2E_TURSO_NAMESPACE: namespace,
          FVOCI_E2E_FINAL_BROWSER: browser,
          FVOCI_E2E_FINAL_SUITE: suite,
          FVOCI_E2E_FINAL_ACTOR_BINDING: path,
          FVOCI_E2E_FINAL_BROWSER_BIN: join(directory, "absent-browser"),
          FVOCI_E2E_RESULT_DIR: directory,
          PLAYWRIGHT_BASE_URL: "http://127.0.0.1:1",
          ...changedEnv,
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    return {
      code: result.exitCode,
      output: result.stdout.toString(),
      error: result.stderr.toString(),
    };
  } finally {
    rmSync(directory, { recursive: true });
  }
}

type Suite = { suites?: Suite[]; specs?: { title: string; file: string }[] };
type Report = Suite & {
  errors: unknown[];
  config: {
    workers: number;
    forbidOnly: boolean;
    metadata: Record<string, unknown>;
    projects: { retries: number }[];
  };
};
const titles = (suite: Suite): string[] => [
  ...(suite.specs ?? []).map((s) => s.title),
  ...(suite.suites ?? []).flatMap(titles),
];

test("real Playwright discovery registers four product cases against base fixtures", () => {
  const result = discovery("chromium", "on", "products");
  expect(result.code, result.error).toBe(0);
  const report = JSON.parse(result.output) as Report;
  expect(titles(report)).toEqual([...PRODUCT_TITLES]);
  expect(report.errors).toEqual([]);
  expect(report.config.workers).toBe(1);
  expect(report.config.forbidOnly).toBe(true);
  expect(report.config.metadata).toMatchObject({
    selectedFinal: true,
    selectedFlow: "on",
    browser: "chromium",
  });
  expect(report.config.projects).toHaveLength(1);
  expect(report.config.projects[0]?.retries).toBe(0);
});

test("real metadata discovery keeps all six browser/flow boundary cases separate", () => {
  for (const browser of browsers)
    for (const flow of flows) {
      const result = discovery(browser, flow, "boundary");
      expect(result.code, result.error).toBe(0);
      const report = JSON.parse(result.output) as Report;
      expect(report.errors).toEqual([]);
      expect(titles(report)).toEqual([boundaryTitle(browser, flow)]);
      expect(report.config.metadata).toMatchObject({
        browser,
        selectedFlow: flow,
        suite: "boundary",
      });
    }
});

test("invalid allocation and pending fixture collisions fail in discovery before hooks", () => {
  for (const [browser, flow, suite, env] of [
    ["safari", "on", "boundary", {}],
    ["chromium", "OFF", "boundary", {}],
    ["firefox", "on", "products", {}],
    ["chromium", "off", "products", {}],
    ["chromium", "on", "products", { FVOCI_E2E_PENDING: "1" }],
    ["chromium", "on", "products", { FVOCI_E2E_SELECTED_SOURCE: "c".repeat(40) }],
    ["chromium", "on", "products", { FVOCI_E2E_FINAL_BROWSER_BIN: "relative-browser" }],
  ] as [string, string, string, Record<string, string>][]) {
    const result = discovery(browser, flow, suite, env);
    expect(result.code).not.toBe(0);
    expect(result.error).toContain(
      env.FVOCI_E2E_FINAL_BROWSER_BIN
        ? "SELECTED_FINAL_ALLOCATION_REQUIRED"
        : "SELECTED_FINAL_INPUT_REFUSED",
    );
  }
});
