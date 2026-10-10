import { afterAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { runGate, type GateRun } from "../gate.ts";
import {
  RESULTS,
  SHA_A,
  SHA_B,
  catalogRows,
  fullSelection,
  gateCorpus,
  honestResults,
  materialize,
  matrixFor,
  needs,
  plan,
  type NeedsOptions,
} from "./corpus.ts";
import { postgresMatrixGateError, postgresMatrixInclude } from "./matrix.ts";
import { dispatchOptIns } from "./opt-in.ts";
import {
  OPT_IN_JOBS,
  PLAN_JOB_ID,
  WORKFLOWS,
  WORKFLOW_JOBS,
  gateJobId,
  type Plan,
  type Workflow,
} from "./schema.ts";
import { isBlank, strip } from "./text.ts";

const scratch = mkdtempSync(join(tmpdir(), "fvoci-gate-"));
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});
let eventFiles = 0;
function writeEvent(contents: string | Uint8Array): string {
  const path = join(scratch, `event-${String(eventFiles++)}.json`);
  writeFileSync(path, contents);
  return path;
}

describe("corpus", () => {
  const cases = gateCorpus();

  test("Python-oracle differences stay the reviewed list", () => {
    expect(cases.filter((c) => c.knownDifference).map((c) => c.name)).toEqual([
      "needs/plan-json-infinity",
      "needs/schema-top-null",
      "needs/nan-in-needs",
      "needs/reason-code-trailing-newline",
      "needs/tested-sha-trailing-newline",
      "needs/schema-install-producer-install-smoke-backup-restore-smoke",
      "needs/schema-install-producer-install-image-install-smoke",
      "opt-in/nan-event",
      "usage/short-help-tail",
      "usage/short-help-tail-before-invalid",
      "usage/short-help-twice",
      "usage/help-prefix",
      "usage/prefix-options",
      "usage/prefix-abbreviations",
      "usage/negative-prefix",
      "usage/negative-number",
      "usage/negative-decimal-prefix",
      "usage/negative-arabic-indic",
      "usage/negative-fullwidth",
      "usage/negative-final-newline",
      "usage/negative-astral-digit",
      "usage/value-with-space",
      "usage/lone-dash-value",
      "usage/negative-explicit",
      "usage/negative-then-valid-needs",
    ]);
  });

  test("covers every workflow, event and outcome class", () => {
    expect(cases.length).toBeGreaterThan(1000);
    for (const workflow of WORKFLOWS) {
      expect(cases.some((c) => c.name.startsWith(`${workflow}/`))).toBe(true);
    }
    expect(new Set(cases.map((c) => c.name)).size).toBe(cases.length);
  });

  test("every case produces its expected exit code and stderr", () => {
    for (const testCase of cases) {
      const { argv, env } = materialize(testCase, writeEvent);
      const run = runGate(argv, env);
      expect(run.code, `${testCase.name}: ${run.stderr}`).toBe(testCase.expect.code);
      if (testCase.expect.help) {
        expect(run.stdout, testCase.name).toStartWith("usage: gate.ts ");
        expect(run.stderr, testCase.name).toBe("");
      } else if (testCase.expect.code === 0) {
        expect(run.stdout, testCase.name).toBe("gate: ok\n");
        expect(run.stderr, testCase.name).toBe("");
      } else {
        expect(run.stdout, testCase.name).toBe("");
        if (testCase.expect.stderr !== undefined) {
          expect(run.stderr, testCase.name).toBe(testCase.expect.stderr);
        } else {
          expect(run.stderr, testCase.name).toContain("gate.ts: error: ");
        }
      }
    }
  });
});

// GateSchemaTest helpers: the event that legitimately produced the plan's
// opt-ins, and the matrix the plan emits for that event.
type GateOptions = NeedsOptions & {
  tested?: string;
  needsJson?: string;
  eventName?: string | null;
  event?: unknown;
};

