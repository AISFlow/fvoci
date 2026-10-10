// Gate input corpus: workflow x event x outcome class, plus schema, needs,
// matrix, opt-in and argument edge cases. Each case carries the expected exit
// code and, except for usage errors, the exact stderr line.

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { catalogFromRustWorkflow, postgresMatrixInclude, RUST_WORKFLOW_FILE } from "./matrix.ts";
import {
  KNOWN_EVENTS,
  OPT_IN_JOBS,
  PLAN_JOB_ID,
  PLAN_VERSION,
  WORKFLOW_JOBS,
  WORKFLOWS,
  type JsonRecord,
  type Workflow,
} from "./schema.ts";

export const SHA_A = "a".repeat(40);
export const SHA_B = "b".repeat(40);
export const RESULTS = ["success", "failure", "cancelled", "skipped"] as const;
export const EVENTS = ["pull_request", "push", "merge_group", "workflow_dispatch", "schedule"];

export type GateCase = {
  name: string;
  argv: string[];
  /** GITHUB_EVENT_NAME; null leaves it unset. */
  eventName: string | null;
  /** Event file contents; null leaves GITHUB_EVENT_PATH unset. */
  event: string | Uint8Array | null;
  /** NEEDS_JSON environment value, when set. */
  needsEnv?: string;
  /** Exit code, and the exact stderr unless the case is a usage error. */
  expect: { code: number; stderr?: string; help?: true };
  /** Why the Python planner reports this case differently, when it does. */
  knownDifference?: string;
};

const root = resolve(import.meta.dir, "../../..");

let catalog: JsonRecord[] | undefined;

export function catalogRows(): JsonRecord[] {
  if (catalog) return catalog;
  const text = readFileSync(join(root, ".github", "workflows", RUST_WORKFLOW_FILE), "utf8");
  const rows = catalogFromRustWorkflow(Bun.YAML.parse(text));
  if (rows === null) throw new Error("rust.yml postgres matrix catalog is unusable");
  catalog = rows;
  return rows;
}

/** The postgres_matrix output the plan emits for this event. */
export function matrixFor(eventName: string): string {
  return JSON.stringify({ include: postgresMatrixInclude(eventName, catalogRows()) });
}

export function plan(
  workflow: Workflow,
  selected: Readonly<Record<string, boolean>> = {},
  overrides: JsonRecord = {},
): JsonRecord {
  const jobs = Object.fromEntries(
    WORKFLOW_JOBS[workflow].map((job) => [job, { selected: selected[job] ?? false }]),
  );
  return {
    version: PLAN_VERSION,
    workflow,
    mode: "narrow",
    reason_code: "NARROW_DOCS",
    plan_ok: true,
    tested_sha: SHA_A,
    jobs,
    ...overrides,
  };
}

/** Every job selected except opt-in jobs, which only a dispatch input selects. */
export function fullSelection(workflow: Workflow, optIn = false): Record<string, boolean> {
  const optIns = new Set(Object.keys(OPT_IN_JOBS[workflow] ?? {}));
  return Object.fromEntries(WORKFLOW_JOBS[workflow].map((job) => [job, optIn || !optIns.has(job)]));
}

export type NeedsOptions = {
  planResult?: string;
  /** Replaces the plan job outputs entirely. */
  planOutputs?: unknown;
  /** Matrix output added for a selected rust postgres; defaults to the event's matrix. */
  matrix?: string | null;
  matrixEvent?: string;
  extra?: JsonRecord;
  omit?: readonly string[];
  jobEntries?: JsonRecord;
};

export function needs(
  planValue: unknown,
  workflow: Workflow,
  results: Readonly<Record<string, string>> = {},
  options: NeedsOptions = {},
): string {
  const out: JsonRecord = {};
  if (options.planOutputs !== undefined) {
    out[PLAN_JOB_ID] = { result: options.planResult ?? "success", outputs: options.planOutputs };
  } else {
    const outputs: Record<string, string> = { plan_json: JSON.stringify(planValue) };
    const jobs = (planValue as { jobs?: Record<string, { selected?: unknown }> } | null)?.jobs;
    if (workflow === "rust" && jobs?.postgres?.selected === true) {
      const matrix =
        options.matrix === undefined
          ? matrixFor(options.matrixEvent ?? "pull_request")
          : options.matrix;
      if (matrix !== null) outputs.postgres_matrix = matrix;
    }
    out[PLAN_JOB_ID] = { result: options.planResult ?? "success", outputs };
  }
  for (const job of WORKFLOW_JOBS[workflow]) {
    if (options.omit?.includes(job)) continue;
    if (options.jobEntries && Object.hasOwn(options.jobEntries, job)) {
      out[job] = options.jobEntries[job];
      continue;
    }
    out[job] = { result: results[job] ?? "skipped", outputs: {} };
  }
  return JSON.stringify({ ...out, ...options.extra });
}

export function honestResults(selected: Readonly<Record<string, boolean>>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(selected).map(([job, on]) => [job, on ? "success" : "skipped"]),
  );
}

