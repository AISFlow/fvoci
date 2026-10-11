import {
  deepEqual,
  get,
  has,
  isMapping,
  pyRepr,
  triggersOf,
  type Mapping,
  type Value,
  type VerifyContext,
} from "./load.ts";
import { verifyOptInWiring } from "./optin.ts";
import { RELEASE_WORKFLOW_FILE, verifyReleaseWorkflow } from "./release.ts";
import { CI_BASE_WRITE_SCOPES, verifyWorkflowWriteScopes } from "./scopes.ts";
import {
  TURSO_MANUAL_WORKFLOW_FILE,
  verifyTursoWorkflow,
  verifyTursoWorkflowText,
} from "./turso.ts";

export const GATED_WORKFLOWS = ["web", "rust", "documents", "collab-engine", "install"] as const;
export type GatedWorkflow = (typeof GATED_WORKFLOWS)[number];

export const WORKFLOW_JOBS: Readonly<Record<GatedWorkflow, readonly string[]>> = {
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
};

export const WORKFLOW_YAML: Readonly<Record<GatedWorkflow, string>> = {
  web: "web.yml",
  rust: "rust.yml",
  documents: "documents.yml",
  "collab-engine": "collab-engine.yml",
  install: "install.yml",
};

export const CI_BASE_WORKFLOW_FILE = "ci-base-image.yml";
export const ALLOWED_WORKFLOW_FILES: readonly string[] = [
  ...Object.values(WORKFLOW_YAML),
  RELEASE_WORKFLOW_FILE,
  CI_BASE_WORKFLOW_FILE,
  TURSO_MANUAL_WORKFLOW_FILE,
];

export const PLAN_JOB_ID = "ci-plan";
export const PLAN_OUTPUT_KEYS = ["mode", "reason_code", "plan_ok", "plan_json"] as const;
export const GATE_NEEDS_JSON_EXPR = "${{ toJSON(needs) }}";
export const GATE_TESTED_SHA_EXPR = "${{ github.sha }}";
// The only pull_request trigger: the default activity types plus the draft
// transitions. A draft PR plans PR_DRAFT and fails its gate, so
// ready_for_review must run the real selection; without it the gate stays red
// until the next push. Branch or path filters would leave gates pending.
export const PULL_REQUEST_TRIGGER: Mapping = {
  types: ["opened", "synchronize", "reopened", "ready_for_review", "converted_to_draft"],
};
const JOB_ID_RE = /^[A-Za-z0-9][A-Za-z0-9_-]*$/;

/**
 * The jobs of a gated workflow whose own checks may run (the rust binary
 * handoff, the web checks): the file parsed to a mapping with a non-empty jobs
 * mapping of valid job ids. Otherwise the registry reports the problem and
 * those checks stay silent, as in the original.
 */
export function gatedWorkflowJobs(
  ctx: { workflows: Readonly<Record<string, unknown>> },
  file: string,
): Mapping | null {
  const data = get(ctx.workflows, file);
  if (!isMapping(data)) return null;
  const jobs = get(data, "jobs");
  if (!isMapping(jobs) || Object.keys(jobs).length === 0) return null;
  return Object.keys(jobs).every((id) => JOB_ID_RE.test(id)) ? jobs : null;
}

// The planner and gate invocations every gated workflow must carry. Moving the
// planner to another runtime changes only this table.
export const PLANNER_COMMANDS = {
  /** Text that marks the plan invocation line. */
  plan: "bun tools/ci/plan.ts",
  planMessage: "must invoke tools/ci/plan.ts",
  /** The selector regression wrapper: an exact line in rust ci-plan, absent elsewhere. */
  selectorRegression: "scripts/test-ci-selection.sh",
  gate: "bun tools/ci/gate.ts",
} as const;

// plan.ts and gate.ts use Bun builtins only: the pinned Bun is their whole
// toolchain. Each job has exactly one run step with the canonical text, so no
// package install or Python can run beside the selector.
export const SETUP_BUN_STEP: Mapping = {
  uses: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
  with: { "bun-version-file": ".bun-version" },
};

export function canonicalPlanRun(workflow: GatedWorkflow): string {
  return (
    "set -euo pipefail\n" +
    (workflow === "rust" ? `bash ${PLANNER_COMMANDS.selectorRegression}\n` : "") +
    `${PLANNER_COMMANDS.plan} \\\n` +
    `  --workflow ${workflow} \\\n` +
    '  --event-json "$GITHUB_EVENT_PATH" \\\n' +
    '  --output-plan "$RUNNER_TEMP/ci-selection-plan.json" \\\n' +
    '  --github-output "$GITHUB_OUTPUT"\n'
  );
}

