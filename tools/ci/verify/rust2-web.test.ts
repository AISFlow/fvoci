import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { contextFromTexts } from "./load.ts";
import type { Mapping } from "./py.ts";
import {
  WEB_COLLAB_LANES,
  verifyWebBrowserBudget,
  rawJobScalar,
  verifyWebBrowserBudgetJobs,
  verifyWebBuildHandoff,
  verifyWebBuildHandoffJobs,
} from "./web-browser.ts";
import { ROOT, RegistryTree, loadContext, realJobs } from "./rust2-fixture.ts";

const clone = <T>(value: T): T => structuredClone(value);
const steps = (jobs: Mapping, job: string) => (jobs[job] as Mapping)["steps"] as Mapping[];
const stepWith = (jobs: Mapping, job: string, match: (step: Mapping) => boolean) => {
  const step = steps(jobs, job).find(match);
  if (!step) throw new Error(`no matching step in ${job}`);
  return step;
};
const usesOf = (step: Mapping) => (typeof step["uses"] === "string" ? step["uses"] : "");
const byId = (id: string) => (step: Mapping) => step["id"] === id;
const isDownload = (step: Mapping) => usesOf(step).startsWith("actions/download-artifact@");
const unitStep = (jobs: Mapping) =>
  stepWith(jobs, "web-checks", (step) => step["name"] === "Web and editor unit regressions");

test("real web.yml passes through the context entry points", () => {
  const ctx = loadContext(ROOT);
  expect(verifyWebBrowserBudget(ctx)).toEqual([]);
  expect(verifyWebBuildHandoff(ctx)).toEqual([]);
  expect(verifyWebBuildHandoff({ root: ROOT, workflows: {} })).toEqual([]);
  expect(verifyWebBrowserBudget({ root: ROOT, workflows: { "web.yml": { jobs: {} } } })).toEqual(
    [],
  );
});

test("normal browser job budget is measured and fail closed", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebBrowserBudgetJobs(jobs, "20")).toEqual([]);
  expect((jobs["workspace-browser-shard"] as Mapping)["strategy"]).toEqual({
    "fail-fast": false,
    matrix: { shard: [0, 1, 2, 3, 4, 5, 6, 7] },
  });
  for (const job of [
    "web-static",
    "web-native-checks",
    "collaboration-build",
    ...WEB_COLLAB_LANES.map((lane) => lane.name),
  ]) {
    expect((jobs[job] as Mapping)["timeout-minutes"], job).toBe(15);
  }
  // Parsed values; the raw-scalar form (20.0, 020) is covered below.
  for (const value of [null, 0, 15, 16, 19, 21, 30, "20", 20.5, true]) {
    const bad = clone(jobs);
    (bad["workspace-browser-shard"] as Mapping)["timeout-minutes"] = value;
    expect(verifyWebBrowserBudgetJobs(bad, "20"), String(value)).toEqual([
      "web: normal browser shard requires the measured 20 minute job budget",
    ]);
  }
  for (const value of [null, [], "20"]) {
    const bad = clone(jobs);
    bad["workspace-browser-shard"] = value;
    expect(verifyWebBrowserBudgetJobs(bad, "20")).toEqual([
      "web: normal browser shard must be a mapping",
    ]);
  }
  const missing = clone(jobs);
  delete missing["workspace-browser-shard"];
  expect(verifyWebBrowserBudgetJobs(missing, "20")).toEqual([
    "web: normal browser shard must be a mapping",
  ]);
});