function gateArgv(workflow: string, needsJson: string | null, tested = SHA_A): string[] {
  const argv = ["--workflow", workflow];
  if (needsJson !== null) argv.push("--needs-json", needsJson);
  return [...argv, "--tested-sha", tested];
}

function ok(): GateCase["expect"] {
  return { code: 0 };
}
function fail(stderr: string): GateCase["expect"] {
  return { code: 1, stderr: `gate: ${stderr}\n` };
}

/** The event payload a run with these opt-ins carries. */
function eventPayload(eventName: string, optIn: boolean): string {
  if (eventName === "workflow_dispatch") {
    return JSON.stringify({ inputs: optIn ? { run_upgrade_smoke_arm: "true" } : {} });
  }
  return "{}";
}

// What a gate with otherwise-consistent selection reports for this event:
// install re-derives opt-ins first, rust then checks the matrix event.
function eventVerdict(
  workflow: Workflow,
  eventName: string,
  jobError: string | null,
  postgresSelected: boolean,
): GateCase["expect"] {
  const known = KNOWN_EVENTS.has(eventName);
  if (workflow === "install" && !known) return fail("opt-in error EVENT_NAME");
  if (jobError) return fail(jobError);
  if (workflow === "rust" && postgresSelected && !known) {
    return fail("postgres matrix error POSTGRES_MATRIX_EVENT");
  }
  return ok();
}

function selectionMatrix(): GateCase[] {
  const cases: GateCase[] = [];
  for (const workflow of WORKFLOWS) {
    for (const eventName of EVENTS) {
      const shapes: [string, Record<string, boolean>, boolean][] = [
        ["narrow", {}, false],
        ["full", fullSelection(workflow), false],
      ];
      if (workflow === "install" && eventName === "workflow_dispatch") {
        shapes.push(["full+opt-in", fullSelection(workflow, true), true]);
      }
      for (const [shape, selected, optIn] of shapes) {
        const p = plan(
          workflow,
          selected,
          shape === "narrow" ? {} : { mode: "full", reason_code: "FULL_PATH_BROADEN" },
        );
        const honest = honestResults(
          Object.fromEntries(WORKFLOW_JOBS[workflow].map((job) => [job, selected[job] ?? false])),
        );
        const postgres = selected.postgres === true;
        const base = { eventName, event: eventPayload(eventName, optIn) };
        const make = (label: string, needsJson: string, expect: GateCase["expect"]): GateCase => ({
          name: `${workflow}/${eventName}/${shape}/${label}`,
          argv: gateArgv(workflow, needsJson),
          ...base,
          expect,
        });
        const withEvent = { matrixEvent: eventName === "schedule" ? "push" : eventName };
        cases.push(
          make(
            "honest",
            needs(p, workflow, honest, withEvent),
            eventVerdict(workflow, eventName, null, postgres),
          ),
        );
        for (const job of WORKFLOW_JOBS[workflow]) {
          const on = selected[job] ?? false;
          for (const result of RESULTS) {
            if (result === (on ? "success" : "skipped")) continue;
            const error = on
              ? `selected job ${job} must succeed, got ${result}`
              : `unselected job ${job} must be skipped, got ${result}`;
            cases.push(
              make(
                `${job}=${result}`,
                needs(p, workflow, { ...honest, [job]: result }, withEvent),
                eventVerdict(workflow, eventName, error, postgres),
              ),
            );
          }
          cases.push(
            make(
              `${job}=missing`,
              needs(p, workflow, honest, { ...withEvent, omit: [job] }),
              fail("needs error NEEDS_KEY_SET"),
            ),
          );
        }
      }
    }
  }
  return cases;
}