function gate(
  planValue: unknown,
  workflow: Workflow,
  results: Record<string, string> = {},
  options: GateOptions = {},
): GateRun {
  let { eventName, event } = options;
  if (eventName === undefined) {
    const jobs = (planValue as { jobs?: Record<string, { selected?: unknown }> } | null)?.jobs;
    const chosen = Object.fromEntries(
      Object.entries(OPT_IN_JOBS[workflow] ?? {})
        .filter(([job]) => jobs?.[job]?.selected === true)
        .map(([, input]) => [input, "true"]),
    );
    [eventName, event] =
      Object.keys(chosen).length > 0
        ? ["workflow_dispatch", { inputs: chosen }]
        : ["pull_request", {}];
  }
  const payload =
    options.needsJson ??
    needs(planValue, workflow, results, { matrixEvent: eventName ?? "pull_request", ...options });
  const env: Record<string, string> = {
    GITHUB_EVENT_PATH: writeEvent(typeof event === "string" ? event : JSON.stringify(event ?? {})),
  };
  if (eventName !== null) env.GITHUB_EVENT_NAME = eventName;
  return runGate(
    ["--workflow", workflow, "--needs-json", payload, "--tested-sha", options.tested ?? SHA_A],
    env,
  );
}

const code = (run: GateRun) => run.code;
const allSelected = (workflow: Workflow) =>
  Object.fromEntries(WORKFLOW_JOBS[workflow].map((job) => [job, true]));
const allSuccess = (workflow: Workflow) =>
  Object.fromEntries(WORKFLOW_JOBS[workflow].map((job) => [job, "success"]));
const full = { mode: "full" };

// The tables are copies until the planner module owns them; hold them to the workflows.
describe("tables match the workflows", () => {
  const repo = resolve(import.meta.dir, "../../..");
  type Doc = {
    on: Record<string, { inputs?: Record<string, { type?: string }> } | null>;
    jobs: Record<string, { needs?: string[] }>;
  };
  const load = (workflow: Workflow) =>
    Bun.YAML.parse(
      readFileSync(join(repo, ".github", "workflows", `${workflow}.yml`), "utf8"),
    ) as Doc;

  for (const workflow of WORKFLOWS) {
    test(`${workflow}: gate needs the plan and exactly the registered jobs`, () => {
      const doc = load(workflow);
      const jobs: readonly string[] = WORKFLOW_JOBS[workflow];
      const gateJob = doc.jobs[gateJobId(workflow)];
      expect(new Set(gateJob?.needs)).toEqual(new Set([PLAN_JOB_ID, ...jobs]));
      expect(new Set(Object.keys(doc.jobs))).toEqual(
        new Set([PLAN_JOB_ID, gateJobId(workflow), ...jobs]),
      );
      const inputs = doc.on.workflow_dispatch?.inputs ?? {};
      for (const input of Object.values(OPT_IN_JOBS[workflow] ?? {})) {
        expect(inputs[input]?.type, input).toBe("boolean");
      }
    });
  }
});

