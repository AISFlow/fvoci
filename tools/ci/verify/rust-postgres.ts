// PostgreSQL matrix catalog: row parsing, per-architecture --test inventory, the
// event-reduced matrix the planner emits, and the A/B/C budget split.
import {
  type Mapping,
  type VerifyContext,
  PY_WS,
  RUST_POSTGRES_BUILD_CACHE_KEY,
  field,
  isMapping,
  pyRepr,
  pySplit,
  pyStrip,
  rustJobs,
  same,
  steps,
} from "./rust-common.ts";

export const POSTGRES_MATRIX_CATALOG_ENV = "FVOCI_POSTGRES_MATRIX_CATALOG";
export const POSTGRES_MATRIX_EXPR = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";
export const POSTGRES_MATRIX_OUTPUT_EXPR = "${{ steps.plan.outputs.postgres_matrix }}";
export const RUST_POSTGRES_RUNNER_ARCH: Readonly<Record<string, "x64" | "arm64">> = {
  "ubuntu-26.04": "x64",
  "ubuntu-26.04-arm": "arm64",
};
// Finite PG16 A budget split; these two complete targets alone move to C.
export const RUST_POSTGRES_C_TARGETS: ReadonlySet<string> = new Set([
  "task_integration",
  "comment_integration",
]);
export const RUST_POSTGRES_BUDGET =
  "${{ matrix.shard == 'b' && (matrix.runner == 'ubuntu-26.04-arm' && 25 || 20) || 15 }}";
export const RUST_POSTGRES_IMAGES: Readonly<Record<string, string>> = {
  "16": "postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54",
  "17": "postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f",
  "18": "postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db",
};
// [runner, pg_major, check-name suffix] for each platform/major pair; shards a/b/c each.
const RUST_POSTGRES_PAIRS = [
  ["ubuntu-26.04", "18", ""],
  ["ubuntu-26.04-arm", "18", "-arm64"],
  ["ubuntu-26.04", "16", "-pg16"],
  ["ubuntu-26.04", "17", "-pg17"],
] as const;
const RUST_POSTGRES_SHARDS = ["a", "b", "c"] as const;
const RUST_POSTGRES_ROW_FIELDS = [
  "runner",
  "pg_major",
  "postgres_image",
  "check",
  "shard",
  "tests",
];

const CARGO_TEST_NAME_RE = /^[A-Za-z0-9_-]+$/;
const cargoTestFlagRe = new RegExp(`(?:^|${PY_WS})--test${PY_WS}+([A-Za-z0-9_-]+)`, "g");

export function cargoTestFlagsInText(text: string): Set<string> {
  return new Set(Array.from(text.matchAll(cargoTestFlagRe), (match) => match[1] ?? ""));
}

export function cargoCommandSuppressionError(
  norm: string,
  context: string,
  allowedLibtestArgs: ReadonlySet<string> = new Set(["--nocapture"]),
): string | null {
  if (norm.includes("--no-run")) return `rust: ${context} must not use --no-run`;
  if (norm.includes("--exclude")) return `rust: ${context} must not use --exclude`;
  for (const operator of ["||", "&&", "|", ";", "&"]) {
    if (norm.includes(operator)) {
      return `rust: ${context} must not contain shell operator ${pyRepr(operator)}`;
    }
  }
  const separator = " -- ";
  const at = norm.indexOf(separator);
  if (at >= 0 && !allowedLibtestArgs.has(pyStrip(norm.slice(at + separator.length)))) {
    return `rust: ${context} must not use libtest filter after --`;
  }
  return null;
}

export function matrixTestsFragmentError(testsField: string): string | null {
  const trimmed = pyStrip(testsField);
  if (!trimmed) return "rust: postgres matrix row missing tests command fragment";
  const suppression = cargoCommandSuppressionError(trimmed, "postgres matrix tests");
  if (suppression) return suppression;
  const tokens = pySplit(trimmed);
  const pairs = "rust: postgres matrix tests must be --test NAME pairs only";
  if (!tokens.length || tokens.length % 2 !== 0) return pairs;
  for (let index = 0; index < tokens.length; index += 2) {
    if (tokens[index] !== "--test" || !CARGO_TEST_NAME_RE.test(tokens[index + 1] ?? ""))
      return pairs;
  }
  return null;
}

export type Result<T> = { value: T; error: null } | { value: null; error: string };

