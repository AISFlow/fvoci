import { ALWAYS_FULL_EVENTS, KNOWN_EVENTS } from "./events.ts";
import { decideFromPaths, type MarkdownInputs, type SelectionDecision } from "./paths.ts";
import { OPT_IN_JOBS, WORKFLOW_JOBS, type Workflow } from "./registry.ts";

// Plan v3: which jobs of one workflow this run selects, and why.

export const PLAN_VERSION = 3;
const REASON_CODE_RE = /^[A-Z][A-Z0-9_]{0,63}$/;

export function sanitizeReasonCode(code: string): string {
  if (!REASON_CODE_RE.test(code)) throw new Error(`unsafe reason code: ${JSON.stringify(code)}`);
  return code;
}

export type Plan = {
  version: number;
  workflow: Workflow;
  mode: "full" | "narrow";
  reason_code: string;
  plan_ok: boolean;
  base_sha: string | null;
  head_sha: string | null;
  merge_base_sha: string | null;
  tested_sha: string | null;
  path_count: number;
  jobs: Record<string, { selected: boolean }>;
};

export function workflowJobSelected(
  workflow: Workflow,
  job: string,
  decision: SelectionDecision,
): boolean {
  if (Object.hasOwn(OPT_IN_JOBS[workflow] ?? {}, job)) return false;
  if (decision.mode === "full") return true;
  return (
    ((workflow === "web" || workflow === "install") &&
      decision.families.has("frontend_web_install")) ||
    (workflow === "web" && decision.families.has("web_tests"))
  );
}

export type PlanInputs = {
  workflow: Workflow;
  eventName: string;
  baseSha: string | null;
  headSha: string | null;
  mergeBaseSha: string | null;
  testedSha: string | null;
  paths: readonly string[] | null;
  fatalError?: string | null;
  forceFullReason?: string | null;
  optInInputs?: ReadonlySet<string>;
  markdown?: MarkdownInputs;
};

const full = (reasonCode: string): SelectionDecision => ({
  mode: "full",
  reasonCode,
  families: new Set(),
});

export function buildPlan(inputs: PlanInputs): Plan {
  const { workflow, eventName, paths, fatalError, forceFullReason } = inputs;
  let planOk = true;
  let decision: SelectionDecision;
  if (fatalError) {
    decision = full(sanitizeReasonCode(fatalError));
    planOk = false;
  } else if (ALWAYS_FULL_EVENTS.has(eventName)) {
    decision = full(sanitizeReasonCode(`FULL_EVENT_${eventName.toUpperCase()}`));
  } else if (!KNOWN_EVENTS.has(eventName)) {
    // Unknown events still select the full job set; plan_ok stays false so
    // the required gate does not accept the run.
    decision = full("EVENT_UNKNOWN");
    planOk = false;
  } else if (forceFullReason) {
    decision = full(sanitizeReasonCode(forceFullReason));
  } else if (paths === null) {
    decision = full("FULL_MISSING_PATHS");
    planOk = false;
  } else {
    decision = decideFromPaths(paths, inputs.markdown);
  }

  const jobs: Record<string, { selected: boolean }> = {};
  for (const job of WORKFLOW_JOBS[workflow])
    jobs[job] = { selected: workflowJobSelected(workflow, job, decision) };
  if (planOk && eventName === "workflow_dispatch") {
    for (const [job, input] of Object.entries(OPT_IN_JOBS[workflow] ?? {})) {
      if (inputs.optInInputs?.has(input)) jobs[job] = { selected: true };
    }
  }

  return {
    version: PLAN_VERSION,
    workflow,
    mode: decision.mode,
    reason_code: decision.reasonCode,
    plan_ok: planOk,
    base_sha: inputs.baseSha,
    head_sha: inputs.headSha,
    merge_base_sha: inputs.mergeBaseSha,
    tested_sha: inputs.testedSha,
    path_count: paths === null ? 0 : paths.length,
    jobs,
  };
}