describe("GateSchemaTest", () => {
  test("postgres budget matrix aggregate is required by gate", () => {
    const p = plan("rust", allSelected("rust"));
    const ok = allSuccess("rust");
    expect(code(gate(p, "rust", ok))).toBe(0);
    for (const result of ["failure", "cancelled", "skipped"]) {
      expect(code(gate(p, "rust", { ...ok, postgres: result })), result).toBe(1);
    }
    expect(code(gate(p, "rust", ok, { omit: ["postgres"] }))).toBe(1);
  });

  test("web budget lanes require both selected success", () => {
    const selected = { "web-checks": true, "web-native-checks": true };
    const p = plan("web", selected);
    const ok = { "web-checks": "success", "web-native-checks": "success" };
    expect(code(gate(p, "web", ok))).toBe(0);
    for (const job of Object.keys(selected)) {
      for (const result of ["failure", "cancelled", "skipped"]) {
        expect(code(gate(p, "web", { ...ok, [job]: result })), `${job} ${result}`).toBe(1);
      }
    }
    expect(code(gate(p, "web", ok, { omit: ["web-native-checks"] }))).toBe(1);
    expect(code(gate(plan("web"), "web"))).toBe(0);
  });

  test("rust native-arm64 gate rejects incomplete results", () => {
    const p = plan("rust", allSelected("rust"));
    const ok = allSuccess("rust");
    for (const result of ["failure", "cancelled", "skipped"]) {
      expect(code(gate(p, "rust", { ...ok, "native-arm64": result })), result).toBe(1);
    }
    expect(code(gate(p, "rust", ok, { omit: ["native-arm64"] }))).toBe(1);
  });

  test("unselected must be skipped", () => {
    const run = gate(plan("web", { "web-checks": false }), "web", { "web-checks": "success" });
    expect(run.code).toBe(1);
    expect(run.stderr).toBe("gate: unselected job web-checks must be skipped, got success\n");
  });

  test("web lint job failure reaches required gate", () => {
    const p = plan("web", { "web-static": true });
    expect(code(gate(p, "web", { "web-static": "failure" }))).toBe(1);
    expect(code(gate(p, "web", { "web-static": "success" }))).toBe(0);
  });

  const docs = (selected = true) => plan("documents", { "native-extraction": selected }, full);

  test("selected missing needs key rejected", () => {
    expect(code(gate(docs(), "documents", {}, { omit: ["native-extraction"] }))).toBe(1);
  });

  test("malformed, list and missing needs rejected", () => {
    expect(gate(docs(), "documents", {}, { needsJson: "{not-json" }).stderr).toContain(
      "NEEDS_MALFORMED",
    );
    expect(gate(docs(), "documents", {}, { needsJson: "[]" }).stderr).toContain("NEEDS_TYPE");
    const missing = runGate(
      ["--workflow", "documents", "--tested-sha", SHA_A, "--needs-json", ""],
      {},
    );
    expect(missing).toEqual({ code: 1, stdout: "", stderr: "gate: needs json missing\n" });
  });

  test("missing or wrongly typed result field rejected", () => {
    for (const entry of [{ outputs: {} }, { result: 1, outputs: {} }]) {
      const run = gate(docs(), "documents", {}, { jobEntries: { "native-extraction": entry } });
      expect(run.code, JSON.stringify(entry)).toBe(1);
      expect(run.stderr).toMatch(/JOB_NEED_RESULT_(MISSING|TYPE)/);
    }
  });

  test("plan result failure rejected", () => {
    const run = gate(
      docs(),
      "documents",
      { "native-extraction": "success" },
      {
        planResult: "failure",
      },
    );
    expect(run.stderr).toBe("gate: needs error PLAN_RESULT\n");
  });

  test("extra unknown job rejected", () => {
    const run = gate(
      docs(false),
      "documents",
      {},
      {
        extra: { mystery: { result: "success", outputs: {} } },
      },
    );
    expect(run.stderr).toBe("gate: needs error NEEDS_KEY_SET\n");
  });

  test("plan not ok rejected", () => {
    const run = gate(plan("web", { "web-checks": true }, { plan_ok: false }), "web", {
      "web-checks": "success",
    });
    expect(run.stderr).toBe("gate: plan schema error PLAN_NOT_OK\n");
  });

  for (const result of ["failure", "cancelled", "skipped"]) {
    test(`selected ${result} rejected`, () => {
      const run = gate(docs(), "documents", { "native-extraction": result });
      expect(run.stderr).toBe(`gate: selected job native-extraction must succeed, got ${result}\n`);
    });
  }

  test("selected success ok", () => {
    expect(gate(docs(), "documents", { "native-extraction": "success" })).toEqual({
      code: 0,
      stdout: "gate: ok\n",
      stderr: "",
    });
  });

  test("invalid plan top type, unknown plan keys and unknown job keys rejected", () => {
    expect(gate(["not", "an", "object"], "documents").stderr).toContain("PLAN_TOP_TYPE");
    expect(gate({ ...docs(false), extra: "nope" }, "documents").stderr).toContain(
      "PLAN_UNKNOWN_KEYS",
    );
    const extraKey = docs(false);
    extraKey.jobs = { "native-extraction": { selected: false, reason_code: "NARROW_DOCS" } };
    expect(gate(extraKey, "documents").stderr).toContain("PLAN_JOB_UNKNOWN_KEYS");
  });

  test("strict bool rejects string true and integer", () => {
    for (const selected of ["true", 1]) {
      const p = { ...docs(false), jobs: { "native-extraction": { selected } } };
      const run = gate(p, "documents", { "native-extraction": "success" });
      expect(run.stderr, String(selected)).toBe("gate: plan schema error PLAN_SELECTED_TYPE\n");
    }
  });

  test("invalid plan json rejected", () => {
    const run = gate(
      docs(),
      "documents",
      { "native-extraction": "success" },
      {
        planOutputs: { plan_json: "{bad" },
      },
    );
    expect(run.stderr).toBe("gate: needs error PLAN_JSON_MALFORMED\n");
  });

  test("tested sha mismatch rejected", () => {
    const run = gate(docs(), "documents", { "native-extraction": "success" }, { tested: SHA_B });
    expect(run.stderr).toBe("gate: tested_sha mismatch\n");
  });
});

