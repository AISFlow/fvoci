import { expect, test } from "bun:test";
import type { Mapping } from "./py.ts";
import {
  WEB_COLLAB_LANES,
  verifyWebBrowserBudget,
  verifyWebBrowserBudgetJobs,
  verifyWebBuildHandoff,
  verifyWebBuildHandoffJobs,
} from "./web-browser.ts";
import { ROOT, loadContext, realJobs } from "./rust2-fixture.ts";

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
  expect(verifyWebBrowserBudgetJobs(jobs)).toEqual([]);
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
  // 20.0 is indistinguishable from 20 after YAML parsing (contract table).
  for (const value of [null, 0, 15, 16, 19, 21, 30, "20", 20.5, true]) {
    const bad = clone(jobs);
    (bad["workspace-browser-shard"] as Mapping)["timeout-minutes"] = value;
    expect(verifyWebBrowserBudgetJobs(bad), String(value)).toEqual([
      "web: normal browser shard requires the measured 20 minute job budget",
    ]);
  }
  for (const value of [null, [], "20"]) {
    const bad = clone(jobs);
    bad["workspace-browser-shard"] = value;
    expect(verifyWebBrowserBudgetJobs(bad)).toEqual([
      "web: normal browser shard must be a mapping",
    ]);
  }
  const missing = clone(jobs);
  delete missing["workspace-browser-shard"];
  expect(verifyWebBrowserBudgetJobs(missing)).toEqual([
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
    "python3 scripts/selected-backend-ci/test_off_registration.py",
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