export function canonicalGateRun(workflow: GatedWorkflow): string {
  return (
    "set -euo pipefail\n" +
    `${PLANNER_COMMANDS.gate} --workflow ${workflow} ` +
    '--needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n'
  );
}

/**
 * The job checks out, then installs the pinned Bun exactly once with no
 * condition, before the step at `runAt`.
 */
function verifyBunToolchain(
  workflow: GatedWorkflow,
  jobId: string,
  job: Mapping,
  runAt: number,
): string[] {
  const errors: string[] = [];
  const steps = stepList(job);
  const setups = steps.flatMap((step, index) =>
    usesStartsWith(step, ["oven-sh/setup-bun@"]) ? [index] : [],
  );
  const checkoutAt = steps.findIndex((step) => usesStartsWith(step, ["actions/checkout@"]));
  const [setupAt] = setups;
  if (
    setups.length !== 1 ||
    setupAt === undefined ||
    !deepEqual(steps[setupAt], SETUP_BUN_STEP) ||
    checkoutAt < 0 ||
    checkoutAt > setupAt ||
    (runAt >= 0 && runAt < setupAt)
  ) {
    errors.push(
      `${workflow}: ${jobId} must install pinned Bun from .bun-version once, after checkout and before the selector`,
    );
  }
  return errors;
}

export function gateJobId(workflow: string): string {
  return `${workflow}-ci-gate`;
}

export function selectOutputKey(job: string): string {
  // Both mandatory web budget lanes and each producer share a selection with
  // their consumers.
  const shared: Record<string, string> = {
    "web-native-checks": "web-checks",
    "workspace-browser-build": "workspace-browser-shard",
    "postgres-build": "postgres",
    "install-image": "install-smoke",
  };
  return `select_${(shared[job] ?? job).replaceAll("-", "_")}`;
}

export function expectedSelectIf(job: string): string {
  return `needs.${PLAN_JOB_ID}.outputs.${selectOutputKey(job)} == 'true'`;
}

// Runner labels and cache-family pins shared with the Rust workflow checks.
export const RUST_POSTGRES_RUNNER_ARCH: Readonly<Record<string, string>> = {
  "ubuntu-26.04": "x64",
  "ubuntu-26.04-arm": "arm64",
};
export const POSTGRES_MATRIX_CATALOG_ENV = "FVOCI_POSTGRES_MATRIX_CATALOG";
export const POSTGRES_MATRIX_EXPR = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";
export const POSTGRES_MATRIX_OUTPUT_EXPR = "${{ steps.plan.outputs.postgres_matrix }}";
export const CACHE_PIN = "0057852bfaa89a56745cba8c7296529d2fc39830";
export const RUST_POSTGRES_BUILD_CACHE_KEY =
  "v3-server-ubuntu-26.04-${{ runner.arch }}-1.98.1-postgres-db-tests-test-nodebug-" +
  "${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml') }}-" +
  "${{ hashFiles('src/**', 'tests/**', 'migrations/**', 'scripts/**', 'vendor/**', 'crates/**', '.cargo/**') }}-" +
  "${{ steps.sqlite.outputs.cache_identity }}";
export const SQLITE_PREFIX_PATH =
  "${{ runner.temp }}/fvoci-sqlite/${{ steps.sqlite.outputs.target }}";
export const SQLITE_PREFIX_KEY =
  "v1-sqlite-prefix-ubuntu-26.04-${{ runner.arch }}-1.98.1-${{ steps.sqlite.outputs.cache_identity }}";
const CACHE_QUALIFIER = "ubuntu-26.04-${{ runner.arch }}-1.98.1-";
const CACHE_ACTIONS = ["actions/cache@", "actions/cache/restore@", "actions/cache/save@"];
// Source-only cache that fetch-rhwp.sh revalidates.
const RHWP_SOURCE_CACHE = "crates/document-extract/.vendor-src/rhwp";

