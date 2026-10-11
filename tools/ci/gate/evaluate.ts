// The fail-closed gate decision for one workflow run.

import { postgresMatrixGateError } from "./matrix.ts";
import { loadNeedsContext } from "./needs.ts";
import { gateOptInError, type EventFile } from "./opt-in.ts";
import { WORKFLOW_JOBS, isSha, validatePlanSchema, type Plan, type Workflow } from "./schema.ts";
import { isBlank } from "./text.ts";

export type GateInput = {
  workflow: Workflow;
  /** `--needs-json`, else NEEDS_JSON; undefined when neither is set. */
  needsJson: string | undefined;
  testedSha: string;
  /** GITHUB_EVENT_NAME, already stripped; "" when unset. */
  eventName: string;
  readEvent: () => EventFile;
  loadRustWorkflow: () => unknown;
};

/** Returns null when the run passes, else the message printed after `gate: `. */
export function evaluateGate(input: GateInput): string | null {
  const { workflow } = input;
  if (!isSha(input.testedSha)) return "tested-sha invalid";
  if (input.needsJson === undefined || isBlank(input.needsJson)) return "needs json missing";

  const needs = loadNeedsContext(input.needsJson, workflow);
  if (!needs.ok) return `needs error ${needs.error}`;
  const { results, planOutputs } = needs.value;

  const schemaError = validatePlanSchema(needs.value.plan, workflow);
  if (schemaError) return `plan schema error ${schemaError}`;
  const plan = needs.value.plan as Plan;

  if (plan.tested_sha !== input.testedSha) return "tested_sha mismatch";

  const optInError = gateOptInError(workflow, plan, input.eventName, input.readEvent);
  if (optInError) return `opt-in error ${optInError}`;

  for (const job of WORKFLOW_JOBS[workflow]) {
    const selected = plan.jobs[job]?.selected === true;
    const result = results[job];
    if (selected && result !== "success") {
      return `selected job ${job} must succeed, got ${String(result)}`;
    }
    if (!selected && result !== "skipped") {
      return `unselected job ${job} must be skipped, got ${String(result)}`;
    }
  }

  const matrixError = postgresMatrixGateError(
    workflow,
    plan,
    planOutputs,
    input.eventName,
    input.loadRustWorkflow,
  );
  if (matrixError) return `postgres matrix error ${matrixError}`;
  return null;
}
