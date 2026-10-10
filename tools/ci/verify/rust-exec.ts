// PostgreSQL and S3 execution steps must run the validated db-tests binaries
// unconditionally (S3 on the B shard) with no masking, filtering or shell tricks.
import { get, has, pyRepr, pyStrip, type Mapping } from "./py.ts";
import {
  CARGO_TEST_NAME_RE,
  RUST_DB_TESTS_FEATURE,
  RUST_POSTGRES_INTEGRATION_RUN_CANONICAL,
  RUST_POSTGRES_INTEGRATION_STEP,
  RUST_S3_INTEGRATION_RUN_CANONICAL,
  RUST_S3_INTEGRATION_STEP,
  RUST_S3_INTEGRATION_STEP_IF,
  cargoCommandSuppressionError,
  cargoTestFlagsInText,
  collapseShellWords,
  executionStepMasked,
  normalizeRunScript,
  postgresJobSteps,
  uniqueNamedStep,
  type Result,
} from "./rust-shared.ts";

export function validateCargoTestInvocation(
  tokens: string[],
  context: string,
  requireTests: boolean,
): string | null {
  const shape = `rust: ${context} must invoke cargo test with --features db-tests`;
  if (tokens.length < 2 || tokens[0] !== "cargo" || tokens[1] !== "test") return shape;
  let locked = false;
  let offline = false;
  let noFailFast = false;
  let features = false;
  let sawTest = false;
  let index = 2;
  while (index < tokens.length) {
    const token = tokens[index] as string;
    if (token === "--locked") {
      locked = true;
      index += 1;
    } else if (token === "--offline") {
      offline = true;
      index += 1;
    } else if (token === "--no-fail-fast") {
      noFailFast = true;
      index += 1;
    } else if (token === "--features") {
      if (tokens[index + 1] !== RUST_DB_TESTS_FEATURE) return shape;
      features = true;
      index += 2;
    } else if (token === "--test") {
      const name = tokens[index + 1];
      if (name === undefined || !CARGO_TEST_NAME_RE.test(name))
        return `rust: ${context} must use --test NAME pairs only`;
      sawTest = true;
      index += 2;
    } else if (
      token === "${{" &&
      tokens[index + 1] === "matrix.tests" &&
      tokens[index + 2] === "}}"
    ) {
      index += 3;
    } else {
      return `rust: ${context} must not use unknown cargo test flag ${pyRepr(token)}`;
    }
  }
  if (!(locked && offline && noFailFast && features)) return shape;
  if (requireTests && !sawTest) return `rust: ${context} must declare at least one --test target`;
  return null;
}

function verifyCanonicalRun(
  run: string,
  context: string,
  canonical: string,
  mismatch: string,
): string | null {
  const norm = pyStrip(normalizeRunScript(run));
  if (norm.startsWith("echo ")) return mismatch;
  const suppression = cargoCommandSuppressionError(norm, context);
  if (suppression) return suppression;
  return collapseShellWords(norm) === canonical ? null : mismatch;
}

export function verifyPostgresIntegrationRun(run: string): string | null {
  return verifyCanonicalRun(
    run,
    "PostgreSQL integration step",
    RUST_POSTGRES_INTEGRATION_RUN_CANONICAL,
    "rust: PostgreSQL integration step must execute validated db-tests binaries for ${{ matrix.tests }}",
  );
}

export function verifyS3IntegrationRun(run: string): string | null {
  return verifyCanonicalRun(
    run,
    "S3 integration step",
    RUST_S3_INTEGRATION_RUN_CANONICAL,
    "rust: S3 integration step must invoke start-test-minio.sh with the validated db-tests binary",
  );
}

export function verifyPostgresIntegrationExecution(jobs: Mapping): string[] {
  const [steps, err] = postgresJobSteps(jobs);
  if (err !== null) return [err];
  const name = RUST_POSTGRES_INTEGRATION_STEP;
  const [step, stepErr] = uniqueNamedStep(steps, name, "postgres");
  if (stepErr !== null) return [stepErr];
  const masked = executionStepMasked(step, "postgres", name);
  if (masked) return [masked];
  if (has(step, "if")) return [`rust: postgres step ${pyRepr(name)} must not have an if condition`];
  const run = get(step, "run");
  if (typeof run !== "string")
    return [`rust: postgres step ${pyRepr(name)} must have a string run command`];
  const runErr = verifyPostgresIntegrationRun(run);
  return runErr ? [runErr] : [];
}

export function postgresS3Inventory(jobs: Mapping): Result<Set<string>> {
  const [steps, err] = postgresJobSteps(jobs);
  if (err !== null) return [null, err];
  const name = RUST_S3_INTEGRATION_STEP;
  const [step, stepErr] = uniqueNamedStep(steps, name, "postgres");
  if (stepErr !== null) return [null, stepErr];
  const masked = executionStepMasked(step, "postgres", name);
  if (masked) return [null, masked];
  const stepIf = get(step, "if");
  if (stepIf !== RUST_S3_INTEGRATION_STEP_IF) {
    return [
      null,
      `rust: S3 integration step if must be ${pyRepr(RUST_S3_INTEGRATION_STEP_IF)}, got ${pyRepr(stepIf)}`,
    ];
  }
  const run = get(step, "run");
  if (typeof run !== "string")
    return [null, `rust: postgres step ${pyRepr(name)} must have a string run command`];
  const runErr = verifyS3IntegrationRun(run);
  if (runErr) return [null, runErr];
  return [cargoTestFlagsInText(run), null];
}
