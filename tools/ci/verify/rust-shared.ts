// rust.yml registry vocabulary shared by the execution, collaboration,
// install, schema and suite-registry checks. The constants and the matrix
// catalog helpers are also used by the planner-side rust checks.
import { readFileSync, statSync } from "node:fs";
import { PY_WS, get, has, isMapping, pyRepr, pyStrip, pySplit, type Mapping } from "./py.ts";

export type VerifyContext = {
  root: string;
  workflows: Record<string, unknown>;
  /** The CLI loader's error for a discovered file it left out of workflows. */
  loadErrors?: Readonly<Record<string, string>>;
};
export type Result<T> = [T, null] | [null, string];

export const RUST_WORKFLOW_FILE = "rust.yml";
export const RUST_COLLAB_CI_SCRIPT = "scripts/run-rust-collaboration-ci-tests.sh";
export const RUST_CAPACITY_PROBE_SCRIPT = "scripts/collab-capacity-probe.sh";
export const RUST_POSTGRES_RUNNER_ARCH: Readonly<Record<string, "x64" | "arm64">> = {
  "ubuntu-26.04": "x64",
  "ubuntu-26.04-arm": "arm64",
};
export const RUST_INTEGRATION_MANUAL_TARGETS: ReadonlySet<string> = new Set([
  "collab_capacity_probe",
]);
export const RUST_NATIVE_ARM64_STEP = "Native server build and policy tests (ARM64)";
export const RUST_NATIVE_ARM64_RUN =
  "cargo build --locked --offline --bins\ncargo test --locked --offline --lib";
export const RUST_DB_TESTS_FEATURE = "db-tests";
export const RUST_SELECTED_INSTALL_STEP = "Selected SQLite install lifetime controls";
export const RUST_SELECTED_INSTALL_TARGET = "selected_install_lifetime";
export const RUST_SELECTED_INSTALL_IF = "matrix.shard == 'b' && matrix.pg_major == '18'";
// Binds the owned setup, compiler artifact selection and execution/count gate.
export const RUST_SELECTED_INSTALL_RUN_SHA256 =
  "cc1a88016e90b5aa59944736c9690a454a2c98e7fe1aa5cb201a47d8afbc29ee";
export const RUST_POSTGRES_INTEGRATION_STEP = "PostgreSQL integration tests";
export const RUST_S3_INTEGRATION_STEP =
  "S3-compatible storage integration tests (pinned test server)";
export const RUST_COLLAB_INTEGRATION_STEP =
  "WebSocket, PostgreSQL and native helper integration tests";
export const RUST_COLLAB_INTEGRATION_RUN = "bash scripts/run-rust-collaboration-ci-tests.sh";
export const RUST_COLLAB_MATRIX_RUNNERS: ReadonlySet<string> = new Set([
  "ubuntu-26.04",
  "ubuntu-26.04-arm",
]);
export const RUST_S3_INTEGRATION_STEP_IF = "matrix.shard == 'b'";
export const RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS: ReadonlySet<string> = new Set([
  "collab_wire",
  "markdown_process",
  "docx_export_process",
  "pdf_export_process",
  "pptx_export_process",
  "doctor_conversion",
  "office_extract_process",
  "static_api",
]);
export const CARGO_TEST_NAME_RE = /^[A-Za-z0-9_-]+$/;
export const RUST_POSTGRES_INTEGRATION_RUN_CANONICAL =
  'python3 scripts/ci_selection.py rust-binaries run --directory "$RUNNER_TEMP/rust-binaries" ${{ matrix.tests }}';
export const RUST_S3_INTEGRATION_RUN_CANONICAL =
  'bash scripts/start-test-minio.sh python3 scripts/ci_selection.py rust-binaries run --directory "$RUNNER_TEMP/rust-binaries" --test attachment_s3_integration';
export const POSTGRES_MATRIX_CATALOG_ENV = "FVOCI_POSTGRES_MATRIX_CATALOG";

export function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