export function sqlitePrefixCacheSteps(saver = false): Mapping[] {
  const steps: Mapping[] = [
    {
      name: "Restore prepared SQLite prefix",
      id: "sqlite_prefix_cache",
      uses: "actions/cache/restore@" + CACHE_PIN,
      with: { path: SQLITE_PREFIX_PATH, key: SQLITE_PREFIX_KEY },
    },
    {
      name: "Verify cached SQLite prefix or build",
      env: { LIBCLANG_PATH: "/usr/lib/llvm-18/lib" },
      run:
        'bash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \\\n' +
        '  --cache-fallback --expected-cache-identity "${{ steps.sqlite.outputs.cache_identity }}" \\\n' +
        '  --github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"\n',
    },
  ];
  if (saver) {
    steps.push({
      name: "Save verified SQLite prefix",
      if: "steps.sqlite_prefix_cache.outputs.cache-hit != 'true'",
      uses: "actions/cache/save@" + CACHE_PIN,
      with: {
        path: SQLITE_PREFIX_PATH,
        key: "${{ steps.sqlite_prefix_cache.outputs.cache-primary-key }}",
      },
    });
  }
  return steps;
}

export function rustFastCacheSteps(): Mapping[] {
  const steps: Mapping[] = [];
  for (const [name, directory, identity, stepId] of [
    ["server", "target/default", "fast-default-test", "default_cache"],
    ["clippy", "target/clippy", "fast-db-tests-clippy", "clippy_cache"],
    ["SQLite library", "target/db-lib", "fast-db-tests-lib-test", "db_lib_cache"],
  ] as const) {
    steps.push({
      name: `Restore ${name} build outputs`,
      id: stepId,
      uses: "actions/cache/restore@" + CACHE_PIN,
      with: {
        path: directory,
        key: RUST_POSTGRES_BUILD_CACHE_KEY.replace("postgres-db-tests-test", identity),
      },
    });
    steps.push({
      name: `Save ${name} build outputs after validation`,
      if: `steps.${stepId}.outputs.cache-hit != 'true'`,
      uses: "actions/cache/save@" + CACHE_PIN,
      with: { path: directory, key: "${{ steps." + stepId + ".outputs.cache-primary-key }}" },
    });
  }
  return steps;
}

/** Rows of the postgres job's matrix catalog (the env JSON the plan filters). */
export function postgresMatrixRows(postgresJob: unknown): { rows?: Mapping[]; error?: string } {
  const raw = get(get(postgresJob, "env"), POSTGRES_MATRIX_CATALOG_ENV);
  if (typeof raw !== "string" || raw.trim() === "") {
    return { error: "rust: postgres matrix catalog missing" };
  }
  let include: unknown;
  try {
    include = JSON.parse(raw);
  } catch {
    return { error: "rust: postgres matrix catalog is not JSON" };
  }
  if (!Array.isArray(include) || include.length === 0) {
    return { error: "rust: postgres matrix catalog must be a non-empty list" };
  }
  if (!include.every(isMapping)) {
    return { error: "rust: postgres matrix catalog row must be a mapping" };
  }
  return { rows: include };
}

/** Python str.splitlines(): every Unicode line boundary, no trailing empty line. */
function splitLines(text: string): string[] {
  const boundaries = new Set([0x0a, 0x0b, 0x0c, 0x0d, 0x1c, 0x1d, 0x1e, 0x85, 0x2028, 0x2029]);
  const lines: string[] = [];
  let line = "";
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    if (!boundaries.has(code)) {
      line += text.charAt(index);
      continue;
    }
    if (code === 0x0d && text.charCodeAt(index + 1) === 0x0a) index++;
    lines.push(line);
    line = "";
  }
  if (line !== "") lines.push(line);
  return lines;
}

function scriptLines(text: string): string[] {
  return text
    .replaceAll("\r\n", "\n")
    .split("\n")
    .map((row) => row.trim())
    .filter((row) => row !== "");
}

function normalizeRunScript(text: string): string {
  return text.replaceAll("\r\n", "\n").trim() + "\n";
}

function stepList(job: unknown): Mapping[] {
  const steps = get(job, "steps");
  return Array.isArray(steps) ? steps.filter(isMapping) : [];
}

function runSteps(job: unknown): Mapping[] {
  return stepList(job).filter((step) => typeof get(step, "run") === "string");
}

function needsList(job: Mapping): { needs?: string[]; error?: string } {
  const needs = get(job, "needs");
  if (needs === undefined || needs === null) return { needs: [] };
  if (typeof needs === "string") return { needs: [needs] };
  if (Array.isArray(needs) && needs.every((item) => typeof item === "string")) {
    return { needs: needs };
  }
  return { error: "needs must be a string or list of strings" };
}

