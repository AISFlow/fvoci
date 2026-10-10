import { isMapping as isPyMapping, pyDumps, pyLoads, PyJsonError, type PyValue } from "./pyjson.ts";
import { isMapping, type Mapping, type RegistryContext } from "./registry.ts";

// The rust postgres matrix. rust.yml keeps the full catalog in the postgres
// job env; ci-plan emits the rows this event runs as the whole
// `strategy.matrix` ({"include": [...]}) and rust-ci-gate recomputes them.

export const POSTGRES_MATRIX_CATALOG_ENV = "FVOCI_POSTGRES_MATRIX_CATALOG";
export const RUST_WORKFLOW_FILE = "rust.yml";
/** The single PostgreSQL major and runner a pull request runs (all shards). */
export const PR_POSTGRES_ROW = { runner: "ubuntu-26.04", pg_major: "18" } as const;

type Rows = { rows: Map<string, PyValue>[]; error: null } | { rows: null; error: string };

export function postgresMatrixRows(postgresJob: Mapping): Rows {
  const env = postgresJob.env;
  const raw = isMapping(env) ? env[POSTGRES_MATRIX_CATALOG_ENV] : undefined;
  if (typeof raw !== "string" || raw.trim() === "") {
    return { rows: null, error: "rust: postgres matrix catalog missing" };
  }
  let include: PyValue;
  try {
    include = pyLoads(raw);
  } catch (error) {
    if (error instanceof PyJsonError)
      return { rows: null, error: "rust: postgres matrix catalog is not JSON" };
    throw error;
  }
  if (!Array.isArray(include) || include.length === 0) {
    return { rows: null, error: "rust: postgres matrix catalog must be a non-empty list" };
  }
  const rows: Map<string, PyValue>[] = [];
  for (const row of include) {
    if (!isPyMapping(row))
      return { rows: null, error: "rust: postgres matrix catalog row must be a mapping" };
    rows.push(row);
  }
  return { rows, error: null };
}

/** Catalog rows of rust.yml's postgres job as the registry context holds it. */
export function rustCatalogRows(ctx: RegistryContext): Rows {
  const parsed = ctx.workflows?.get(RUST_WORKFLOW_FILE);
  if (parsed === undefined)
    return { rows: null, error: `rust: missing workflow file ${RUST_WORKFLOW_FILE}` };
  if (!parsed.ok) return { rows: null, error: `rust: ${parsed.error}` };
  const jobs = parsed.data.jobs;
  if (!isMapping(jobs)) return { rows: null, error: "rust: jobs mapping missing" };
  const postgres = jobs.postgres;
  if (!isMapping(postgres)) return { rows: null, error: "rust: postgres job missing" };
  return postgresMatrixRows(postgres);
}

/**
 * Pull requests run PG 18 on x64 only (every shard); every other event runs
 * every row. The event alone decides: a pull request whose paths make the
 * plan full still runs the reduced rows, and merge_group runs the full
 * catalog before main.
 */
export function postgresMatrixRowRuns(eventName: string, row: Map<string, PyValue>): boolean {
  if (eventName !== "pull_request") return true;
  return (
    row.get("runner") === PR_POSTGRES_ROW.runner && row.get("pg_major") === PR_POSTGRES_ROW.pg_major
  );
}

export function postgresMatrixInclude(eventName: string, rows: readonly Map<string, PyValue>[]) {
  return rows.filter((row) => postgresMatrixRowRuns(eventName, row));
}

// Policy, not derived from the catalog it checks: PG 16, 17 and 18 on x64 and
// PG 18 on ARM64, each split into shards a, b and c (12 rows).
const POSTGRES_PLATFORMS = [
  ["16", "ubuntu-26.04"],
  ["17", "ubuntu-26.04"],
  ["18", "ubuntu-26.04"],
  ["18", "ubuntu-26.04-arm"],
] as const;
const POSTGRES_SHARDS = ["a", "b", "c"] as const;
export const POSTGRES_ROW_KEYS: readonly string[] = POSTGRES_PLATFORMS.flatMap(([major, runner]) =>
  POSTGRES_SHARDS.map((shard) => `${major}/${runner}/${shard}`),
);

const rowKey = (row: Map<string, PyValue>): string | null => {
  const [major, runner, shard] = [row.get("pg_major"), row.get("runner"), row.get("shard")];
  return typeof major === "string" && typeof runner === "string" && typeof shard === "string"
    ? `${major}/${runner}/${shard}`
    : null;
};

/** The catalog must hold every policy row exactly once and nothing else. */
export function postgresCatalogError(rows: readonly Map<string, PyValue>[]): string | null {
  const seen = new Set<string>();
  const problems: string[] = [];
  for (const row of rows) {
    const key = rowKey(row);
    if (key === null)
      return "rust: postgres matrix catalog row needs string pg_major, runner and shard";
    if (seen.has(key)) problems.push(`duplicate ${key}`);
    else if (!POSTGRES_ROW_KEYS.includes(key)) problems.push(`unexpected ${key}`);
    seen.add(key);
  }
  for (const key of POSTGRES_ROW_KEYS) if (!seen.has(key)) problems.push(`missing ${key}`);
  if (problems.length === 0) return null;
  return `rust: postgres matrix catalog must hold exactly the ${String(POSTGRES_ROW_KEYS.length)} policy rows (${problems.join(", ")})`;
}

/**
 * The `strategy.matrix` JSON line for this event: the PR rows (3) or the
 * full catalog (12). A catalog that would shrink either set is refused.
 */
export function postgresMatrixJson(
  eventName: string,
  rows: readonly Map<string, PyValue>[],
): { json: string; error: null } | { json: null; error: string } {
  const error = postgresCatalogError(rows);
  if (error !== null) return { json: null, error };
  const include = postgresMatrixInclude(eventName, rows);
  return { json: pyDumps(new Map([["include", include]]), { compact: true }), error: null };
}

/** Runner labels of the catalog, for the registry's explicit-runner check. */
export function catalogRunners(job: Mapping): unknown[] {
  const { rows } = postgresMatrixRows(job);
  return (rows ?? []).map((row) => row.get("runner"));
}
