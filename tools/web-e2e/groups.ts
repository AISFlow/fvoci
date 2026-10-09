import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import type { JSONReport, JSONReportSuite } from "@playwright/test/reporter";

export const repositoryRoot = resolve(import.meta.dir, "../..");
const require = createRequire(import.meta.url);
const playwrightCli = resolve(dirname(require.resolve("playwright/package.json")), "cli.js");

export interface ListedTest {
  id: string;
  file: string;
  title: string;
  expectedStatus: string;
}

export interface DiscoveryOptions {
  config: string;
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  shard?: string;
  selection?: readonly string[];
}

// The installed Playwright CLI owns matching, project dependencies and sharding.
// This adapter reads its public JSON reporter format; it creates no execution plan.
export function listTests(options: DiscoveryOptions): ListedTest[] {
  const result = spawnSync(
    process.execPath,
    [
      "--bun",
      playwrightCli,
      "test",
      "--config",
      options.config,
      "--list",
      "--reporter=json",
      ...(options.shard === undefined ? [] : ["--shard=" + options.shard]),
      ...(options.selection ?? []),
    ],
    {
      cwd: options.cwd ?? repositoryRoot,
      env: {
        ...process.env,
        ...options.env,
        PLAYWRIGHT_JSON_OUTPUT_NAME: "",
        PLAYWRIGHT_JSON_OUTPUT_FILE: "",
      },
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    // CLI output can contain arbitrary test source; retain it only in the caller's
    // private evidence, never turn it into a public diagnostic.
    throw new Error("Playwright discovery failed (exit=" + String(result.status) + ")");
  }
  const report = JSON.parse(result.stdout) as JSONReport;
  if (report.errors.length !== 0) throw new Error("Playwright discovery reported errors");
  const tests: ListedTest[] = [];
  function visit(suite: JSONReportSuite): void {
    for (const spec of suite.specs) {
      const file = relative(report.config.rootDir, resolve(report.config.rootDir, spec.file));
      if (isAbsolute(file) || file === ".." || file.startsWith(".." + sep)) {
        throw new Error("Discovered test is outside testDir");
      }
      for (const test of spec.tests) {
        tests.push({
          id: spec.id + ":" + test.projectId,
          file: file.split(sep).join("/"),
          title: spec.title,
          expectedStatus: test.expectedStatus,
        });
      }
    }
    for (const child of suite.suites ?? []) visit(child);
  }
  for (const suite of report.suites) visit(suite);
  assertRunnableList(tests);
  return tests;
}

export function assertRunnableList(tests: readonly ListedTest[]): void {
  if (tests.length === 0) throw new Error("Selected shard has no tests");
  if (new Set(tests.map((test) => test.id)).size !== tests.length) {
    throw new Error("Duplicate discovered test");
  }
  if (tests.some((test) => test.expectedStatus !== "passed")) {
    throw new Error("Selected test is skipped or expects failure");
  }
}

export function assertShardCoverage(
  full: readonly ListedTest[],
  shards: readonly (readonly ListedTest[])[],
): void {
  assertRunnableList(full);
  for (const shard of shards) assertRunnableList(shard);
  const assigned = shards.flat();
  const expected = new Map(full.map((test) => [test.id, test]));
  if (
    assigned.length !== full.length ||
    new Set(assigned.map((test) => test.id)).size !== assigned.length
  ) {
    throw new Error("Shard coverage has missing or duplicate tests");
  }
  for (const test of assigned) {
    const original = expected.get(test.id);
    if (
      original === undefined ||
      original.file !== test.file ||
      original.title !== test.title ||
      original.expectedStatus !== test.expectedStatus
    ) {
      throw new Error("Shard coverage has foreign or altered tests");
    }
  }
  // A file must retain one fresh runtime within each project, including serial
  // suites. IDs already include project identity, so multi-project coverage is
  // checked per test above rather than conflating project-specific files here.
}

const recovery =
  /real browser offline start|a native committed pause|a planner A-B-A|an estimate A-B-A|a genuine new session retires|transient browser 429|one ordinary task restore/;
const restart = /native same-database restart/;
const controls =
  /ordinary research plan persists|task widget retires|a late task-widget R1|owner releases opaque legacy reservations|a late legacy release/;
export const timerRuns = [
  {
    name: "ordinary",
    count: 9,
    args: ["--grep-invert", [recovery.source, restart.source, controls.source].join("|")],
  },
  { name: "recovery", count: 7, args: ["--grep", recovery.source] },
  { name: "restart", count: 1, args: ["--grep", restart.source] },
  { name: "controls", count: 5, args: ["--grep", controls.source] },
] as const;

export function assertTimerCoverage(
  full: readonly ListedTest[],
  runs: readonly (readonly ListedTest[])[],
): void {
  if (
    runs.length !== timerRuns.length ||
    runs.some((run, index) => run.length !== timerRuns[index]?.count)
  ) {
    throw new Error("Timer requires four fresh runs with counts 9/7/1/5");
  }
  assertShardCoverage(full, runs);
}

// Until B extracts the product's independent workspace setup, discovery must
// retain the complete ordered pair. This is an admission guard, not a scheduler.
export function assertWorkspacePair(tests: readonly ListedTest[]): void {
  const files = [...new Set(tests.map((test) => test.file))];
  const first = files.indexOf("workspace-flow.spec.ts");
  const wiki = files.indexOf("workspace-wiki-flow.spec.ts");
  if (first < 0 || wiki <= first)
    throw new Error("Workspace requires complete ordered setup/wiki coverage");
}