function usesStartsWith(step: Mapping, prefixes: readonly string[]): boolean {
  const uses = get(step, "uses");
  return typeof uses === "string" && prefixes.some((prefix) => uses.startsWith(prefix));
}

function jobRunners(job: Mapping): Value[] {
  const runner = get(job, "runs-on");
  if (runner !== "${{ matrix.runner }}") return [runner ?? null];
  const matrix = get(get(job, "strategy"), "matrix");
  let rows: Value[];
  if (matrix === POSTGRES_MATRIX_EXPR) rows = postgresMatrixRows(job).rows ?? [];
  else {
    const include = get(matrix, "include");
    rows = Array.isArray(include) ? include : [];
  }
  return rows.filter(isMapping).map((row) => get(row, "runner") ?? null);
}

/** Every job runs on an explicit Ubuntu 26.04 label and caches bind OS, arch and toolchain. */
function verifyRunnersAndCaches(file: string, data: Mapping): string[] {
  const errors: string[] = [];
  const jobs = get(data, "jobs");
  if (!isMapping(jobs)) return errors;
  const fastSaves = rustFastCacheSteps().filter((_, index) => index % 2 === 1);
  const sqliteSaver = sqlitePrefixCacheSteps(true)[2];
  for (const [jobId, job] of Object.entries(jobs)) {
    if (!isMapping(job)) continue;
    const runners = jobRunners(job);
    if (
      runners.length === 0 ||
      runners.some(
        (label) => typeof label !== "string" || !Object.hasOwn(RUST_POSTGRES_RUNNER_ARCH, label),
      )
    ) {
      errors.push(`${file}: ${jobId} requires explicit Ubuntu 26.04 runners`);
    }
    const steps = get(job, "steps");
    if (steps !== undefined && !Array.isArray(steps)) {
      errors.push(`${file}: ${jobId} steps must be a list`);
    }
    for (const step of stepList(job)) {
      if (!usesStartsWith(step, CACHE_ACTIONS)) continue;
      const cache = has(step, "with") ? get(step, "with") : {};
      if (!isMapping(cache)) {
        errors.push(`${file}: ${jobId} cache with must be a mapping`);
        continue;
      }
      if (get(cache, "path") === RHWP_SOURCE_CACHE) continue;
      const exempt =
        (file === "rust.yml" &&
          jobId === "fast" &&
          fastSaves.some((save) => deepEqual(step, save))) ||
        (file === "rust.yml" && jobId === "postgres-build" && deepEqual(step, sqliteSaver));
      if (exempt) continue;
      for (const field of ["key", "restore-keys"]) {
        if (!has(cache, field)) continue;
        const value = get(cache, field);
        if (
          typeof value !== "string" ||
          splitLines(value).some((line) => !line.includes(CACHE_QUALIFIER))
        ) {
          errors.push(
            `${file}: ${jobId} cache ${field} must bind Ubuntu 26.04, architecture and toolchain`,
          );
        }
      }
    }
  }
  return errors;
}