// Cases where the needs or plan are broken. They use the documents workflow,
// whose gate reads no event file and no matrix.
function needsAndSchema(): GateCase[] {
  const wf: Workflow = "documents";
  const sel = { "native-extraction": true };
  const p = plan(wf, sel, { mode: "full" });
  const res = { "native-extraction": "success" };
  const base = { eventName: "pull_request", event: "{}" };
  const c = (
    name: string,
    argv: string[],
    expect: GateCase["expect"],
    extra: Partial<GateCase> = {},
  ): GateCase => ({
    name: `needs/${name}`,
    argv,
    ...base,
    expect,
    ...extra,
  });
  const n = (opts: NeedsOptions = {}, planValue: unknown = p, results = res) =>
    needs(planValue, wf, results, opts);
  const schema = (name: string, planValue: unknown, code: string, results = res): GateCase =>
    c(`schema-${name}`, gateArgv(wf, n({}, planValue, results)), fail(`plan schema error ${code}`));
  const jobs = (entry: unknown) => ({ jobs: { "native-extraction": entry } });
  const cases: GateCase[] = [
    c("ok", gateArgv(wf, n()), ok()),
    c("malformed", gateArgv(wf, "{not-json"), fail("needs error NEEDS_MALFORMED")),
    c("list", gateArgv(wf, "[]"), fail("needs error NEEDS_TYPE")),
    c("null", gateArgv(wf, "null"), fail("needs error NEEDS_TYPE")),
    c("number", gateArgv(wf, "3"), fail("needs error NEEDS_TYPE")),
    c("empty", gateArgv(wf, ""), fail("needs json missing")),
    c("blank", gateArgv(wf, " \n\t"), fail("needs json missing")),
    c("python-space-blank", gateArgv(wf, "\x1c\x85"), fail("needs json missing")),
    c("absent", gateArgv(wf, null), fail("needs json missing")),
    c("env-fallback", gateArgv(wf, null), ok(), { needsEnv: n() }),
    c("flag-over-env", gateArgv(wf, n()), ok(), { needsEnv: "{bad" }),
    c("empty-flag-over-env", gateArgv(wf, ""), fail("needs json missing"), { needsEnv: n() }),
    c(
      "extra-job",
      gateArgv(wf, n({ extra: { mystery: { result: "success" } } })),
      fail("needs error NEEDS_KEY_SET"),
    ),
    c(
      "proto-key",
      gateArgv(wf, n().replace(/^\{/, '{"__proto__":{"result":"success"},')),
      fail("needs error NEEDS_KEY_SET"),
    ),
    c(
      "constructor-key",
      gateArgv(wf, n({ extra: { constructor: { result: "success" } } })),
      fail("needs error NEEDS_KEY_SET"),
    ),
    c(
      "no-plan-job",
      gateArgv(wf, JSON.stringify({ "native-extraction": { result: "success" } })),
      fail("needs error NEEDS_KEY_SET"),
    ),
    c(
      "plan-entry-list",
      gateArgv(
        wf,
        JSON.stringify({ [PLAN_JOB_ID]: [], "native-extraction": { result: "success" } }),
      ),
      fail("needs error PLAN_NEED_ENTRY_TYPE"),
    ),
    c(
      "plan-result-missing",
      gateArgv(
        wf,
        JSON.stringify({
          [PLAN_JOB_ID]: { outputs: {} },
          "native-extraction": { result: "success" },
        }),
      ),
      fail("needs error PLAN_NEED_RESULT_MISSING"),
    ),
    c(
      "plan-result-type",
      gateArgv(wf, n({ planResult: 1 as unknown as string })),
      fail("needs error PLAN_NEED_RESULT_TYPE"),
    ),
    c(
      "plan-result-invalid",
      gateArgv(wf, n({ planResult: "neutral" })),
      fail("needs error PLAN_NEED_RESULT_INVALID"),
    ),
    c(
      "plan-result-case",
      gateArgv(wf, n({ planResult: "Success" })),
      fail("needs error PLAN_NEED_RESULT_INVALID"),
    ),
    ...(["failure", "cancelled", "skipped"] as const).map((r) =>
      c(`plan-result-${r}`, gateArgv(wf, n({ planResult: r })), fail("needs error PLAN_RESULT")),
    ),
    c(
      "plan-outputs-list",
      gateArgv(wf, n({ planOutputs: [] })),
      fail("needs error PLAN_NEED_OUTPUTS_TYPE"),
    ),
    c(
      "plan-outputs-string",
      gateArgv(wf, n({ planOutputs: "x" })),
      fail("needs error PLAN_NEED_OUTPUTS_TYPE"),
    ),
    c(
      "plan-outputs-value-type",
      gateArgv(wf, n({ planOutputs: { plan_json: JSON.stringify(p), other: 1 } })),
      fail("needs error PLAN_NEED_OUTPUTS_TYPE"),
    ),
    c(
      "plan-outputs-null",
      gateArgv(wf, n({ planOutputs: null })),
      fail("needs error PLAN_JSON_MISSING"),
    ),
    c(
      "plan-outputs-absent",
      gateArgv(
        wf,
        JSON.stringify({
          [PLAN_JOB_ID]: { result: "success" },
          "native-extraction": { result: "success" },
        }),
      ),
      fail("needs error PLAN_JSON_MISSING"),
    ),
    c(
      "plan-json-missing",
      gateArgv(wf, n({ planOutputs: {} })),
      fail("needs error PLAN_JSON_MISSING"),
    ),
    c(
      "plan-json-blank",
      gateArgv(wf, n({ planOutputs: { plan_json: " " } })),
      fail("needs error PLAN_JSON_MISSING"),
    ),
    c(
      "plan-json-python-blank",
      gateArgv(wf, n({ planOutputs: { plan_json: "\x1f" } })),
      fail("needs error PLAN_JSON_MISSING"),
    ),
    c(
      "plan-json-malformed",
      gateArgv(wf, n({ planOutputs: { plan_json: "{bad" } })),
      fail("needs error PLAN_JSON_MALFORMED"),
    ),
    c(
      "plan-json-infinity",
      gateArgv(
        wf,
        n({
          planOutputs: {
            plan_json: JSON.stringify(p).replace('"version":3', '"version":Infinity'),
          },
        }),
      ),
      fail("needs error PLAN_JSON_MALFORMED"),
      { knownDifference: "planner: json.loads accepts Infinity and reports PLAN_VERSION" },
    ),
    c(
      "job-entry-type",
      gateArgv(wf, n({ jobEntries: { "native-extraction": "success" } })),
      fail("needs error JOB_NEED_ENTRY_TYPE"),
    ),
    c(
      "job-result-missing",
      gateArgv(wf, n({ jobEntries: { "native-extraction": { outputs: {} } } })),
      fail("needs error JOB_NEED_RESULT_MISSING"),
    ),
    c(
      "job-result-type",
      gateArgv(wf, n({ jobEntries: { "native-extraction": { result: 1 } } })),
      fail("needs error JOB_NEED_RESULT_TYPE"),
    ),
    c(
      "job-result-null",
      gateArgv(wf, n({ jobEntries: { "native-extraction": { result: null } } })),
      fail("needs error JOB_NEED_RESULT_TYPE"),
    ),
    c(
      "job-result-invalid",
      gateArgv(wf, n({ jobEntries: { "native-extraction": { result: "neutral" } } })),
      fail("needs error JOB_NEED_RESULT_INVALID"),
    ),
    c(
      "job-outputs-not-checked",
      gateArgv(wf, n({ jobEntries: { "native-extraction": { result: "success", outputs: 7 } } })),
      ok(),
    ),
    // First error wins.
    c(
      "extra-key-before-plan-failure",
      gateArgv(wf, n({ planResult: "failure", extra: { x: {} } })),
      fail("needs error NEEDS_KEY_SET"),
    ),
    c(
      "plan-not-ok-before-tested-mismatch",
      gateArgv(wf, n({}, { ...p, plan_ok: false, tested_sha: SHA_B })),
      fail("plan schema error PLAN_NOT_OK"),
    ),
    c(
      "plan-failure-before-job-error",
      gateArgv(wf, n({ planResult: "failure", jobEntries: { "native-extraction": 5 } })),
      fail("needs error PLAN_RESULT"),
    ),
    c(
      "tested-mismatch-before-results",
      gateArgv(wf, n({}, p, { "native-extraction": "failure" }), SHA_B),
      fail("tested_sha mismatch"),
    ),
    c("tested-mismatch", gateArgv(wf, n(), SHA_B), fail("tested_sha mismatch")),
    c("tested-invalid-upper", gateArgv(wf, n(), "A".repeat(40)), fail("tested-sha invalid")),
    c("tested-invalid-short", gateArgv(wf, n(), "a".repeat(39)), fail("tested-sha invalid")),
    c("tested-invalid-empty", gateArgv(wf, n(), ""), fail("tested-sha invalid")),
    c("tested-invalid-before-needs", gateArgv(wf, "{bad", "x"), fail("tested-sha invalid")),
    schema("top-list", ["not", "an", "object"], "PLAN_TOP_TYPE"),
    {
      ...schema("top-null", null, "PLAN_TOP_TYPE"),
      knownDifference: "planner: plan_json null trips an assert (traceback, exit 1)",
    },
    schema("unknown-key", { ...p, extra: "nope" }, "PLAN_UNKNOWN_KEYS"),
    schema("version-2", { ...p, version: 2 }, "PLAN_VERSION"),
    schema("version-string", { ...p, version: "3" }, "PLAN_VERSION"),
    schema("version-missing", { ...p, version: undefined }, "PLAN_VERSION"),
    schema("workflow", { ...p, workflow: "web" }, "PLAN_WORKFLOW"),
    schema("mode", { ...p, mode: "partial" }, "PLAN_MODE"),
    schema("reason-lower", { ...p, reason_code: "narrow_docs" }, "PLAN_REASON_CODE"),
    schema("reason-long", { ...p, reason_code: `A${"B".repeat(64)}` }, "PLAN_REASON_CODE"),
    schema("reason-type", { ...p, reason_code: 7 }, "PLAN_REASON_CODE"),
    schema("plan-ok-string", { ...p, plan_ok: "true" }, "PLAN_OK_TYPE"),
    schema("plan-ok-int", { ...p, plan_ok: 1 }, "PLAN_OK_TYPE"),
    schema("plan-not-ok", { ...p, plan_ok: false }, "PLAN_NOT_OK"),
    schema("tested-null", { ...p, tested_sha: null }, "PLAN_TESTED_SHA"),
    schema("tested-short", { ...p, tested_sha: "abc" }, "PLAN_TESTED_SHA"),
    schema("jobs-list", { ...p, jobs: [] }, "PLAN_JOBS"),
    schema(
      "jobs-extra",
      { ...p, jobs: { "native-extraction": { selected: true }, x: { selected: false } } },
      "PLAN_JOB_SET",
    ),
    schema("jobs-empty", { ...p, jobs: {} }, "PLAN_JOB_SET"),
    schema("job-entry-bool", { ...p, ...jobs(true) }, "PLAN_JOB_MISSING"),
    schema(
      "job-unknown-key",
      { ...p, ...jobs({ selected: true, reason_code: "NARROW_DOCS" }) },
      "PLAN_JOB_UNKNOWN_KEYS",
    ),
    schema("selected-string", { ...p, ...jobs({ selected: "true" }) }, "PLAN_SELECTED_TYPE"),
    schema("selected-int", { ...p, ...jobs({ selected: 1 }) }, "PLAN_SELECTED_TYPE"),
    schema("selected-missing", { ...p, ...jobs({}) }, "PLAN_SELECTED_TYPE"),
    schema(
      "path-count-and-shas-allowed-but-not-ok",
      {
        ...p,
        base_sha: null,
        head_sha: SHA_B,
        merge_base_sha: null,
        path_count: 3,
        plan_ok: false,
      },
      "PLAN_NOT_OK",
    ),
    c(
      "full-plan-keys-ok",
      gateArgv(
        wf,
        n({}, { ...p, base_sha: null, head_sha: SHA_B, merge_base_sha: SHA_B, path_count: 0 }),
      ),
      ok(),
    ),
    c(
      "version-float-ok",
      gateArgv(
        wf,
        n({
          planOutputs: { plan_json: JSON.stringify(p).replace('"version":3', '"version":3.0') },
        }),
      ),
      ok(),
    ),
    c(
      "nan-in-needs",
      gateArgv(wf, n().replace('"result":"success"', '"result":NaN')),
      fail("needs error NEEDS_MALFORMED"),
      {
        knownDifference: "planner: json.loads accepts NaN and reports PLAN_NEED_RESULT_TYPE",
      },
    ),
    c(
      "reason-code-trailing-newline",
      gateArgv(wf, n({}, { ...p, reason_code: "FULL_PATH_BROADEN\n" })),
      fail("plan schema error PLAN_REASON_CODE"),
      {
        knownDifference: "planner: Python's $ matches before a final newline and accepts the code",
      },
    ),
    c(
      "tested-sha-trailing-newline",
      gateArgv(wf, n({}, { ...p, tested_sha: `${SHA_A}\n` }), `${SHA_A}\n`),
      fail("tested-sha invalid"),
      {
        knownDifference: "planner: Python's $ matches before a final newline and accepts the SHA",
      },
    ),
  ];
  // Producer and consumer jobs are selected together.
  const rust = (selected: Record<string, boolean>, name: string): GateCase => ({
    name: `needs/schema-rust-${name}`,
    argv: gateArgv("rust", needs(plan("rust", selected), "rust", honestResults(selected))),
    ...base,
    expect: fail("plan schema error PLAN_BINARY_PRODUCER_SELECTION"),
  });
  cases.push(
    rust({ postgres: true }, "postgres-without-build"),
    rust({ "postgres-build": true }, "build-without-postgres"),
    rust({ collaboration: true }, "collaboration-without-build"),
    {
      name: "needs/schema-web-browser-producer",
      argv: gateArgv(
        "web",
        needs(plan("web", { "workspace-browser-shard": true }), "web", {
          "workspace-browser-shard": "success",
        }),
      ),
      ...base,
      expect: fail("plan schema error PLAN_BROWSER_PRODUCER_SELECTION"),
    },
    {
      name: "needs/web-budget-lanes-ok",
      argv: gateArgv(
        "web",
        needs(plan("web", { "web-checks": true, "web-native-checks": true }), "web", {
          "web-checks": "success",
          "web-native-checks": "success",
        }),
      ),
      ...base,
      expect: ok(),
    },
  );
  return cases;
}

function matrixCases(): GateCase[] {
  const sel = fullSelection("rust");
  const p = plan("rust", sel, { mode: "full", reason_code: "FULL_PATH_BROADEN" });
  const res = honestResults(sel);
  const rows = catalogRows();
  const pr = postgresMatrixInclude("pull_request", rows);
  const prMatrix = JSON.stringify({ include: pr });
  const fullMatrix = JSON.stringify({ include: rows });
  const pick = (runner: string, major: string) =>
    rows.find((row) => row.runner === runner && row.pg_major === major) as JsonRecord;
  const swapped = (row: JsonRecord) => JSON.stringify({ include: [row, ...pr.slice(1)] });
  const reversedKeys = (list: JsonRecord[]) =>
    JSON.stringify({
      include: list.map((row) => Object.fromEntries(Object.entries(row).reverse())),
    });
  const c = (
    name: string,
    eventName: string | null,
    matrix: string | null,
    expect: GateCase["expect"],
    opts: NeedsOptions = {},
  ): GateCase => ({
    name: `matrix/${name}`,
    argv: gateArgv("rust", needs(p, "rust", res, { matrix, ...opts })),
    eventName,
    event: "{}",
    expect,
  });
  const err = (code: string) => fail(`postgres matrix error ${code}`);
  const cases: GateCase[] = [
    c("missing", "pull_request", null, err("POSTGRES_MATRIX_MISSING")),
    c("blank", "pull_request", "", err("POSTGRES_MATRIX_MISSING")),
    c("space", "pull_request", "  ", err("POSTGRES_MATRIX_MISSING")),
    c("malformed", "pull_request", "{", err("POSTGRES_MATRIX_MALFORMED")),
    c("array", "pull_request", "[]", err("POSTGRES_MATRIX_EMPTY")),
    c("object", "pull_request", "{}", err("POSTGRES_MATRIX_EMPTY")),
    c("null", "pull_request", "null", err("POSTGRES_MATRIX_EMPTY")),
    c("empty-include", "pull_request", '{"include":[]}', err("POSTGRES_MATRIX_EMPTY")),
    c("empty-row", "pull_request", '{"include":[{}]}', err("POSTGRES_MATRIX_EMPTY")),
    c("non-object-row", "pull_request", '{"include":["x"]}', err("POSTGRES_MATRIX_EMPTY")),
    c("include-object", "pull_request", '{"include":{"a":1}}', err("POSTGRES_MATRIX_EMPTY")),
    c(
      "extra-key",
      "pull_request",
      JSON.stringify({ include: pr, exclude: [] }),
      err("POSTGRES_MATRIX_EMPTY"),
    ),
    c("pr-gets-full", "pull_request", fullMatrix, err("POSTGRES_MATRIX_ROW_COUNT")),
    c(
      "pr-one-row",
      "pull_request",
      JSON.stringify({ include: pr.slice(0, 1) }),
      err("POSTGRES_MATRIX_ROW_COUNT"),
    ),
    c(
      "pr-pg17-swapped",
      "pull_request",
      swapped(pick("ubuntu-26.04", "17")),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c(
      "pr-arm64-swapped",
      "pull_request",
      swapped(pick("ubuntu-26.04-arm", "18")),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c(
      "pr-edited-image",
      "pull_request",
      JSON.stringify({
        include: pr.map((row, i) => (i ? row : { ...row, postgres_image: "postgres:18" })),
      }),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c(
      "pr-extra-row-key",
      "pull_request",
      JSON.stringify({ include: pr.map((row, i) => (i ? row : { ...row, extra: "x" })) }),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c(
      "pr-numeric-major",
      "pull_request",
      JSON.stringify({ include: pr.map((row) => ({ ...row, pg_major: 18 })) }),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c(
      "pr-reordered-rows",
      "pull_request",
      JSON.stringify({ include: [...pr].reverse() }),
      pr.length > 1 ? err("POSTGRES_MATRIX_ROWS") : ok(),
    ),
    c("pr-reordered-keys", "pull_request", reversedKeys(pr), ok()),
    c(
      "merge-group-duplicated-row",
      "merge_group",
      JSON.stringify({ include: [...rows.slice(1), rows[1]] }),
      err("POSTGRES_MATRIX_ROWS"),
    ),
    c("schedule", "schedule", fullMatrix, err("POSTGRES_MATRIX_EVENT")),
    c("empty-event-name", "", prMatrix, err("POSTGRES_MATRIX_EVENT")),
    c("unset-event-name", null, prMatrix, err("POSTGRES_MATRIX_EVENT")),
    c("padded-event-name", " pull_request\n", prMatrix, ok()),
    c("empty-before-event", "schedule", "[]", err("POSTGRES_MATRIX_EMPTY")),
    {
      name: "matrix/unselected-postgres-needs-no-matrix",
      argv: gateArgv(
        "rust",
        needs(
          plan("rust", { fast: true }),
          "rust",
          { fast: "success" },
          {
            planOutputs: {
              plan_json: JSON.stringify(plan("rust", { fast: true })),
              postgres_matrix: "[]",
            },
          },
        ),
      ),
      eventName: "pull_request",
      event: "{}",
      expect: ok(),
    },
  ];
  for (const eventName of ["merge_group", "push", "workflow_dispatch"]) {
    cases.push(
      c(`${eventName}-gets-reduced`, eventName, prMatrix, err("POSTGRES_MATRIX_ROW_COUNT")),
      c(`${eventName}-full`, eventName, fullMatrix, ok()),
      c(`${eventName}-reordered-keys`, eventName, reversedKeys(rows), ok()),
    );
  }
  cases.push(c("pull_request-reduced", "pull_request", prMatrix, ok()));
  return cases;
}

function optInCases(): GateCase[] {
  const wf: Workflow = "install";
  const c = (
    name: string,
    selected: Record<string, boolean>,
    results: Record<string, string>,
    eventName: string | null,
    event: GateCase["event"],
    expect: GateCase["expect"],
    omit: string[] = [],
  ): GateCase => ({
    name: `opt-in/${name}`,
    argv: gateArgv(wf, needs(plan(wf, selected), wf, results, { omit })),
    eventName,
    event,
    expect,
  });
  const up = { "upgrade-smoke-arm64": true };
  const chosen = JSON.stringify({ inputs: { run_upgrade_smoke_arm: "true" } });
  const err = (code: string) => fail(`opt-in error ${code}`);
  const cases: GateCase[] = [
    c(
      "chosen-success",
      up,
      { "upgrade-smoke-arm64": "success" },
      "workflow_dispatch",
      chosen,
      ok(),
    ),
    c(
      "chosen-bool-true",
      up,
      { "upgrade-smoke-arm64": "success" },
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: true } }),
      ok(),
    ),
    ...(["failure", "cancelled", "skipped"] as const).map((r) =>
      c(
        `chosen-${r}`,
        up,
        { "upgrade-smoke-arm64": r },
        "workflow_dispatch",
        chosen,
        fail(`selected job upgrade-smoke-arm64 must succeed, got ${r}`),
      ),
    ),
    c("chosen-missing", up, {}, "workflow_dispatch", chosen, fail("needs error NEEDS_KEY_SET"), [
      "upgrade-smoke-arm64",
    ]),
    c(
      "dropped-opt-in",
      {},
      {},
      "workflow_dispatch",
      chosen,
      err("OPT_IN_MISMATCH upgrade-smoke-arm64"),
    ),
    c("missing-event-name", {}, {}, null, "{}", err("EVENT_NAME")),
    c("unknown-event-name", {}, {}, "schedule", "{}", err("EVENT_NAME")),
    c("missing-event-path", {}, {}, "push", null, err("EVENT_PATH_MISSING")),
    c("malformed-event", {}, {}, "workflow_dispatch", "{bad", err("EVENT_MALFORMED")),
    c("bom-event", {}, {}, "push", "﻿{}", err("EVENT_MALFORMED")),
    c(
      "invalid-utf8-event",
      {},
      {},
      "push",
      new Uint8Array([0x7b, 0xff, 0x7d]),
      err("EVENT_MALFORMED"),
    ),
    c("event-list", {}, {}, "workflow_dispatch", "[]", err("DISPATCH_EVENT_INVALID")),
    c("event-list-on-push", {}, {}, "push", "[]", ok()),
    c(
      "inputs-list",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: [] }),
      err("DISPATCH_INPUTS_INVALID"),
    ),
    c("inputs-null", {}, {}, "workflow_dispatch", JSON.stringify({ inputs: null }), ok()),
    c("inputs-absent", {}, {}, "workflow_dispatch", "{}", ok()),
    c(
      "inputs-yes",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: "yes" } }),
      err("DISPATCH_INPUT_VALUE_INVALID"),
    ),
    c(
      "inputs-upper",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: "TRUE" } }),
      err("DISPATCH_INPUT_VALUE_INVALID"),
    ),
    c(
      "inputs-int",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: 1 } }),
      err("DISPATCH_INPUT_VALUE_INVALID"),
    ),
    c(
      "inputs-null-value",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: null } }),
      err("DISPATCH_INPUT_VALUE_INVALID"),
    ),
    c(
      "inputs-unknown",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { other: "true" } }),
      err("DISPATCH_INPUTS_UNKNOWN"),
    ),
    c(
      "inputs-false",
      {},
      {},
      "workflow_dispatch",
      JSON.stringify({ inputs: { run_upgrade_smoke_arm: false } }),
      ok(),
    ),
    c(
      "opt-in-before-results",
      {},
      { "install-smoke": "failure" },
      "schedule",
      "{}",
      err("EVENT_NAME"),
    ),
  ];
  for (const [eventName, event] of [
    ["pull_request", chosen],
    ["push", "{}"],
    ["merge_group", "{}"],
    ["workflow_dispatch", JSON.stringify({ inputs: { run_upgrade_smoke_arm: "false" } })],
    ["workflow_dispatch", "{}"],
  ] as const) {
    cases.push(
      c(
        `forced-${eventName}-${String(event.length)}`,
        up,
        { "upgrade-smoke-arm64": "success" },
        eventName,
        event,
        err("OPT_IN_MISMATCH upgrade-smoke-arm64"),
      ),
    );
  }
  cases.push({
    ...c(
      "nan-event",
      {},
      {},
      "workflow_dispatch",
      '{"inputs":{"run_upgrade_smoke_arm":NaN}}',
      err("EVENT_MALFORMED"),
    ),
    knownDifference: "planner: json.loads accepts NaN and reports DISPATCH_INPUT_VALUE_INVALID",
  });
  // Event files are read only by workflows with opt-in jobs.
  for (const workflow of ["web", "documents", "collab-engine"] as const) {
    cases.push({
      name: `opt-in/${workflow}-ignores-event`,
      argv: gateArgv(workflow, needs(plan(workflow), workflow)),
      eventName: null,
      event: "{bad",
      expect: ok(),
    });
  }
  return cases;
}