/** The JSON catalog stored in the postgres job env; every row a mapping. */
export function postgresMatrixRows(postgresJob: unknown): Result<Mapping[]> {
  const env = field(postgresJob, "env");
  const raw = isMapping(env) ? env[POSTGRES_MATRIX_CATALOG_ENV] : undefined;
  if (typeof raw !== "string" || !pyStrip(raw)) {
    return { value: null, error: "rust: postgres matrix catalog missing" };
  }
  let include: unknown;
  try {
    include = JSON.parse(raw);
  } catch {
    return { value: null, error: "rust: postgres matrix catalog is not JSON" };
  }
  if (!Array.isArray(include) || !include.length) {
    return { value: null, error: "rust: postgres matrix catalog must be a non-empty list" };
  }
  if (!include.every(isMapping)) {
    return { value: null, error: "rust: postgres matrix catalog row must be a mapping" };
  }
  return { value: include, error: null };
}

export type ArchInventory = { x64: Set<string>; arm64: Set<string> };

/** --test targets per architecture; an empty inventory accompanies any error. */
export function postgresMatrixInventory(jobs: Mapping): {
  perArch: ArchInventory;
  error: string | null;
} {
  const empty = (error: string) => ({
    perArch: { x64: new Set<string>(), arm64: new Set<string>() },
    error,
  });
  const postgres = jobs.postgres;
  if (!isMapping(postgres)) return empty("rust: postgres job missing");
  const rows = postgresMatrixRows(postgres);
  if (rows.error !== null) return empty(rows.error);
  const perArch: ArchInventory = { x64: new Set(), arm64: new Set() };
  for (const row of rows.value) {
    const runner = row.runner;
    if (typeof runner !== "string" || !Object.hasOwn(RUST_POSTGRES_RUNNER_ARCH, runner)) {
      return empty(`rust: postgres matrix row has unknown runner ${pyRepr(runner)}`);
    }
    const tests = row.tests;
    if (typeof tests !== "string")
      return empty("rust: postgres matrix row missing tests command fragment");
    const fragmentError = matrixTestsFragmentError(tests);
    if (fragmentError) return empty(fragmentError);
    const arch = RUST_POSTGRES_RUNNER_ARCH[runner] as keyof ArchInventory;
    for (const name of cargoTestFlagsInText(tests)) perArch[arch].add(name);
  }
  return { perArch, error: null };
}

/**
 * Pull requests run PG 18 on x64 only; every other event runs every row. The event
 * alone decides: a broadened pull request still runs the reduced rows, and
 * merge_group runs the full catalog before main. The gate recomputes this matrix.
 */
export function postgresMatrixRowRuns(eventName: string, row: Mapping): boolean {
  if (eventName !== "pull_request") return true;
  return row.runner === "ubuntu-26.04" && row.pg_major === "18";
}

export function postgresMatrixInclude(eventName: string, rows: readonly Mapping[]): Mapping[] {
  return rows.filter((row) => postgresMatrixRowRuns(eventName, row));
}

/** Compact JSON as Python json.dumps(separators=(",", ":")) writes it. */
export function postgresMatrixJson(eventName: string, rows: readonly Mapping[]): string {
  return pyJsonDumps({ include: postgresMatrixInclude(eventName, rows) });
}

// json.dumps escapes non-ASCII (ensure_ascii) where JSON.stringify keeps it raw.
function pyJsonDumps(value: unknown): string {
  return JSON.stringify(value).replace(
    /[\u007f-￿]/g,
    (char) => "\\u" + char.charCodeAt(0).toString(16).padStart(4, "0"),
  );
}

