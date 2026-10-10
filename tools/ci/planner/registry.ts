import { YAML } from "bun";
import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { readUtf8 } from "./paths.ts";

// The selection registry: the five gated workflows, their product jobs, the
// manual opt-ins, and the wiring the plan step relies on before it emits.

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
  install: ["install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"],
} as const satisfies Record<string, readonly string[]>;
export type Workflow = keyof typeof WORKFLOW_JOBS;
export const WORKFLOWS = Object.keys(WORKFLOW_JOBS) as Workflow[];

export function isWorkflow(name: string): name is Workflow {
  return Object.hasOwn(WORKFLOW_JOBS, name);
}

// Manual opt-in jobs: no path or event policy selects them (not even full
// mode or a fatal plan). Only a workflow_dispatch whose boolean input of the
// same name is exactly true selects the job; the gate re-derives that.
export const OPT_IN_JOBS: Partial<Record<Workflow, Readonly<Record<string, string>>>> = {
  install: { "upgrade-smoke-arm64": "run_upgrade_smoke_arm" },
};

export const WORKFLOW_YAML: Readonly<Record<Workflow, string>> = {
  web: "web.yml",
  rust: "rust.yml",
  documents: "documents.yml",
  "collab-engine": "collab-engine.yml",
  install: "install.yml",
};
export const RELEASE_WORKFLOW_FILE = "release.yml";
export const CI_BASE_WORKFLOW_FILE = "ci-base-image.yml";
export const TURSO_MANUAL_WORKFLOW_FILE = "turso-test.yml";

export const PLAN_JOB_ID = "ci-plan";
export const PLAN_OUTPUT_KEYS = ["mode", "reason_code", "plan_ok", "plan_json"] as const;
export const POSTGRES_MATRIX_EXPR = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";
export const POSTGRES_MATRIX_OUTPUT_EXPR = "${{ steps.plan.outputs.postgres_matrix }}";
export const RUNNER_ARCH: Readonly<Record<string, "x64" | "arm64">> = {
  "ubuntu-26.04": "x64",
  "ubuntu-26.04-arm": "arm64",
};
const JOB_ID_RE = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;

export function gateJobId(workflow: string): string {
  return `${workflow}-ci-gate`;
}

/** The ci-plan output a job's `if:` reads; producer/consumer pairs share one. */
export function selectOutputKey(job: string): string {
  const shared: Record<string, string> = {
    "web-native-checks": "web-checks",
    "workspace-browser-build": "workspace-browser-shard",
    "postgres-build": "postgres",
  };
  return `select_${(shared[job] ?? job).replaceAll("-", "_")}`;
}

// ---- Workflow files -------------------------------------------------------

export type Mapping = Record<string, unknown>;
export type ParsedWorkflow = { ok: true; data: Mapping } | { ok: false; error: string };
/** What a registry verifier sees: the checkout and every workflow file, parsed. */
export type RegistryContext = {
  root: string;
  /** null when .github/workflows is not a directory. */
  workflows: ReadonlyMap<string, ParsedWorkflow> | null;
};
export type RegistryVerifier = (ctx: RegistryContext) => string[];

export const isMapping = (value: unknown): value is Mapping =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export function parseWorkflow(name: string, text: string): ParsedWorkflow {
  let data: unknown;
  try {
    data = YAML.parse(text);
  } catch (error) {
    return { ok: false, error: `${name}: YAML parse failed: ${(error as Error).message}` };
  }
  if (!isMapping(data)) return { ok: false, error: `${name}: workflow YAML must be a mapping` };
  return { ok: true, data };
}

/** Every *.yml / *.yaml file under .github/workflows, sorted by name. */
export function loadRegistryContext(root: string): RegistryContext {
  const dir = join(root, ".github", "workflows");
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return { root, workflows: null };
  const workflows = new Map<string, ParsedWorkflow>();
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (!/\.ya?ml$/.test(name) || !statSync(path).isFile()) continue;
    let text: string;
    try {
      text = readUtf8(path);
    } catch (error) {
      workflows.set(name, {
        ok: false,
        error: `${name}: YAML parse failed: ${(error as Error).message}`,
      });
      continue;
    }
    workflows.set(name, parseWorkflow(name, text));
  }
  return { root, workflows };
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((v, i) => deepEqual(v, b[i]))
    );
  }
  if (!isMapping(a) || !isMapping(b)) return false;
  const keys = Object.keys(a);
  return (
    keys.length === Object.keys(b).length &&
    keys.every((k) => Object.hasOwn(b, k) && deepEqual(a[k], b[k]))
  );
}

const steps = (job: Mapping): unknown[] => (Array.isArray(job.steps) ? job.steps : []);

/** Runner labels a job resolves to, including the plan-emitted postgres matrix. */
function jobRunners(job: Mapping, catalogRunners: (job: Mapping) => unknown[]): unknown[] {
  const runner = job["runs-on"];
  if (runner !== "${{ matrix.runner }}") return [runner];
  const strategy = job.strategy;
  const matrix = isMapping(strategy) ? strategy.matrix : undefined;
  if (matrix === POSTGRES_MATRIX_EXPR) return catalogRunners(job);
  const include = isMapping(matrix) ? (matrix.include ?? []) : [];
  const rows: unknown[] = Array.isArray(include) ? include : [];
  return rows.filter(isMapping).map((row) => row.runner);
}

/**
 * The registry subset the plan step itself depends on: the gated workflow
 * files, their triggers, job ids, the ci-plan job shape and its outputs. The
 * complete workflow registry (verify-workflows) replaces this at cutover.
 */
