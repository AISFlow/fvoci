import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import { chmodSync, lstatSync, mkdirSync, statSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { configListInputs } from "./admission.ts";
import { env, gid, groups, observedExit, read, root, sha, uid, write } from "./io.ts";
import { knownOnBrowserTest } from "./runtime.ts";

export interface Listing {
  config: { workers: number; metadata: unknown };
  errors: unknown[];
  stats: { expected: number; unexpected: number; flaky: number; skipped: number };
  suites: { specs: { title: string; tests: { results: unknown[] }[] }[] }[];
}
export function qualifyListing(listed: Listing): void {
  assert.ok(listed.config.workers === 1 && deepEquals(listed.errors, []));
  assert.ok(
    deepEquals(listed.config.metadata, { selectedBackend: "postgres", selectedFlow: "on" }),
  );
  assert.ok(
    listed.stats.expected === 0 &&
      listed.stats.unexpected === 0 &&
      listed.stats.flaky === 0 &&
      listed.stats.skipped === 1,
  );
  assert.equal(listed.suites.length, 1);
  const suite = listed.suites[0];
  assert.ok(suite);
  assert.equal(suite.specs.length, 1);
  const spec = suite.specs[0];
  assert.ok(spec);
  assert.ok(
    spec.title === knownOnBrowserTest &&
      spec.tests.length === 1 &&
      deepEquals(spec.tests[0]?.results, []),
  );
}
export function configList(output: string): number {
  const { before, browser, modules } = configListInputs(output),
    directory = join(output, "config-list");
  mkdirSync(directory, { mode: 0o700 });
  for (const name of ["tmp", "bun-transpiler-cache"])
    mkdirSync(join(directory, name), { mode: 0o700 });
  const environment = {
    PATH: env("PATH"),
    LANG: process.env.LANG ?? "C.UTF-8",
    CI: "true",
    TMPDIR: join(directory, "tmp"),
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: join(directory, "bun-transpiler-cache"),
    PLAYWRIGHT_BROWSERS_PATH: env("PLAYWRIGHT_BROWSERS_PATH"),
    FVOCI_E2E_SELECTED_BACKEND: "postgres",
    FVOCI_E2E_SELECTED_FLOW: "on",
    FVOCI_E2E_SELECTED_AUXILIARY: "normal-api",
    FVOCI_E2E_SELECTED_SOURCE: before.head,
    FVOCI_E2E_SELECTED_COMPILED_SOURCE: before.head,
    FVOCI_E2E_RESULT_DIR: directory,
    PLAYWRIGHT_JSON_OUTPUT_FILE: join(directory, "listing.json"),
  };
  const args = [
    browser.bun.path,
    "--no-install",
    join(root, "node_modules/playwright/cli.js"),
    "test",
    "--config",
    "e2e-pending/collab-playwright.config.ts",
    "--reporter=line,json",
    "--list",
    "workspace-wiki-selected-backend.spec.ts",
  ];
  const receipt: Record<string, unknown> = {
    schema: 1,
    phase: "config-list",
    source: before.head,
    tree: before.tree,
    compiledSource: before.head,
    uid: uid(),
    gid: gid(),
    groups: groups(),
    environment_names: Object.keys(environment).sort(),
    list_only: true,
    actual_browser_tests: 0,
    actual_db_tests: 0,
    bun_version: "1.4.2",
    module_hashes: modules,
    executables: Object.fromEntries(
      (["bun", "chromium"] as const).map((name) => {
        const facts = statSync(browser[name].path);
        return [
          name,
          {
            sha256: browser[name].sha256,
            mode: facts.mode & 0o7777,
            uid: facts.uid,
            gid: facts.gid,
          },
        ];
      }),
    ),
  };
  write(join(directory, "start.json"), receipt);
  process.stdout.write(JSON.stringify(receipt) + "\n");
  const mask = process.umask(0o077);
  let code: number;
  try {
    code = observedExit(
      spawnSync(args, {
        cwd: join(root, "apps/web"),
        env: environment,
        stdin: "ignore",
        stdout: "inherit",
        stderr: "inherit",
      }),
    );
  } finally {
    process.umask(mask);
  }
  receipt.exit = code;
  write(join(directory, "result.json"), receipt);
  process.stdout.write(JSON.stringify(receipt) + "\n");
  if (code) return code;
  const report = join(directory, "listing.json"),
    facts = lstatSync(report);
  assert.ok(facts.isFile() && !facts.isSymbolicLink() && facts.uid === 1000);
  chmodSync(report, 0o600);
  qualifyListing(read(report) as Listing);
  configListInputs(output);
  receipt.json_report_sha256 = sha(report);
  receipt.configuration_load_qualified = true;
  write(join(directory, "qualified.json"), receipt);
  process.stdout.write(JSON.stringify(receipt) + "\n");
  return 0;
}