describe("AgentDocsGateTest", () => {
  const upgrade = "upgrade-smoke-arm64";

  test("opt-in selected bad result or missing rejected", () => {
    const p = plan("install", { [upgrade]: true });
    const ok = { [upgrade]: "success" };
    expect(code(gate(p, "install", ok))).toBe(0);
    for (const result of ["failure", "cancelled", "skipped"]) {
      expect(code(gate(p, "install", { [upgrade]: result })), result).toBe(1);
    }
    expect(code(gate(p, "install", ok, { omit: [upgrade] }))).toBe(1);
  });

  const unchosenEvents: [string, unknown][] = [
    ["pull_request", {}],
    ["push", {}],
    ["merge_group", {}],
    ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "false" } }],
  ];

  test("opt-in unchosen job that ran rejected", () => {
    const p = plan("install");
    for (const [eventName, event] of unchosenEvents) {
      expect(code(gate(p, "install", {}, { eventName, event })), eventName).toBe(0);
      for (const result of ["success", "failure", "cancelled"]) {
        const run = gate(p, "install", { [upgrade]: result }, { eventName, event });
        expect(run.code, `${eventName} ${result}`).toBe(1);
      }
    }
  });

  test("opt-in plan override rejected", () => {
    const forced = plan("install", { [upgrade]: true });
    for (const [eventName, event] of [
      ["pull_request", { inputs: { run_upgrade_smoke_arm: "true" } }],
      ...unchosenEvents.slice(1),
      ["workflow_dispatch", {}],
    ] as [string, unknown][]) {
      const run = gate(forced, "install", { [upgrade]: "success" }, { eventName, event });
      expect(run.stderr, eventName).toBe(`gate: opt-in error OPT_IN_MISMATCH ${upgrade}\n`);
    }
    const dropped = gate(
      plan("install"),
      "install",
      {},
      {
        eventName: "workflow_dispatch",
        event: { inputs: { run_upgrade_smoke_arm: "true" } },
      },
    );
    expect(dropped.stderr).toBe(`gate: opt-in error OPT_IN_MISMATCH ${upgrade}\n`);
  });

  test("opt-in malformed event rejected", () => {
    const p = plan("install");
    for (const [eventName, event] of [
      ["workflow_dispatch", "{bad"],
      ["workflow_dispatch", []],
      ["workflow_dispatch", { inputs: [] }],
      ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "yes" } }],
      ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: 1 } }],
      ["workflow_dispatch", { inputs: { other: "true" } }],
      ["schedule", {}],
    ] as [string, unknown][]) {
      expect(code(gate(p, "install", {}, { eventName, event })), JSON.stringify(event)).toBe(1);
    }
    const payload = needs(p, "install");
    const argv = ["--workflow", "install", "--needs-json", payload, "--tested-sha", SHA_A];
    expect(runGate(argv, { GITHUB_EVENT_PATH: writeEvent("{}") }).stderr).toBe(
      "gate: opt-in error EVENT_NAME\n",
    );
    expect(runGate(argv, { GITHUB_EVENT_NAME: "push" }).stderr).toBe(
      "gate: opt-in error EVENT_PATH_MISSING\n",
    );
    expect(
      runGate(argv, { GITHUB_EVENT_NAME: "push", GITHUB_EVENT_PATH: join(scratch, "absent") })
        .stderr,
    ).toBe("gate: opt-in error EVENT_MALFORMED\n");
  });

  test("docs plan with every job skipped passes", () => {
    for (const workflow of WORKFLOWS) {
      expect(code(gate(plan(workflow), workflow)), workflow).toBe(0);
    }
  });

  test("docs plan with an unselected job that ran is rejected", () => {
    for (const workflow of WORKFLOWS) {
      for (const job of WORKFLOW_JOBS[workflow]) {
        for (const result of ["success", "failure", "cancelled"]) {
          const run = gate(plan(workflow), workflow, { [job]: result });
          expect(run.code, `${workflow} ${job} ${result}`).toBe(1);
        }
      }
    }
  });

  test("full plan with a bad selected result is rejected", () => {
    for (const workflow of WORKFLOWS) {
      const p = plan(workflow, allSelected(workflow), {
        mode: "full",
        reason_code: "FULL_PATH_BROADEN",
      });
      const ok = allSuccess(workflow);
      expect(code(gate(p, workflow, ok)), workflow).toBe(0);
      for (const job of WORKFLOW_JOBS[workflow]) {
        for (const result of ["failure", "cancelled", "skipped"]) {
          expect(code(gate(p, workflow, { ...ok, [job]: result })), `${job} ${result}`).toBe(1);
        }
        expect(code(gate(p, workflow, ok, { omit: [job] })), job).toBe(1);
      }
    }
  });
});

