// The documents workflow runs no Python: the process client's native-dependency
// check is tools/ci/document-client-deps.ts under the pinned Bun, run on the
// client's own cargo metadata right after its tests.
import { deepEqual, get, isMapping, type Mapping, type VerifyContext } from "./load.ts";
import { SETUP_BUN_STEP } from "./registry.ts";

export const DOCUMENTS_WORKFLOW_FILE = "documents.yml";
const JOB = "native-extraction";
export const THIN_CLIENT_STEP = "Thin process client without native parser dependencies";
export const THIN_CLIENT_RUN =
  "cargo fetch --locked\n" +
  "cargo fmt --check\n" +
  "cargo clippy --locked --offline --all-targets -- -D warnings\n" +
  "cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings\n" +
  "cargo test --locked --offline --all-targets\n" +
  "cargo test --locked --offline --all-targets --features test-hang\n" +
  'cargo metadata --locked --offline --format-version 1 > "$RUNNER_TEMP/extract-client-metadata.json"\n' +
  'bun ../../tools/ci/document-client-deps.ts "$RUNNER_TEMP/extract-client-metadata.json"\n';
const THIN_CLIENT_KEYS = ["name", "working-directory", "run"];
const PYTHON = /python/i;

function steps(job: unknown): Mapping[] {
  const list = get(job, "steps");
  return Array.isArray(list) ? list.filter(isMapping) : [];
}

function usesPrefix(step: Mapping, prefix: string): boolean {
  const uses = get(step, "uses");
  return typeof uses === "string" && uses.startsWith(prefix);
}

/** No step of any documents job runs, installs or sets up Python. */
function verifyNoPython(data: Mapping, jobs: Mapping): string[] {
  const errors: string[] = [];
  const workflowShell = get(get(get(data, "defaults"), "run"), "shell");
  if (typeof workflowShell === "string" && PYTHON.test(workflowShell)) {
    errors.push(`${DOCUMENTS_WORKFLOW_FILE}: must not run or install Python`);
  }
  for (const [jobId, job] of Object.entries(jobs)) {
    const defaultShell = get(get(get(job, "defaults"), "run"), "shell");
    const values = [
      defaultShell,
      ...steps(job).flatMap((step) => ["run", "shell", "uses"].map((key) => get(step, key))),
    ];
    if (values.some((value) => typeof value === "string" && PYTHON.test(value))) {
      errors.push(`${DOCUMENTS_WORKFLOW_FILE}: ${jobId} must not run or install Python`);
    }
  }
  return errors;
}

/**
 * The client's native-dependency check: one exact thin-client step that ends
 * with the Bun checker over its cargo metadata, after one pinned setup-bun
 * that follows checkout.
 */
function verifyThinClient(job: unknown): string[] {
  const list = steps(job);
  const named = list.flatMap((step, index) =>
    get(step, "name") === THIN_CLIENT_STEP ? [index] : [],
  );
  const [at] = named;
  const step = at === undefined ? undefined : list[at];
  const run = get(step, "run");
  const errors: string[] = [];
  if (
    named.length !== 1 ||
    step === undefined ||
    Object.keys(step).length !== THIN_CLIENT_KEYS.length ||
    !THIN_CLIENT_KEYS.every((key) => Object.hasOwn(step, key)) ||
    get(step, "working-directory") !== "crates/document-extract-client" ||
    typeof run !== "string" ||
    run.replaceAll("\r\n", "\n").trim() + "\n" !== THIN_CLIENT_RUN
  ) {
    errors.push(
      `${DOCUMENTS_WORKFLOW_FILE}: ${JOB} must check the process client's cargo metadata with tools/ci/document-client-deps.ts in the exact thin-client step`,
    );
  }
  const setups = list.flatMap((item, index) =>
    usesPrefix(item, "oven-sh/setup-bun@") ? [index] : [],
  );
  const checkoutAt = list.findIndex((item) => usesPrefix(item, "actions/checkout@"));
  const [setupAt] = setups;
  if (
    setups.length !== 1 ||
    setupAt === undefined ||
    !deepEqual(list[setupAt], SETUP_BUN_STEP) ||
    checkoutAt < 0 ||
    checkoutAt > setupAt ||
    (at !== undefined && at < setupAt)
  ) {
    errors.push(
      `${DOCUMENTS_WORKFLOW_FILE}: ${JOB} must install pinned Bun from .bun-version once, after checkout and before the thin-client check`,
    );
  }
  return errors;
}

export function verifyDocumentsWorkflow(ctx: VerifyContext): string[] {
  // A missing or unparsable file, job map or job is reported by the registry.
  const data = ctx.workflows[DOCUMENTS_WORKFLOW_FILE];
  const jobs = get(data, "jobs");
  if (data === undefined || !isMapping(jobs)) return [];
  const job = get(jobs, JOB);
  return [...verifyNoPython(data, jobs), ...(isMapping(job) ? verifyThinClient(job) : [])];
}
