import { describe, expect, test } from "bun:test";
import { dispatchOptIns, eventShas, EventShapeError, mergeGroupShas } from "./events.ts";
import { decideFromPaths } from "./paths.ts";
import { buildPlan, sanitizeReasonCode } from "./plan.ts";
import { pyLoads, type PyValue } from "./pyjson.ts";
import { WORKFLOW_JOBS, WORKFLOWS, type Workflow } from "./registry.ts";
import { planAll, selectedJobs, SHA_A, SHA_B, SHA_C } from "./test-support.ts";

const ev = (json: string): PyValue => pyLoads(json);
const ALL_BUT_OPT_IN: Record<Workflow, string[]> = {
  web: [...WORKFLOW_JOBS.web],
  rust: [...WORKFLOW_JOBS.rust],
  documents: [...WORKFLOW_JOBS.documents],
  "collab-engine": [...WORKFLOW_JOBS["collab-engine"]],
  install: ["install-smoke", "backup-restore-smoke"],
};
const NONE: Record<Workflow, string[]> = {
  web: [],
  rust: [],
  documents: [],
  "collab-engine": [],
  install: [],
};
const FRONTEND = {
  ...NONE,
  web: [...WORKFLOW_JOBS.web],
  install: ["install-smoke", "backup-restore-smoke"],
};
const WEB_ONLY = { ...NONE, web: [...WORKFLOW_JOBS.web] };

const CHANGE_KINDS: Record<string, string[]> = {
  "docs-only markdown": ["README.md", "docs/other.md", "AGENTS.md"],
  "fixture markdown": ["compat/fixtures/markdown-oracle/new.md"],
  "consumed markdown": ["apps/web/NOTICE.md"],
  "prettier markdown": ["scripts/WEB_LINT.md"],
  frontend: ["apps/web/src/x.ts"],
  "web tests": ["apps/web/e2e/x.spec.ts"],
  code: ["src/lib.rs"],
  workflow: [".github/workflows/web.yml"],
  "mixed docs+frontend": ["README.md", "apps/web/src/x.ts"],
  "mixed docs+code": ["README.md", "src/lib.rs"],
  unknown: ["new-unknown-file.ts"],
};
const PR_EXPECTED: Record<string, [string, Record<Workflow, string[]>]> = {
  "docs-only markdown": ["NARROW_DOCS", NONE],
  "fixture markdown": ["FULL_PATH_BROADEN", ALL_BUT_OPT_IN],
  "consumed markdown": ["FULL_PATH_BROADEN", ALL_BUT_OPT_IN],
  "prettier markdown": ["NARROW_FRONTEND_WEB_INSTALL", FRONTEND],
  frontend: ["NARROW_FRONTEND_WEB_INSTALL", FRONTEND],
  "web tests": ["NARROW_WEB_TESTS", WEB_ONLY],
  code: ["FULL_PATH_BROADEN", ALL_BUT_OPT_IN],
  workflow: ["FULL_PATH_BROADEN", ALL_BUT_OPT_IN],
  "mixed docs+frontend": ["NARROW_FRONTEND_WEB_INSTALL", FRONTEND],
  "mixed docs+code": ["FULL_PATH_BROADEN", ALL_BUT_OPT_IN],
  unknown: ["FULL_UNKNOWN_PATH", ALL_BUT_OPT_IN],
};