function verifyPlanJob(workflow: GatedWorkflow, data: Mapping, planJob: Mapping): string[] {
  const errors: string[] = [];
  const expected = WORKFLOW_JOBS[workflow];
  // The ancestry exception trusts only GitHub's merge SHA for this event. Pin
  // that assumption to normal checkout (no alternate ref or repository) and
  // prevent YAML from replacing the runner SHA.
  const planSteps = get(planJob, "steps");
  const checkouts = Array.isArray(planSteps)
    ? planSteps.filter(isMapping).filter((step) => usesStartsWith(step, ["actions/checkout@"]))
    : [];
  if (checkouts.length !== 1 || !deepEqual(get(checkouts[0], "with"), { "fetch-depth": 0 })) {
    errors.push(
      `${workflow}: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override`,
    );
  }
  const envs = [get(data, "env"), get(planJob, "env")];
  if (Array.isArray(planSteps))
    envs.push(...planSteps.filter(isMapping).map((step) => get(step, "env")));
  if (envs.some((env) => has(env, "GITHUB_SHA"))) {
    errors.push(`${workflow}: ci-plan must not override trusted GITHUB_SHA`);
  }
  if (has(planJob, "if")) errors.push(`${workflow}: ${PLAN_JOB_ID} must not have an if condition`);
  const outputs = get(planJob, "outputs");
  if (!isMapping(outputs)) {
    errors.push(`${workflow}: ${PLAN_JOB_ID} outputs mapping missing`);
  } else {
    for (const key of PLAN_OUTPUT_KEYS) {
      if (!has(outputs, key)) errors.push(`${workflow}: ${PLAN_JOB_ID} missing output ${key}`);
    }
    for (const job of expected) {
      const key = selectOutputKey(job);
      if (!has(outputs, key)) errors.push(`${workflow}: missing selector output ${key}`);
    }
    if (workflow === "rust" && get(outputs, "postgres_matrix") !== POSTGRES_MATRIX_OUTPUT_EXPR) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} must publish postgres_matrix from the plan step`);
    }
    if (workflow === "rust" && has(outputs, "postgres_exclude")) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} must not publish postgres_exclude`);
    }
  }
  const planRuns = runSteps(planJob)
    .map((step) => get(step, "run") as string)
    .join("\n");
  const planAt = stepList(planJob).findIndex((step) => {
    const run = get(step, "run");
    return typeof run === "string" && run.includes(PLANNER_COMMANDS.plan);
  });
  errors.push(...verifyBunToolchain(workflow, PLAN_JOB_ID, planJob, planAt));
  const planRunSteps = runSteps(planJob);
  if (
    planRunSteps.length !== 1 ||
    normalizeRunScript(get(planRunSteps[0], "run") as string) !== canonicalPlanRun(workflow)
  ) {
    errors.push(
      `${workflow}: ${PLAN_JOB_ID} must use the canonical plan invocation in its only run step`,
    );
  }
  if (!planRuns.includes(PLANNER_COMMANDS.plan)) {
    errors.push(`${workflow}: ${PLAN_JOB_ID} ${PLANNER_COMMANDS.planMessage}`);
  }
  if (
    !planRuns.includes(`--workflow ${workflow}`) &&
    !planRuns.includes(`--workflow=${workflow}`)
  ) {
    errors.push(`${workflow}: ${PLAN_JOB_ID} must pass --workflow ${workflow}`);
  }
  const wrapper = PLANNER_COMMANDS.selectorRegression;
  if (workflow === "rust") {
    const lines = scriptLines(planRuns);
    const wrapperAt = lines.indexOf(`bash ${wrapper}`);
    const planAt = lines.findIndex((row) => row.includes(PLANNER_COMMANDS.plan));
    if (wrapperAt < 0 || planAt < 0 || wrapperAt > planAt) {
      errors.push(`${workflow}: ${PLAN_JOB_ID} must run ${wrapper} before plan output`);
    }
  } else if (planRuns.includes(wrapper)) {
    errors.push(`${workflow}: ${PLAN_JOB_ID} must not duplicate ${wrapper}`);
  }
  return errors;
}

function verifyGateJob(workflow: GatedWorkflow, gateJob: Mapping): string[] {
  const errors: string[] = [];
  const gate = gateJobId(workflow);
  if (get(gateJob, "if") !== "always()") errors.push(`${workflow}: ${gate} must use if: always()`);
  if (get(gateJob, "name") !== gate) {
    errors.push(`${workflow}: ${gate} name must stay ${gate} for pull_request and merge_group`);
  }
  const { needs, error } = needsList(gateJob);
  const expectedNeeds = new Set([PLAN_JOB_ID, ...WORKFLOW_JOBS[workflow]]);
  if (error) errors.push(`${workflow}: ${gate} ${error}`);
  else {
    const actual = new Set(needs);
    if (
      actual.size !== expectedNeeds.size ||
      ![...actual].every((need) => expectedNeeds.has(need))
    ) {
      errors.push(`${workflow}: ${gate} needs must be ${PLAN_JOB_ID} and every registered job`);
    }
  }
  const steps = runSteps(gateJob);
  if (steps.length !== 1) {
    errors.push(`${workflow}: ${gate} must have exactly one run step`);
    return errors;
  }
  const [step] = steps as [Mapping];
  if (
    !deepEqual(get(step, "env"), {
      NEEDS_JSON: GATE_NEEDS_JSON_EXPR,
      TESTED_SHA: GATE_TESTED_SHA_EXPR,
    })
  ) {
    errors.push(
      `${workflow}: ${gate} env must be exactly ` +
        `NEEDS_JSON=${GATE_NEEDS_JSON_EXPR} and TESTED_SHA=${GATE_TESTED_SHA_EXPR}`,
    );
  }
  if (normalizeRunScript(get(step, "run") as string) !== canonicalGateRun(workflow)) {
    errors.push(`${workflow}: ${gate} must use the canonical gate invocation`);
  }
  errors.push(...verifyBunToolchain(workflow, gate, gateJob, stepList(gateJob).indexOf(step)));
  return errors;
}

