// Plan v3 contract and the selection tables the gate needs.
//
// These tables mirror WORKFLOW_JOBS, OPT_IN_JOBS and the plan constants of the
// planner. At cutover the TypeScript planner module becomes their single
// source and this file imports them instead of declaring them.

export const PLAN_VERSION = 3;
export const PLAN_JOB_ID = "ci-plan";

// Job order matters: the gate reports the first offending job in this order.
export const WORKFLOW_JOBS = {
  web: [
    "web-static",
    "web-checks",
    "web-native-checks",
    "workspace-browser-build",
    "workspace-browser-shard",
    "collaboration-build",
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
  ],
  rust: ["fast", "native-arm64", "postgres-build", "postgres", "collaboration"],
  documents: ["native-extraction"],
  "collab-engine": ["native-collab-engine"],
  install: ["install-image", "install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"],
} as const satisfies Record<string, readonly string[]>;

export type Workflow = keyof typeof WORKFLOW_JOBS;
export const WORKFLOWS = (Object.keys(WORKFLOW_JOBS) as Workflow[]).sort();

export function isWorkflow(value: string): value is Workflow {
  return Object.hasOwn(WORKFLOW_JOBS, value);
}

// Manual opt-in jobs: only a workflow_dispatch whose boolean input of this
// name is exactly true selects the job. The gate re-derives that from the
// event file, so the plan cannot override it.
export const OPT_IN_JOBS: Partial<Record<Workflow, Readonly<Record<string, string>>>> = {
  install: { "upgrade-smoke-arm64": "run_upgrade_smoke_arm" },
};

export const KNOWN_EVENTS: ReadonlySet<string> = new Set([
  "pull_request",
  "push",
  "merge_group",
  "workflow_dispatch",
]);

export function gateJobId(workflow: Workflow): string {
  return `${workflow}-ci-gate`;
}

const ALLOWED_PLAN_KEYS: ReadonlySet<string> = new Set([
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
const ALLOWED_JOB_ENTRY_KEYS: ReadonlySet<string> = new Set(["selected"]);

const SHA_RE = /^[0-9a-f]{40}$/;
const REASON_CODE_RE = /^[A-Z][A-Z0-9_]{0,63}$/;

export type JsonRecord = Record<string, unknown>;

/** A plan that passed validatePlanSchema; only the fields the gate reads are typed. */
export type Plan = JsonRecord & {
  version: typeof PLAN_VERSION;
  workflow: Workflow;
  mode: "full" | "narrow";
  reason_code: string;
  plan_ok: true;
  tested_sha: string;
  jobs: Record<string, { selected: boolean }>;
};

export function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isSha(value: unknown): value is string {
  return typeof value === "string" && SHA_RE.test(value);
}

export function sameKeySet(keys: readonly string[], expected: readonly string[]): boolean {
  const have = new Set(keys);
  const want = new Set(expected);
  return have.size === want.size && [...want].every((key) => have.has(key));
}

/** Returns the first schema error code, or null when the plan is valid. */
export function validatePlanSchema(plan: unknown, workflow: Workflow): string | null {
  if (!isRecord(plan)) return "PLAN_TOP_TYPE";
  if (Object.keys(plan).some((key) => !ALLOWED_PLAN_KEYS.has(key))) return "PLAN_UNKNOWN_KEYS";
  if (plan.version !== PLAN_VERSION) return "PLAN_VERSION";
  if (plan.workflow !== workflow) return "PLAN_WORKFLOW";
  if (plan.mode !== "full" && plan.mode !== "narrow") return "PLAN_MODE";
  if (typeof plan.reason_code !== "string" || !REASON_CODE_RE.test(plan.reason_code)) {
    return "PLAN_REASON_CODE";
  }
  if (typeof plan.plan_ok !== "boolean") return "PLAN_OK_TYPE";
  if (!plan.plan_ok) return "PLAN_NOT_OK";
  if (!isSha(plan.tested_sha)) return "PLAN_TESTED_SHA";
  const jobs = plan.jobs;
  if (!isRecord(jobs)) return "PLAN_JOBS";
  const expected = WORKFLOW_JOBS[workflow];
  if (!sameKeySet(Object.keys(jobs), expected)) return "PLAN_JOB_SET";
  for (const job of expected) {
    const entry = jobs[job];
    if (!isRecord(entry)) return "PLAN_JOB_MISSING";
    if (Object.keys(entry).some((key) => !ALLOWED_JOB_ENTRY_KEYS.has(key))) {
      return "PLAN_JOB_UNKNOWN_KEYS";
    }
    if (typeof entry.selected !== "boolean") return "PLAN_SELECTED_TYPE";
  }
  const selected = (job: string): boolean => (jobs[job] as { selected: boolean }).selected;
  // A consumer job is selected only together with the job that builds its binaries.
  if (
    workflow === "rust" &&
    (selected("postgres-build") !== selected("postgres") ||
      (selected("collaboration") && !selected("postgres-build")))
  ) {
    return "PLAN_BINARY_PRODUCER_SELECTION";
  }
  if (
    workflow === "web" &&
    selected("workspace-browser-build") !== selected("workspace-browser-shard")
  ) {
    return "PLAN_BROWSER_PRODUCER_SELECTION";
  }
  // Both container smokes run the image that install-image builds.
  if (
    workflow === "install" &&
    (selected("install-image") !== selected("install-smoke") ||
      selected("install-image") !== selected("backup-restore-smoke"))
  ) {
    return "PLAN_IMAGE_PRODUCER_SELECTION";
  }
  return null;
}