// Strict UTF-8 like Python's read_text(encoding="utf-8"): invalid bytes are a
// refusal (null), never U+FFFD that could pass as a harmless comment.
export function readUtf8(path: string): string | null {
  try {
    return UTF8.decode(readFileSync(path));
  } catch {
    return null;
  }
}

export function notUtf8(rel: string): string {
  return `rust: ${rel} is not valid UTF-8`;
}

export function normalizeRunScript(text: string): string {
  return pyStrip(text.replaceAll("\r\n", "\n")) + "\n";
}

export function collapseShellWords(text: string): string {
  return pySplit(text).join(" ");
}

// Job steps that are mappings with a string `run`.
export function runSteps(job: unknown): Mapping[] {
  const steps = isMapping(job) ? get(job, "steps") : undefined;
  if (!Array.isArray(steps)) return [];
  return steps.filter(
    (step): step is Mapping => isMapping(step) && typeof get(step, "run") === "string",
  );
}

// Python re `\s` is the str.isspace() set, not the JS one (U+0085, U+001C-U+001F
// are whitespace; U+FEFF is not).
const CARGO_TEST_FLAG_RE = new RegExp(`(?:^|[${PY_WS}])--test[${PY_WS}]+([A-Za-z0-9_-]+)`, "g");

export function cargoTestFlagsInText(text: string): Set<string> {
  return new Set([...text.matchAll(CARGO_TEST_FLAG_RE)].map((match) => match[1] ?? ""));
}

// ctx.workflows maps a workflow filename to its parsed YAML (or the loader's
// parse Error); the CLI loader instead records a refused file in loadErrors.
// A file in neither does not exist.
export function rustWorkflowPresent(ctx: VerifyContext): boolean {
  return has(ctx.workflows, RUST_WORKFLOW_FILE) || rustLoadError(ctx) !== undefined;
}

function rustLoadError(ctx: VerifyContext): string | undefined {
  const errors = ctx.loadErrors;
  return errors !== undefined && Object.hasOwn(errors, RUST_WORKFLOW_FILE)
    ? errors[RUST_WORKFLOW_FILE]
    : undefined;
}

export function rustWorkflowJobs(ctx: VerifyContext): Result<Mapping> {
  if (!rustWorkflowPresent(ctx)) return [null, `rust: missing workflow file ${RUST_WORKFLOW_FILE}`];
  const loadError = rustLoadError(ctx);
  if (loadError !== undefined) return [null, `rust: ${loadError}`];
  const data = ctx.workflows[RUST_WORKFLOW_FILE];
  if (data instanceof Error)
    return [null, `rust: ${RUST_WORKFLOW_FILE}: YAML parse failed: ${data.message}`];
  if (!isMapping(data))
    return [null, `rust: ${RUST_WORKFLOW_FILE}: workflow YAML must be a mapping`];
  const jobs = get(data, "jobs");
  if (!isMapping(jobs)) return [null, "rust: jobs mapping missing"];
  return [jobs, null];
}

export function postgresJobSteps(jobs: Mapping): Result<unknown[]> {
  const job = get(jobs, "postgres");
  if (!isMapping(job)) return [null, "rust: postgres job missing"];
  const steps = get(job, "steps");
  if (!Array.isArray(steps)) return [null, "rust: postgres job steps missing"];
  return [steps, null];
}

export function uniqueNamedStep(steps: unknown[], stepName: string, job: string): Result<Mapping> {
  const matches = steps.filter(
    (step): step is Mapping => isMapping(step) && get(step, "name") === stepName,
  );
  if (matches.length === 0) return [null, `rust: missing ${job} step ${pyRepr(stepName)}`];
  if (matches.length !== 1)
    return [null, `rust: ${job} step ${pyRepr(stepName)} must appear exactly once`];
  return [matches[0] as Mapping, null];
}

export function executionStepMasked(step: Mapping, job: string, stepName: string): string | null {
  if (has(step, "continue-on-error") && get(step, "continue-on-error") !== false) {
    return `rust: ${job} step ${pyRepr(stepName)} must not use continue-on-error`;
  }
  return null;
}