function verifyGatedWorkflow(workflow: GatedWorkflow, data: Mapping): string[] {
  const errors: string[] = [];
  const triggers = triggersOf(data);
  if (!isMapping(triggers) || !has(triggers, "pull_request")) {
    errors.push(`${workflow}: pull_request trigger is required for the stable gate`);
  } else if (!deepEqual(get(triggers, "pull_request"), PULL_REQUEST_TRIGGER)) {
    errors.push(
      `${workflow}: pull_request must be exactly types: ` +
        `[${(PULL_REQUEST_TRIGGER.types as string[]).join(", ")}] so required gates always run`,
    );
  }
  if (!isMapping(triggers) || !has(triggers, "merge_group")) {
    errors.push(`${workflow}: merge_group trigger is required for the stable gate`);
  } else if (!deepEqual(get(triggers, "merge_group"), { types: ["checks_requested"] })) {
    errors.push(`${workflow}: merge_group must request checks_requested`);
  }
  const jobs = get(data, "jobs");
  if (!isMapping(jobs) || Object.keys(jobs).length === 0) {
    return [...errors, `${workflow}: jobs mapping missing`];
  }
  if (Object.keys(jobs).some((jobId) => !JOB_ID_RE.test(jobId))) {
    return [...errors, `${workflow}: invalid job id`];
  }

  const gate = gateJobId(workflow);
  const reserved = new Set([PLAN_JOB_ID, gate]);
  if (!has(jobs, PLAN_JOB_ID)) errors.push(`${workflow}: missing reserved plan job ${PLAN_JOB_ID}`);
  if (!has(jobs, gate)) errors.push(`${workflow}: missing reserved gate job ${gate}`);
  const expected = WORKFLOW_JOBS[workflow];
  for (const job of expected) {
    if (!has(jobs, job)) errors.push(`${workflow}: missing registered job id ${job}`);
    else if (reserved.has(job))
      errors.push(`${workflow}: registered job collides with reserved id ${job}`);
  }
  for (const job of Object.keys(jobs)) {
    if (!reserved.has(job) && !expected.includes(job)) {
      errors.push(`${workflow}: unregistered job id ${job}`);
    }
  }

  const planJob = get(jobs, PLAN_JOB_ID);
  if (isMapping(planJob)) errors.push(...verifyPlanJob(workflow, data, planJob));
  else if (has(jobs, PLAN_JOB_ID)) errors.push(`${workflow}: ${PLAN_JOB_ID} must be a mapping`);

  for (const job of expected) {
    const spec = get(jobs, job);
    if (!isMapping(spec)) continue;
    const { needs, error } = needsList(spec);
    if (error) errors.push(`${workflow}: ${job} ${error}`);
    else if (!(needs ?? []).includes(PLAN_JOB_ID))
      errors.push(`${workflow}: ${job} must need ${PLAN_JOB_ID}`);
    if (get(spec, "if") !== expectedSelectIf(job)) {
      errors.push(`${workflow}: ${job} if must be ${pyRepr(expectedSelectIf(job))}`);
    }
  }

  const gateJob = get(jobs, gate);
  if (isMapping(gateJob)) errors.push(...verifyGateJob(workflow, gateJob));
  else if (has(jobs, gate)) errors.push(`${workflow}: ${gate} must be a mapping`);

  errors.push(...verifyOptInWiring(workflow, data, jobs));
  return errors;
}

/**
 * The workflow registry: only registered files exist, every gated workflow is
 * wired to the planner and its stable gate, and the release, image and Turso
 * exceptions keep their trigger and token boundaries.
 */