/** Keep twelve isolated A/B/C rows and equal complete coverage per pair. */
export function verifyPostgresBudgetMatrix(jobs: Mapping): string[] {
  const job = jobs.postgres;
  if (!isMapping(job)) return ["rust: PostgreSQL budget job missing"];
  const errors: string[] = [];
  if (field(job, "timeout-minutes") !== RUST_POSTGRES_BUDGET) {
    errors.push("rust: PostgreSQL budget must retain A/C15m, x64 B20m and ARM64 B25m");
  }
  if (field(job, "runs-on") !== "${{ matrix.runner }}" || Object.hasOwn(job, "continue-on-error")) {
    errors.push("rust: PostgreSQL budget requires isolated matrix runners without error masking");
  }
  const strategy = Object.hasOwn(job, "strategy") ? job.strategy : {};
  if (!isMapping(strategy) || field(strategy, "fail-fast") !== false) {
    errors.push("rust: PostgreSQL budget must run every selected matrix row");
  }
  if (!isMapping(strategy) || field(strategy, "matrix") !== POSTGRES_MATRIX_EXPR) {
    errors.push(
      "rust: PostgreSQL budget matrix must be fromJSON(needs.ci-plan.outputs.postgres_matrix)" +
        " without an empty-array fallback",
    );
  }
  const rows = postgresMatrixRows(job);
  if (rows.error !== null) return [...errors, rows.error];
  const expected = new Map<string, string>();
  for (const [runner, major, suffix] of RUST_POSTGRES_PAIRS) {
    for (const shard of RUST_POSTGRES_SHARDS) {
      expected.set(
        rowKey(runner, major, shard),
        "postgres" + suffix + (shard === "a" ? "" : "-" + shard),
      );
    }
  }
  const seen = new Set<string>();
  const perPair = new Map<string, Set<string>>();
  for (const row of rows.value) {
    const { runner, pg_major: major, shard } = row;
    if (typeof runner !== "string" || typeof major !== "string" || typeof shard !== "string") {
      errors.push("rust: PostgreSQL budget has an unsupported platform/major/shard row");
      continue;
    }
    const key = rowKey(runner, major, shard);
    if (!expected.has(key)) {
      errors.push("rust: PostgreSQL budget has an unsupported platform/major/shard row");
      continue;
    }
    if (seen.has(key)) errors.push("rust: PostgreSQL budget has a duplicate matrix row");
    seen.add(key);
    const fields = Object.keys(row);
    if (
      fields.length !== RUST_POSTGRES_ROW_FIELDS.length ||
      !RUST_POSTGRES_ROW_FIELDS.every((name) => fields.includes(name))
    ) {
      errors.push("rust: PostgreSQL budget row must keep exact execution fields");
    }
    if (row.check !== expected.get(key) || row.postgres_image !== RUST_POSTGRES_IMAGES[major]) {
      errors.push("rust: PostgreSQL budget must retain check names and signed image pins");
    }
    const fragment = row.tests;
    if (typeof fragment !== "string" || matrixTestsFragmentError(fragment)) {
      errors.push("rust: PostgreSQL budget requires complete --test target pairs");
      continue;
    }
    const names = pySplit(fragment).filter((_, index) => index % 2 === 1);
    const targets = new Set(names);
    const pair = rowKey(runner, major);
    const assigned = perPair.get(pair) ?? new Set<string>();
    perPair.set(pair, assigned);
    if (names.length !== targets.size || names.some((name) => assigned.has(name))) {
      errors.push("rust: PostgreSQL budget duplicates a target within a platform/major pair");
    }
    for (const name of targets) assigned.add(name);
    if (shard === "c" && !sameSet(targets, RUST_POSTGRES_C_TARGETS)) {
      errors.push("rust: PostgreSQL budget C must run exactly task/comment targets");
    }
  }
  if (!sameSet(seen, new Set(expected.keys()))) {
    errors.push("rust: PostgreSQL budget requires all twelve A/B/C rows");
  }
  const baseline = perPair.get(rowKey("ubuntu-26.04", "18")) ?? new Set<string>();
  if ([...perPair.values()].some((targets) => !sameSet(targets, baseline))) {
    errors.push(
      "rust: PostgreSQL budget must retain equal target coverage on every platform/major pair",
    );
  }
  const cache = steps(jobs["postgres-build"]).filter(
    (step) => step.name === "Restore server build outputs",
  );
  if (
    cache.length !== 1 ||
    !same(field(cache[0], "with"), { path: "target/db-tests", key: RUST_POSTGRES_BUILD_CACHE_KEY })
  ) {
    errors.push(
      "rust: PostgreSQL producer must retain strict complete-input server cache without restore fallback",
    );
  }
  return errors;
}

export function verifyPostgresBudgetMatrixCtx(ctx: VerifyContext): string[] {
  const jobs = rustJobs(ctx);
  return jobs ? verifyPostgresBudgetMatrix(jobs) : [];
}

const rowKey = (...parts: string[]) => JSON.stringify(parts);

function sameSet(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  return a.size === b.size && [...a].every((item) => b.has(item));
}