export function verifyPlanRegistry(
  ctx: RegistryContext,
  catalogRunners: (job: Mapping) => unknown[] = () => [],
): string[] {
  const errors: string[] = [];
  if (ctx.workflows === null) return ["missing .github/workflows directory"];
  const allowed = new Set([
    ...Object.values(WORKFLOW_YAML),
    RELEASE_WORKFLOW_FILE,
    CI_BASE_WORKFLOW_FILE,
    TURSO_MANUAL_WORKFLOW_FILE,
  ]);
  for (const [name, parsed] of ctx.workflows) {
    if (!allowed.has(name)) {
      errors.push(`unknown workflow file ${name}`);
      continue;
    }
    if (!parsed.ok || !isMapping(parsed.data.jobs)) continue;
    for (const [jobId, job] of Object.entries(parsed.data.jobs)) {
      if (!isMapping(job)) continue;
      const runners = jobRunners(job, catalogRunners);
      if (
        runners.length === 0 ||
        runners.some((r) => typeof r !== "string" || !Object.hasOwn(RUNNER_ARCH, r))
      ) {
        errors.push(`${name}: ${jobId} requires explicit Ubuntu 26.04 runners`);
      }
    }
  }

  for (const workflow of WORKFLOWS) {
    const filename = WORKFLOW_YAML[workflow];
    const parsed = ctx.workflows.get(filename);
    if (parsed === undefined) {
      errors.push(`${workflow}: missing workflow file ${filename}`);
      continue;
    }
    if (!parsed.ok) {
      errors.push(`${workflow}: ${parsed.error}`);
      continue;
    }
    const data = parsed.data;
    const triggers = data.on;
    if (!isMapping(triggers) || !Object.hasOwn(triggers, "pull_request")) {
      errors.push(`${workflow}: pull_request trigger is required for the stable gate`);
    } else if (triggers.pull_request !== null) {
      errors.push(`${workflow}: pull_request must be unfiltered so required gates always run`);
    }
    if (!isMapping(triggers) || !Object.hasOwn(triggers, "merge_group")) {
      errors.push(`${workflow}: merge_group trigger is required for the stable gate`);
    } else if (!deepEqual(triggers.merge_group, { types: ["checks_requested"] })) {
      errors.push(`${workflow}: merge_group must request checks_requested`);
    }
    const jobs = data.jobs;
    if (!isMapping(jobs) || Object.keys(jobs).length === 0) {
      errors.push(`${workflow}: jobs mapping missing`);
      continue;
    }
    if (Object.keys(jobs).some((id) => !JOB_ID_RE.test(id))) {
      errors.push(`${workflow}: invalid job id`);
      continue;
    }
    const reservedGate = gateJobId(workflow);
    if (!Object.hasOwn(jobs, PLAN_JOB_ID))
      errors.push(`${workflow}: missing reserved plan job ${PLAN_JOB_ID}`);
    if (!Object.hasOwn(jobs, reservedGate))
      errors.push(`${workflow}: missing reserved gate job ${reservedGate}`);
    const expected: readonly string[] = WORKFLOW_JOBS[workflow];
    for (const job of expected) {
      if (!Object.hasOwn(jobs, job)) errors.push(`${workflow}: missing registered job id ${job}`);
    }
    for (const job of Object.keys(jobs)) {
      if (job !== PLAN_JOB_ID && job !== reservedGate && !expected.includes(job)) {
        errors.push(`${workflow}: unregistered job id ${job}`);
      }
    }

    const planJob = jobs[PLAN_JOB_ID];
    if (!isMapping(planJob)) continue;
    // The ancestry exception trusts only GitHub's merge SHA for this event:
    // normal checkout (no alternate ref or repository), runner GITHUB_SHA.
    const planSteps: unknown = planJob.steps;
    const checkouts: unknown[] = Array.isArray(planSteps)
      ? (planSteps as unknown[]).filter(
          (s) =>
            isMapping(s) && typeof s.uses === "string" && s.uses.startsWith("actions/checkout@"),
        )
      : [];
    const checkout = checkouts[0];
    if (
      checkouts.length !== 1 ||
      !isMapping(checkout) ||
      !deepEqual(checkout.with, { "fetch-depth": 0 })
    ) {
      errors.push(
        `${workflow}: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override`,
      );
    }
    const envs = [
      data.env,
      planJob.env,
      ...steps(planJob)
        .filter(isMapping)
        .map((s) => s.env),
    ];
    if (envs.some((env) => isMapping(env) && Object.hasOwn(env, "GITHUB_SHA"))) {
      errors.push(`${workflow}: ci-plan must not override trusted GITHUB_SHA`);
    }
    if (Object.hasOwn(planJob, "if"))
      errors.push(`${workflow}: ${PLAN_JOB_ID} must not have an if condition`);
    const outputs = planJob.outputs;
    if (!isMapping(outputs)) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} outputs mapping missing`);
      continue;
    }
    for (const key of PLAN_OUTPUT_KEYS) {
      if (!Object.hasOwn(outputs, key))
        errors.push(`${workflow}: ${PLAN_JOB_ID} missing output ${key}`);
    }
    for (const job of expected) {
      const key = selectOutputKey(job);
      if (!Object.hasOwn(outputs, key)) errors.push(`${workflow}: missing selector output ${key}`);
    }
    if (workflow === "rust" && outputs.postgres_matrix !== POSTGRES_MATRIX_OUTPUT_EXPR) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} must publish postgres_matrix from the plan step`);
    }
    if (workflow === "rust" && Object.hasOwn(outputs, "postgres_exclude")) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} must not publish postgres_exclude`);
    }
  }
  return errors;
}