test("web current build handoff positive and fail closed", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebBuildHandoffJobs(jobs)).toEqual([]);
  const mutations: [string, (bad: Mapping) => void, string][] = [
    [
      "lane needs",
      (bad) => void ((bad["collaboration-install-on"] as Mapping)["needs"] = "ci-plan"),
      "consumer needs successful registered producer",
    ],
    [
      "producer budget",
      (bad) => void ((bad["collaboration-build"] as Mapping)["timeout-minutes"] = 16),
      "fixed runner/budget",
    ],
    [
      "prepare masked",
      (bad) =>
        void (stepWith(bad, "collaboration-build", byId("prepare"))["continue-on-error"] = true),
      "unconditional qualified producer",
    ],
    [
      "publish masked",
      (bad) =>
        void (stepWith(bad, "collaboration-build", byId("publish"))["continue-on-error"] = true),
      "publish only successful complete packet",
    ],
    [
      "browser masked",
      (bad) =>
        void (stepWith(bad, "collaboration-install-on", byId("browser"))["continue-on-error"] =
          true),
      "mandatory full original runtime",
    ],
    [
      "foreign run id",
      (bad) =>
        void ((stepWith(bad, "collaboration-postgres-on", isDownload)["with"] as Mapping)[
          "run-id"
        ] = "foreign"),
      "current-run exact artifact ID",
    ],
    [
      "digest output",
      (bad) =>
        void (((bad["collaboration-build"] as Mapping)["outputs"] as Mapping)["handoff_sha256"] =
          ""),
      "producer artifact identity",
    ],
    [
      "browser command",
      (bad) =>
        void (stepWith(bad, "collaboration-sqlite-off", byId("browser"))["run"] =
          "bash scripts/run-web-e2e.sh --ci-use-committed-api --with-selected-backends"),
      "mandatory full original runtime",
    ],
    [
      "target cache",
      (bad) =>
        void steps(bad, "collaboration-postgres-off").push({
          uses: "actions/cache@anything",
          with: { path: "target" },
        }),
      "consumer cannot borrow target cache",
    ],
    [
      "foreign download",
      (bad) =>
        void steps(bad, "collaboration-postgres-on").push({
          name: "Foreign download",
          uses: "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093",
          with: {
            "artifact-ids": "${{ needs.collaboration-build.outputs.artifact_id }}",
            "merge-multiple": true,
            path: "${{ runner.temp }}/foreign",
            "run-id": "999999",
            "github-token": "${{ secrets.GITHUB_TOKEN }}",
          },
        }),
      "current-run exact artifact ID",
    ],
    [
      "missing receipt download",
      (bad) => {
        const job = bad["collaboration-sqlite-on"] as Mapping;
        job["steps"] = steps(bad, "collaboration-sqlite-on").filter(
          (step) =>
            (step["with"] as Mapping | undefined)?.["path"] !==
            "${{ runner.temp }}/fvoci-closed-install",
        );
      },
      "current-run exact artifact ID",
    ],
    [
      "unpinned download",
      (bad) =>
        void (stepWith(bad, "collaboration-install-on", isDownload)["uses"] =
          "actions/download-artifact@v4"),
      "current-run exact artifact ID",
    ],
    [
      "checkout credentials",
      (bad) =>
        void (stepWith(bad, "collaboration-build", (s) =>
          usesOf(s).startsWith("actions/checkout@"),
        )["with"] = {}),
      "default exact checkout",
    ],
    [
      "selection",
      (bad) => void ((bad["collaboration-sqlite-on"] as Mapping)["if"] = "always()"),
      "selection only by registered plan",
    ],
    [
      "authority",
      (bad) =>
        void ((bad["collaboration-build"] as Mapping)["permissions"] = { contents: "write" }),
      "no masked/alternate authority",
    ],
    [
      "non-mapping step",
      (bad) => void steps(bad, "collaboration-build").push("oops" as unknown as Mapping),
      "job collaboration-build steps must be a list of mappings",
    ],
    [
      "non-mapping job",
      (bad) => void (bad["collaboration-sqlite-off"] = null),
      "job collaboration-sqlite-off must be a mapping",
    ],
  ];
  for (const [name, mutate, needle] of mutations) {
    const bad = clone(jobs);
    mutate(bad);
    const errors = verifyWebBuildHandoffJobs(bad);
    expect(errors.join("\n"), name).toContain("web: current build handoff " + needle);
  }
});

test("selected registration commands cannot be missing or masked", () => {
  const jobs = realJobs("web.yml");
  for (const command of [
    "(cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)",
  ]) {
    const bad = clone(jobs);
    const step = unitStep(bad);
    expect(step["run"] as string).toContain(command + "\n");
    step["run"] = (step["run"] as string).replace(command + "\n", "");
    expect(verifyWebBuildHandoffJobs(bad)).toEqual([
      "web: current build handoff mandatory complete web/editor and selected registration fixtures",
    ]);
  }
  for (const [key, value] of [
    ["if", "false"],
    ["continue-on-error", true],
  ] as const) {
    const bad = clone(jobs);
    unitStep(bad)[key] = value;
    expect(verifyWebBuildHandoffJobs(bad).length, key).toBe(1);
  }
});

const BUDGET_ERROR = "web: normal browser shard requires the measured 20 minute job budget";

function budgetWithRaw(raw: string): string[] {
  using tree = new RegistryTree([]);
  const rel = ".github/workflows/web.yml";
  const text = tree.read(rel);
  const start = text.indexOf("  workspace-browser-shard:\n");
  const tail = text.slice(start);
  expect(tail).toContain("\n    timeout-minutes: 20\n");
  tree.write(
    rel,
    text.slice(0, start) +
      tail.replace("\n    timeout-minutes: 20\n", `\n    timeout-minutes: ${raw}\n`),
  );
  return verifyWebBrowserBudget(loadContext(tree.root));
}

test("browser budget raw scalar must be a plain decimal integer", () => {
  expect(budgetWithRaw("20")).toEqual([]);
  expect(budgetWithRaw("20 # measured")).toEqual([]);
  // Bun.YAML reads each of these as the number 20; PyYAML does not.
  for (const raw of ["20.0", "020", "0x14", "+20", "2_0", "20.", "2e1"]) {
    expect(budgetWithRaw(raw), raw).toEqual([BUDGET_ERROR]);
  }
});

