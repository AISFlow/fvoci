// Reads the gate job's `toJSON(needs)` payload: the plan job's result and
// outputs, the embedded plan, and every registered job's result.

import { PLAN_JOB_ID, WORKFLOW_JOBS, isRecord, sameKeySet, type Workflow } from "./schema.ts";
import { isBlank, parseJson } from "./text.ts";

export type JobResult = "success" | "failure" | "cancelled" | "skipped";
const VALID_RESULTS: ReadonlySet<string> = new Set(["success", "failure", "cancelled", "skipped"]);

export type NeedsContext = {
  /** The parsed plan_json output; its schema is not checked yet. */
  plan: unknown;
  results: Record<string, JobResult>;
  planOutputs: Record<string, string>;
};

type Result<T> = { ok: true; value: T } | { ok: false; error: string };

function needResult(entry: unknown): Result<JobResult> {
  if (!isRecord(entry)) return { ok: false, error: "NEED_ENTRY_TYPE" };
  if (!Object.hasOwn(entry, "result")) return { ok: false, error: "NEED_RESULT_MISSING" };
  const result = entry.result;
  if (typeof result !== "string") return { ok: false, error: "NEED_RESULT_TYPE" };
  if (!VALID_RESULTS.has(result)) return { ok: false, error: "NEED_RESULT_INVALID" };
  return { ok: true, value: result as JobResult };
}

// A missing or null outputs object is empty; anything else must map strings to strings.
function needOutputs(entry: Record<string, unknown>): Result<Record<string, string>> {
  const outputs = entry.outputs;
  if (outputs === undefined || outputs === null) return { ok: true, value: {} };
  if (!isRecord(outputs)) return { ok: false, error: "NEED_OUTPUTS_TYPE" };
  if (Object.values(outputs).some((value) => typeof value !== "string")) {
    return { ok: false, error: "NEED_OUTPUTS_TYPE" };
  }
  return { ok: true, value: outputs as Record<string, string> };
}

/** Returns the needs context, or the first error code in the planner's check order. */
export function loadNeedsContext(raw: string, workflow: Workflow): Result<NeedsContext> {
  const parsed = parseJson(raw);
  if (!parsed.ok) return { ok: false, error: "NEEDS_MALFORMED" };
  const needs = parsed.value;
  if (!isRecord(needs)) return { ok: false, error: "NEEDS_TYPE" };
  const expectedJobs = WORKFLOW_JOBS[workflow];
  if (!sameKeySet(Object.keys(needs), [PLAN_JOB_ID, ...expectedJobs])) {
    return { ok: false, error: "NEEDS_KEY_SET" };
  }

  const planEntry = needs[PLAN_JOB_ID];
  const planResult = needResult(planEntry);
  if (!planResult.ok) return { ok: false, error: `PLAN_${planResult.error}` };
  if (planResult.value !== "success") return { ok: false, error: "PLAN_RESULT" };
  const outputs = needOutputs(planEntry as Record<string, unknown>);
  if (!outputs.ok) return { ok: false, error: `PLAN_${outputs.error}` };
  const planJson = outputs.value.plan_json;
  if (planJson === undefined || isBlank(planJson)) return { ok: false, error: "PLAN_JSON_MISSING" };
  const plan = parseJson(planJson);
  if (!plan.ok) return { ok: false, error: "PLAN_JSON_MALFORMED" };

  const results: Record<string, JobResult> = {};
  for (const job of expectedJobs) {
    const result = needResult(needs[job]);
    if (!result.ok) return { ok: false, error: `JOB_${result.error}` };
    results[job] = result.value;
  }
  return { ok: true, value: { plan: plan.value, results, planOutputs: outputs.value } };
}