describe("postgres matrix", () => {
  const rows = catalogRows();
  const fullPlan = plan("rust", fullSelection("rust"), full) as Plan;

  test("pull requests carry only PG 18 x64 rows, every other event the full catalog", () => {
    const pr = postgresMatrixInclude("pull_request", rows);
    expect(pr.length).toBeGreaterThan(0);
    expect(pr.length).toBeLessThan(rows.length);
    for (const row of pr) expect([row.runner, row.pg_major]).toEqual(["ubuntu-26.04", "18"]);
    // Every PR shard of the designated version is kept.
    const prShards = new Set(pr.map((row) => row.shard));
    const allX64Pg18 = rows.filter((r) => r.runner === "ubuntu-26.04" && r.pg_major === "18");
    expect(prShards).toEqual(new Set(allX64Pg18.map((row) => row.shard)));
    for (const event of ["push", "merge_group", "workflow_dispatch"]) {
      expect(postgresMatrixInclude(event, rows)).toEqual(rows);
    }
  });

  test("a reduced matrix on merge_group fails; the full one passes", () => {
    const check = (event: string, matrix: string) =>
      postgresMatrixGateError("rust", fullPlan, { postgres_matrix: matrix }, event, () => ({
        jobs: { postgres: { env: { FVOCI_POSTGRES_MATRIX_CATALOG: JSON.stringify(rows) } } },
      }));
    expect(check("merge_group", matrixFor("pull_request"))).toBe("POSTGRES_MATRIX_ROW_COUNT");
    expect(check("merge_group", matrixFor("merge_group"))).toBeNull();
    const dropped = JSON.stringify({ include: [...rows.slice(1), rows[0]].slice(0, -1) });
    expect(check("merge_group", dropped)).toBe("POSTGRES_MATRIX_ROW_COUNT");
  });

  test("an unusable catalog fails closed without a fallback", () => {
    const matrix = { postgres_matrix: matrixFor("pull_request") };
    for (const data of [
      undefined,
      [],
      { jobs: [] },
      { jobs: {} },
      { jobs: { postgres: { env: {} } } },
      { jobs: { postgres: { env: { FVOCI_POSTGRES_MATRIX_CATALOG: " " } } } },
      { jobs: { postgres: { env: { FVOCI_POSTGRES_MATRIX_CATALOG: "[" } } } },
      { jobs: { postgres: { env: { FVOCI_POSTGRES_MATRIX_CATALOG: "[]" } } } },
      { jobs: { postgres: { env: { FVOCI_POSTGRES_MATRIX_CATALOG: '["x"]' } } } },
    ]) {
      const error = postgresMatrixGateError("rust", fullPlan, matrix, "pull_request", () => data);
      expect(error, JSON.stringify(data)).toBe("POSTGRES_MATRIX_CATALOG");
    }
  });

  test("the catalog is read only after the output and event are valid", () => {
    let reads = 0;
    const load = () => {
      reads++;
      return undefined;
    };
    expect(postgresMatrixGateError("rust", fullPlan, {}, "pull_request", load)).toBe(
      "POSTGRES_MATRIX_MISSING",
    );
    expect(
      postgresMatrixGateError("rust", fullPlan, { postgres_matrix: "{}" }, "schedule", load),
    ).toBe("POSTGRES_MATRIX_EMPTY");
    const unselected = plan("rust") as Plan;
    expect(postgresMatrixGateError("rust", unselected, {}, "pull_request", load)).toBeNull();
    expect(reads).toBe(0);
  });
});