describe("event x change kind", () => {
  for (const [kind, paths] of Object.entries(CHANGE_KINDS)) {
    test(`pull_request / ${kind}`, () => {
      const [reason, lanes] = PR_EXPECTED[kind] as [string, Record<Workflow, string[]>];
      for (const [workflow, plan] of Object.entries(planAll(paths)) as [
        Workflow,
        ReturnType<typeof buildPlan>,
      ][]) {
        expect(plan.reason_code).toBe(reason);
        expect(plan.plan_ok).toBe(true);
        expect(selectedJobs(plan), `${kind} ${workflow}`).toEqual(lanes[workflow]);
      }
    });
    for (const [eventName, reason] of [
      ["merge_group", "FULL_EVENT_MERGE_GROUP"],
      ["push", "FULL_EVENT_PUSH"],
      ["workflow_dispatch", "FULL_EVENT_WORKFLOW_DISPATCH"],
    ] as const) {
      test(`${eventName} / ${kind} selects every lane`, () => {
        for (const [workflow, plan] of Object.entries(planAll(paths, eventName)) as [
          Workflow,
          ReturnType<typeof buildPlan>,
        ][]) {
          expect(plan).toMatchObject({ mode: "full", reason_code: reason, plan_ok: true });
          expect(selectedJobs(plan)).toEqual(ALL_BUT_OPT_IN[workflow]);
        }
      });
    }
    test(`unknown event / ${kind} is full and not ok`, () => {
      for (const eventName of ["schedule", "Push", "pull_request_target", "repository_dispatch"]) {
        for (const [workflow, plan] of Object.entries(planAll(paths, eventName)) as [
          Workflow,
          ReturnType<typeof buildPlan>,
        ][]) {
          expect(plan).toMatchObject({
            mode: "full",
            reason_code: "EVENT_UNKNOWN",
            plan_ok: false,
          });
          expect(selectedJobs(plan)).toEqual(ALL_BUT_OPT_IN[workflow]);
        }
      }
    });
  }

  test("workflow_dispatch opt-in adds exactly the opted-in job", () => {
    const plans = planAll(null, "workflow_dispatch", {
      optInInputs: new Set(["run_upgrade_smoke_arm"]),
    });
    for (const workflow of WORKFLOWS) {
      const want = workflow === "install" ? [...WORKFLOW_JOBS.install] : ALL_BUT_OPT_IN[workflow];
      expect(selectedJobs(plans[workflow])).toEqual(want);
    }
  });

  test("a fatal input wins over the event and never narrows", () => {
    for (const eventName of [
      "pull_request",
      "merge_group",
      "push",
      "workflow_dispatch",
      "schedule",
    ]) {
      for (const fatal of [
        "TESTED_SHA_INVALID",
        "TESTED_SHA_MISMATCH",
        "GIT_DIFF_FAILED",
        "DIFF_TRUNCATED",
        "FETCH_FAILED",
      ]) {
        const plans = planAll(["README.md"], eventName, {
          fatalError: fatal,
          optInInputs: new Set(["run_upgrade_smoke_arm"]),
        });
        for (const workflow of WORKFLOWS) {
          expect(plans[workflow]).toMatchObject({
            mode: "full",
            reason_code: fatal,
            plan_ok: false,
          });
          expect(selectedJobs(plans[workflow])).toEqual(ALL_BUT_OPT_IN[workflow]);
        }
      }
    }
  });

  test("missing paths on a pull request is full and not ok", () => {
    for (const plan of Object.values(planAll(null))) {
      expect(plan).toMatchObject({
        mode: "full",
        reason_code: "FULL_MISSING_PATHS",
        plan_ok: false,
        path_count: 0,
      });
    }
  });

  test("a checkout that cannot bind is full but ok", () => {
    for (const plan of Object.values(
      planAll(["README.md"], "pull_request", { forceFullReason: "FULL_PR_MERGE_PARENTS_MISMATCH" }),
    )) {
      expect(plan).toMatchObject({
        mode: "full",
        reason_code: "FULL_PR_MERGE_PARENTS_MISMATCH",
        plan_ok: true,
      });
    }
  });
});

describe("buildPlan", () => {
  test("plan v3 shape and key order", () => {
    const plan = buildPlan({
      workflow: "install",
      eventName: "pull_request",
      baseSha: SHA_A,
      headSha: SHA_B,
      mergeBaseSha: SHA_C,
      testedSha: SHA_B,
      paths: ["apps/web/src/x.ts", "README.md"],
    });
    expect(Object.keys(plan)).toEqual([
      "version",
      "workflow",
      "mode",
      "reason_code",
      "plan_ok",
      "base_sha",
      "head_sha",
      "merge_base_sha",
      "tested_sha",
      "path_count",
      "jobs",
    ]);
    expect(plan.jobs).toEqual({
      "install-smoke": { selected: true },
      "backup-restore-smoke": { selected: true },
      "upgrade-smoke-arm64": { selected: false },
    });
    expect(plan.path_count).toBe(2);
    expect(plan.version).toBe(3);
  });

  test("producer and consumer jobs share one selection", () => {
    for (const paths of Object.values(CHANGE_KINDS)) {
      const plans = planAll(paths);
      expect(plans.rust.jobs["postgres-build"]).toEqual(
        plans.rust.jobs.postgres as { selected: boolean },
      );
      expect(plans.web.jobs["workspace-browser-build"]).toEqual(
        plans.web.jobs["workspace-browser-shard"] as { selected: boolean },
      );
    }
  });

  test("reason codes are validated", () => {
    expect(() => sanitizeReasonCode("bad code")).toThrow("unsafe reason code");
    expect(() => planAll(null, "pull_request", { fatalError: "x;rm" })).toThrow(
      "unsafe reason code",
    );
    expect(decideFromPaths(["README.md"]).reasonCode).toBe("NARROW_DOCS");
  });
});