// Applies `edit` to the workspace-browser-shard job text only; `append` also
// adds a copy of the job, with that budget line set to 20.0, at the file end.
function budgetWithSource(edit: (job: string) => string, append = ""): string[] {
  using tree = new RegistryTree([]);
  const rel = ".github/workflows/web.yml";
  const text = tree.read(rel);
  const start = text.indexOf("  workspace-browser-shard:\n");
  const end = text.slice(start + 1).search(/\n {2}[^ #\n]/) + start + 2;
  const job = text.slice(start, end);
  const changed =
    text.slice(0, start) +
    edit(job) +
    text.slice(end) +
    (append === "" ? "" : job.replace(append, "\n    timeout-minutes: 20.0\n"));
  expect(changed).not.toBe(text);
  tree.write(rel, changed);
  const ctx = loadContext(tree.root);
  return [...verifyWebBrowserBudget(ctx), ...verifyWebBuildHandoff(ctx)];
}

test("duplicate spellings of the budget path fail closed", () => {
  // Both loaders keep the last duplicate (Python then refuses 20.0) while a
  // first-match raw lookup would read the earlier 20.
  const line = "\n    timeout-minutes: 20\n";
  for (const second of [
    '"timeout-minutes": 20.0',
    "'timeout-minutes': 20.0",
    '"timeout\\u002dminutes": 20.0',
  ]) {
    const errors = budgetWithSource((text) => text.replace(line, `${line}    ${second}\n`));
    expect(errors, second).toEqual([BUDGET_ERROR]);
  }
  const duplicateJob = budgetWithSource(
    (job) => job + job.replace(line, "\n    timeout-minutes: 20.0\n"),
  );
  expect(duplicateJob).toEqual([BUDGET_ERROR]);
  // The reviewer's form: the copy with 20.0 appended at the end of jobs.
  expect(budgetWithSource((job) => job, line)).toEqual([BUDGET_ERROR]);
  // A quoted single spelling is not the plain form the raw check accepts.
  expect(budgetWithSource((text) => text.replace(line, '\n    "timeout-minutes": 20\n'))).toEqual([
    BUDGET_ERROR,
  ]);
  // Unrelated duplicates elsewhere in the job stay outside this check.
  expect(
    budgetWithSource((text) => text.replace(line, `${line}    "name": dup\n    'name': dup\n`)),
  ).toEqual([]);
});

test("raw block scalar refuses unclassified key lines on the path", () => {
  const job = (body: string) => `jobs:\n  j:\n${body}`;
  expect(rawJobScalar(job("    t: 5\n"), "j", "t")).toBe("5");
  for (const body of [
    "    <<: *base\n    t: 5\n",
    "    ? t\n    : 5\n",
    "    &a t: 5\n",
    "    \tt: 5\n",
  ]) {
    expect(rawJobScalar(job(body), "j", "t"), JSON.stringify(body)).toBeNull();
  }
  expect(rawJobScalar('"jobs":\n  j:\n    t: 5\njobs:\n  j:\n    t: 6\n', "j", "t")).toBeNull();
  expect(rawJobScalar("jobs:\n  'j':\n    t: 5\n  j:\n    t: 6\n", "j", "t")).toBeNull();
});

test("raw job scalar lookup is exact and fails closed", () => {
  const source = [
    "on: push",
    "jobs:",
    "  a:",
    "    timeout-minutes: 7 # c",
    "    steps:",
    "      - timeout-minutes: 9",
    "  b:",
    "    run: |",
    "      timeout-minutes: 3",
    "  c:",
    "    timeout-minutes: 1",
    "    timeout-minutes: 2",
    "",
  ].join("\n");
  expect(rawJobScalar(source, "a", "timeout-minutes")).toBe("7");
  expect(rawJobScalar(source, "b", "timeout-minutes")).toBeNull();
  expect(rawJobScalar(source, "c", "timeout-minutes")).toBeNull();
  expect(rawJobScalar(source, "missing", "timeout-minutes")).toBeNull();
  expect(rawJobScalar("jobs: {a: {timeout-minutes: 7}}\n", "a", "timeout-minutes")).toBeNull();
  expect(verifyWebBrowserBudgetJobs(realJobs("web.yml"), null)).toEqual([BUDGET_ERROR]);
});

test("the budget scalar comes from the context text, not the file on disk", () => {
  const source = readFileSync(join(ROOT, ".github/workflows/web.yml"), "utf8");
  const marker = "\n    timeout-minutes: 20\n";
  const start = source.indexOf("  workspace-browser-shard:\n");
  expect(source.indexOf(marker, start)).toBeGreaterThan(start);
  const at = source.indexOf(marker, start);
  const text =
    source.slice(0, at) + "\n    timeout-minutes: 20.0\n" + source.slice(at + marker.length);
  expect(verifyWebBrowserBudget(contextFromTexts(ROOT, { "web.yml": text }))).toEqual([
    BUDGET_ERROR,
  ]);
  expect(verifyWebBrowserBudget(contextFromTexts(ROOT, { "web.yml": source }))).toEqual([]);
});