describe("helpers", () => {
  test("dispatch opt-ins", () => {
    expect([
      ...expectChosen(
        dispatchOptIns("install", "workflow_dispatch", { inputs: { run_upgrade_smoke_arm: true } }),
      ),
    ]).toEqual(["run_upgrade_smoke_arm"]);
    expect(
      dispatchOptIns("web", "workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "true" } }),
    ).toEqual({ ok: false, error: "DISPATCH_INPUTS_UNKNOWN" });
    expect([
      ...expectChosen(
        dispatchOptIns("install", "push", { inputs: { run_upgrade_smoke_arm: "true" } }),
      ),
    ]).toEqual([]);
  });

  test("blank uses the planner's whitespace set", () => {
    expect(isBlank("\x1c\x1d\x1e\x1f\x85 　")).toBe(true);
    expect(isBlank("﻿")).toBe(false);
    expect(strip(" pull_request\n")).toBe("pull_request");
  });

  test("honest results mirror the selection", () => {
    expect(honestResults({ a: true, b: false })).toEqual({ a: "success", b: "skipped" });
    expect(RESULTS).toEqual(["success", "failure", "cancelled", "skipped"]);
  });
});

function expectChosen(result: ReturnType<typeof dispatchOptIns>): ReadonlySet<string> {
  if (!result.ok) throw new Error(result.error);
  return result.chosen;
}

describe("CLI", () => {
  const cli = resolve(import.meta.dir, "../gate.ts");
  const spawn = (args: string[], env: Record<string, string>) => {
    const proc = Bun.spawnSync([process.execPath, cli, ...args], {
      env: { PATH: process.env.PATH ?? "", ...env },
      stdout: "pipe",
      stderr: "pipe",
    });
    return { code: proc.exitCode, stdout: proc.stdout.toString(), stderr: proc.stderr.toString() };
  };
  const docsNeeds = needs(plan("documents"), "documents");

  test("pass, fail and usage error keep their streams and exit codes", () => {
    const argv = ["--workflow", "documents", "--tested-sha", SHA_A];
    expect(spawn([...argv, "--needs-json", docsNeeds], {})).toEqual({
      code: 0,
      stdout: "gate: ok\n",
      stderr: "",
    });
    expect(spawn(argv, { NEEDS_JSON: docsNeeds }).code).toBe(0);
    expect(spawn([...argv, "--needs-json", "{"], {})).toEqual({
      code: 1,
      stdout: "",
      stderr: "gate: needs error NEEDS_MALFORMED\n",
    });
    const usage = spawn(["--workflow", "release", "--tested-sha", SHA_A], {});
    expect(usage.code).toBe(2);
    expect(usage.stderr).toContain("invalid choice: 'release'");
  });

  test("the rust gate reads the checked-out rust.yml catalog", () => {
    const p = plan("rust", fullSelection("rust"), full);
    const results = honestResults(fullSelection("rust"));
    for (const event of ["pull_request", "merge_group"]) {
      const run = spawn(
        [
          "--workflow",
          "rust",
          "--tested-sha",
          SHA_A,
          "--needs-json",
          needs(p, "rust", results, { matrixEvent: event }),
        ],
        { GITHUB_EVENT_NAME: event },
      );
      expect(run, event).toEqual({ code: 0, stdout: "gate: ok\n", stderr: "" });
    }
    const reduced = spawn(
      [
        "--workflow",
        "rust",
        "--tested-sha",
        SHA_A,
        "--needs-json",
        needs(p, "rust", results, { matrixEvent: "pull_request" }),
      ],
      { GITHUB_EVENT_NAME: "merge_group" },
    );
    expect(reduced.stderr).toBe("gate: postgres matrix error POSTGRES_MATRIX_ROW_COUNT\n");
  });
});