describe("event payloads", () => {
  test("merge_group copies only well-formed SHAs", () => {
    expect(
      mergeGroupShas(
        ev(`{"merge_group":{"base_sha":"${SHA_A}","head_sha":"${SHA_B}"},"pull_request":{}}`),
      ),
    ).toEqual([SHA_A, SHA_B]);
    for (const bad of [
      "[]",
      "{}",
      '{"merge_group":[]}',
      '{"merge_group":{"base_sha":"A","head_sha":3}}',
    ]) {
      expect(mergeGroupShas(ev(bad)), bad).toEqual([null, null]);
    }
    expect(mergeGroupShas(ev(`{"merge_group":{"base_sha":"${SHA_A}\\n"}}`))).toEqual([null, null]);
  });

  test("pull_request and push values are carried as given; impossible shapes refuse", () => {
    expect(
      eventShas(ev(`{"pull_request":{"base":{"sha":"x"},"head":{}}}`), "pull_request"),
    ).toEqual(["x", null]);
    expect(eventShas(ev("{}"), "pull_request")).toEqual([null, null]);
    expect(eventShas(ev('{"pull_request":0}'), "pull_request")).toEqual([null, null]);
    for (const bad of [
      "[]",
      '{"pull_request":"x"}',
      '{"pull_request":{"base":null}}',
      '{"pull_request":{"head":[]}}',
    ]) {
      expect(() => eventShas(ev(bad), "pull_request"), bad).toThrow(EventShapeError);
    }
    expect(eventShas(ev('{"before":"a","after":1}'), "push")).toEqual(["a", pyLoads("1")]);
    expect(() => eventShas(ev("[]"), "push")).toThrow(EventShapeError);
    expect(eventShas(ev("[]"), "schedule")).toEqual([null, null]);
  });

  test("dispatch opt-ins fail closed", () => {
    const install = (json: string) => dispatchOptIns("install", "workflow_dispatch", ev(json));
    for (const json of [
      "{}",
      '{"inputs":null}',
      '{"inputs":{}}',
      '{"inputs":{"run_upgrade_smoke_arm":"false"}}',
      '{"inputs":{"run_upgrade_smoke_arm":false}}',
    ]) {
      expect(install(json), json).toEqual({ chosen: new Set(), error: null });
    }
    for (const value of ['"true"', "true"]) {
      expect(install(`{"inputs":{"run_upgrade_smoke_arm":${value}}}`).chosen).toEqual(
        new Set(["run_upgrade_smoke_arm"]),
      );
    }
    for (const [json, code] of [
      ["[]", "DISPATCH_EVENT_INVALID"],
      ['{"inputs":"true"}', "DISPATCH_INPUTS_INVALID"],
      ['{"inputs":{"run_upgrade_smoke_arm":"TRUE"}}', "DISPATCH_INPUT_VALUE_INVALID"],
      ['{"inputs":{"run_upgrade_smoke_arm":1}}', "DISPATCH_INPUT_VALUE_INVALID"],
      ['{"inputs":{"run_upgrade_smoke_arm":null}}', "DISPATCH_INPUT_VALUE_INVALID"],
      [
        `{"inputs":{"run_upgrade_smoke_arm":"true","old":"${"d".repeat(40)}"}}`,
        "DISPATCH_INPUTS_UNKNOWN",
      ],
    ] as const) {
      expect(install(json), json).toEqual({ chosen: new Set(), error: code });
    }
    expect(
      dispatchOptIns("web", "workflow_dispatch", ev('{"inputs":{"run_upgrade_smoke_arm":"true"}}'))
        .error,
    ).toBe("DISPATCH_INPUTS_UNKNOWN");
    for (const eventName of ["pull_request", "push", "merge_group"]) {
      expect(
        dispatchOptIns("install", eventName, ev('{"inputs":{"run_upgrade_smoke_arm":"true"}}')),
      ).toEqual({
        chosen: new Set(),
        error: null,
      });
    }
  });
});