export function verifyWorkflowRegistry(ctx: VerifyContext): string[] {
  if (ctx.files === null) return ["missing .github/workflows directory"];
  const errors: string[] = [];
  for (const file of ctx.files) {
    if (!ALLOWED_WORKFLOW_FILES.includes(file)) {
      errors.push(`unknown workflow file ${file}`);
      continue;
    }
    // A parse failure is reported by the workflow-specific check below.
    const data = ctx.workflows[file];
    if (data) errors.push(...verifyRunnersAndCaches(file, data));
  }

  const present = new Set(ctx.files);
  for (const workflow of GATED_WORKFLOWS) {
    const file = WORKFLOW_YAML[workflow];
    if (!present.has(file)) {
      errors.push(`${workflow}: missing workflow file ${file}`);
      continue;
    }
    const data = ctx.workflows[file];
    if (!data) {
      errors.push(`${workflow}: ${ctx.loadErrors[file] ?? "unreadable"}`);
      continue;
    }
    errors.push(...verifyGatedWorkflow(workflow, data));
  }

  const exceptions: [string, (data: Mapping) => string[]][] = [
    [RELEASE_WORKFLOW_FILE, (data) => verifyReleaseWorkflow(data)],
    [
      CI_BASE_WORKFLOW_FILE,
      (data) => verifyWorkflowWriteScopes(data, CI_BASE_WORKFLOW_FILE, CI_BASE_WRITE_SCOPES),
    ],
    [TURSO_MANUAL_WORKFLOW_FILE, (data) => verifyTursoWorkflow(data)],
  ];
  for (const [file, check] of exceptions) {
    if (!present.has(file)) continue;
    const data = ctx.workflows[file];
    errors.push(...(data ? check(data) : [`${file}: ${ctx.loadErrors[file] ?? "unreadable"}`]));
  }
  const tursoText = ctx.texts[TURSO_MANUAL_WORKFLOW_FILE];
  if (tursoText !== undefined) errors.push(...verifyTursoWorkflowText(tursoText));
  return errors;
}

// The five required status checks (branch protection), one per gated workflow.
export const REQUIRED_CHECKS: Readonly<Record<string, string>> = {
  "rust-ci-gate": "rust.yml",
  "web-ci-gate": "web.yml",
  "install-ci-gate": "install.yml",
  "documents-ci-gate": "documents.yml",
  "collab-engine-ci-gate": "collab-engine.yml",
};

const SHA_PINNED_USES = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_./-]+@[0-9a-f]{40}$/;

/**
 * Boundaries that keep the required gates honest beyond the planner wiring:
 * the five check names, gate triggers without path filters, SHA-pinned
 * actions and the plan-owned postgres matrix.
 */
export function verifyGateHardening(ctx: VerifyContext): string[] {
  const errors: string[] = [];
  for (const [check, file] of Object.entries(REQUIRED_CHECKS)) {
    const data = ctx.workflows[file];
    if (!data) continue; // Missing and unparsable files are reported by the registry.
    if (get(get(get(data, "jobs"), check), "name") !== check) {
      errors.push(`${file}: required check ${check} missing`);
    }
    const triggers = triggersOf(data);
    if (!isMapping(triggers)) continue;
    for (const [event, filter] of Object.entries(triggers)) {
      for (const key of ["paths", "paths-ignore"]) {
        if (has(filter, key)) {
          errors.push(`${file}: ${event} must not use ${key} so required gates always run`);
        }
      }
    }
  }

  for (const file of Object.keys(ctx.workflows).sort()) {
    const jobs = get(ctx.workflows[file], "jobs");
    if (!isMapping(jobs)) continue;
    for (const [jobId, job] of Object.entries(jobs)) {
      const uses = [get(job, "uses"), ...stepList(job).map((step) => get(step, "uses"))];
      for (const ref of uses) {
        if (ref !== undefined && (typeof ref !== "string" || !SHA_PINNED_USES.test(ref))) {
          errors.push(
            `${file}: ${jobId} uses ${pyRepr(typeof ref === "string" ? ref : JSON.stringify(ref))} must pin a full commit SHA`,
          );
        }
      }
    }
  }

  const postgres = get(get(ctx.workflows["rust.yml"], "jobs"), "postgres");
  if (
    isMapping(postgres) &&
    !deepEqual(get(postgres, "strategy"), { "fail-fast": false, matrix: POSTGRES_MATRIX_EXPR })
  ) {
    errors.push(
      `rust: postgres strategy must be exactly fail-fast: false and matrix: ${POSTGRES_MATRIX_EXPR}`,
    );
  }
  return errors;
}