function usageCases(): GateCase[] {
  const valid = needs(plan("documents"), "documents");
  const usage = { code: 2 };
  const docs = (needsJson: string, tested = SHA_A) => [
    "--workflow",
    "documents",
    "--needs-json",
    needsJson,
    "--tested-sha",
    tested,
  ];
  const c = (name: string, argv: string[], expect: GateCase["expect"] = usage): GateCase => ({
    name: `usage/${name}`,
    argv,
    eventName: "pull_request",
    event: "{}",
    expect,
  });
  const help = { code: 0, help: true } as const;
  // argparse quirks the gate deliberately does not reproduce (all stricter).
  const strict = (name: string, argv: string[], why: string): GateCase => ({
    ...c(name, argv),
    knownDifference: `planner: argparse ${why}; gate refuses it (exit 2)`,
  });
  const dashValue =
    "accepts a value starting with - when it looks like a negative number or has a space";
  return [
    // Each --workflow occurrence is checked where it occurs.
    c("invalid-then-valid-workflow", ["--workflow", "bogus", ...docs(valid)]),
    c("valid-then-invalid-workflow", [...docs(valid), "--workflow", "bogus"]),
    c("invalid-workflow-equals", ["--workflow=bogus", ...docs(valid)]),
    c("invalid-workflow", ["--workflow", "release", "--needs-json", valid, "--tested-sha", SHA_A]),
    c("invalid-workflow-before-help", ["--workflow", "bogus", "--help"]),
    c("help-before-invalid-workflow", ["--help", "--workflow", "bogus"], help),
    c("short-help", ["-h"], help),
    c("last-wins", ["--workflow", "web", ...docs(valid)], ok()),
    c(
      "equals-form",
      ["--workflow=documents", `--needs-json=${valid}`, `--tested-sha=${SHA_A}`],
      ok(),
    ),
    // Help takes no attached value.
    c("help-equals-value", [...docs(valid), "--help=bad"]),
    c("help-equals-empty", [...docs(valid), "--help="]),
    c("short-help-equals", [...docs(valid), "-h=x"]),
    c("short-help-dash-tail", [...docs(valid), "-h-x"]),
    strict("short-help-tail", [...docs(valid), "-hbad"], "treats -hbad as -h plus extras"),
    strict(
      "short-help-tail-before-invalid",
      ["-hbad", "--workflow", "bogus"],
      "treats -hbad as -h plus extras",
    ),
    strict("short-help-twice", [...docs(valid), "-hh"], "treats -hh as -h -h"),
    // No abbreviations.
    strict("help-prefix", ["--he"], "expands --he to --help"),
    strict(
      "prefix-options",
      ["--w", "documents", "--n", valid, "--t", SHA_A],
      "expands unique prefixes",
    ),
    strict(
      "prefix-abbreviations",
      ["--work", "documents", "--needs", valid, `--tested=${SHA_A}`],
      "expands unique prefixes",
    ),
    c("tested-prefix-missing-value", [
      "--workflow",
      "documents",
      "--needs-json",
      valid,
      "--tested",
    ]),
    // A value starting with - is a missing value.
    strict("negative-prefix", docs("-1x"), dashValue),
    strict("negative-number", docs("-1"), dashValue),
    strict("negative-decimal-prefix", docs("-.1x"), dashValue),
    strict("negative-arabic-indic", docs("-١"), dashValue),
    strict("negative-fullwidth", docs("-１.２"), dashValue),
    strict("negative-final-newline", docs("-1\n"), dashValue),
    strict("negative-astral-digit", docs(valid, "-\u{1d7ce}"), dashValue),
    strict("value-with-space", docs("-x y"), dashValue),
    strict("lone-dash-value", docs("-"), "accepts - as a value"),
    strict(
      "negative-explicit",
      ["--workflow", "documents", "--needs-json=-1x", "--tested-sha", SHA_A],
      "accepts --opt=-value",
    ),
    strict("negative-then-valid-needs", ["--needs-json", "-1", ...docs(valid)], dashValue),
    c("superscript-value", docs("-²")),
    c("dot-letter-value", docs("-.x")),
    c("short-option-value", docs("-h")),
    c("unknown-option-value", docs("-x")),
    c("long-option-value", docs("--bad")),
    c("option-as-value", ["--workflow", "documents", "--needs-json", "--tested-sha", SHA_A]),
    c("option-as-sha", ["--tested-sha", "--workflow", "documents", "--needs-json", valid]),
    c("double-dash-value", docs("--")),
    c("empty-value", docs(""), fail("needs json missing")),
    c(
      "tested-equals",
      ["--workflow", "documents", "--needs-json", valid, `--tested-sha=${SHA_A}`],
      ok(),
    ),
    // Other usage errors.
    c("no-args", []),
    c("missing-workflow", ["--needs-json", valid, "--tested-sha", SHA_A]),
    c("missing-tested-sha", ["--workflow", "documents", "--needs-json", valid]),
    c("missing-value", ["--workflow", "documents", "--tested-sha"]),
    c("unknown-flag", [...docs(valid), "--extra"]),
    c("positional", [...docs(valid), "x"]),
    c("bare-double-dash", [...docs(valid), "--"]),
    c("trailing-double-dash", [...docs(valid), "--", "x"]),
    c("single-dash-unknown", [...docs(valid), "-x"]),
    c("empty-long-option", [...docs(valid), "--=x"]),
  ];
}

export function gateCorpus(): GateCase[] {
  return [
    ...selectionMatrix(),
    ...needsAndSchema(),
    ...matrixCases(),
    ...optInCases(),
    ...usageCases(),
  ];
}

/** The argv and environment a case runs with, given a scratch directory for its event file. */
export function materialize(
  testCase: GateCase,
  writeEvent: (contents: string | Uint8Array) => string,
): { argv: string[]; env: Record<string, string> } {
  const env: Record<string, string> = {};
  if (testCase.eventName !== null) env.GITHUB_EVENT_NAME = testCase.eventName;
  if (testCase.event !== null) env.GITHUB_EVENT_PATH = writeEvent(testCase.event);
  if (testCase.needsEnv !== undefined) env.NEEDS_JSON = testCase.needsEnv;
  return { argv: testCase.argv, env };
}