const SHELL_OPERATORS = ["||", "&&", "|", ";", "&"] as const;
const DEFAULT_LIBTEST_ARGS: ReadonlySet<string> = new Set(["--nocapture"]);

export function cargoCommandSuppressionError(
  norm: string,
  context: string,
  allowedLibtestArgs: ReadonlySet<string> = DEFAULT_LIBTEST_ARGS,
): string | null {
  if (norm.includes("--no-run")) return `rust: ${context} must not use --no-run`;
  if (norm.includes("--exclude")) return `rust: ${context} must not use --exclude`;
  for (const operator of SHELL_OPERATORS) {
    if (norm.includes(operator))
      return `rust: ${context} must not contain shell operator ${pyRepr(operator)}`;
  }
  const separator = " -- ";
  const at = norm.indexOf(separator);
  if (at !== -1) {
    const suffix = pyStrip(norm.slice(at + separator.length));
    if (!allowedLibtestArgs.has(suffix))
      return `rust: ${context} must not use libtest filter after --`;
  }
  return null;
}

export function validateMatrixTestsFragment(testsField: string): string | null {
  const trimmed = pyStrip(testsField);
  if (!trimmed) return "rust: postgres matrix row missing tests command fragment";
  const suppression = cargoCommandSuppressionError(trimmed, "postgres matrix tests");
  if (suppression) return suppression;
  const tokens = pySplit(trimmed);
  const pairs = "rust: postgres matrix tests must be --test NAME pairs only";
  if (tokens.length === 0 || tokens.length % 2 !== 0) return pairs;
  for (let index = 0; index < tokens.length; index += 2) {
    if (tokens[index] !== "--test" || !CARGO_TEST_NAME_RE.test(tokens[index + 1] ?? ""))
      return pairs;
  }
  return null;
}

export function postgresMatrixRows(postgresJob: Mapping): Result<Mapping[]> {
  const env = get(postgresJob, "env");
  const raw = isMapping(env) ? get(env, POSTGRES_MATRIX_CATALOG_ENV) : undefined;
  if (typeof raw !== "string" || !pyStrip(raw))
    return [null, "rust: postgres matrix catalog missing"];
  let include: unknown;
  try {
    include = JSON.parse(raw);
  } catch {
    return [null, "rust: postgres matrix catalog is not JSON"];
  }
  if (!Array.isArray(include) || include.length === 0) {
    return [null, "rust: postgres matrix catalog must be a non-empty list"];
  }
  const rows: Mapping[] = [];
  for (const row of include) {
    if (!isMapping(row)) return [null, "rust: postgres matrix catalog row must be a mapping"];
    rows.push(row);
  }
  return [rows, null];
}

export type PerArch = { x64: Set<string>; arm64: Set<string> };

export function postgresMatrixInventory(jobs: Mapping): Result<PerArch> {
  const job = get(jobs, "postgres");
  if (!isMapping(job)) return [null, "rust: postgres job missing"];
  const [rows, err] = postgresMatrixRows(job);
  if (err !== null) return [null, err];
  const perArch: PerArch = { x64: new Set(), arm64: new Set() };
  for (const row of rows) {
    const runner = get(row, "runner");
    const testsField = get(row, "tests");
    if (typeof runner !== "string" || !Object.hasOwn(RUST_POSTGRES_RUNNER_ARCH, runner)) {
      return [null, `rust: postgres matrix row has unknown runner ${pyRepr(runner)}`];
    }
    if (typeof testsField !== "string")
      return [null, "rust: postgres matrix row missing tests command fragment"];
    const fragmentErr = validateMatrixTestsFragment(testsField);
    if (fragmentErr) return [null, fragmentErr];
    const arch = RUST_POSTGRES_RUNNER_ARCH[runner] as "x64" | "arm64";
    for (const name of cargoTestFlagsInText(testsField)) perArch[arch].add(name);
  }
  return [perArch, null];
}
