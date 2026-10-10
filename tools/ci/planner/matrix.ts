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

/** The `strategy.matrix` JSON line for this event. */
export function postgresMatrixJson(
  eventName: string,
  rows: readonly Map<string, PyValue>[],
): { json: string; error: null } | { json: null; error: string } {
  const include = postgresMatrixInclude(eventName, rows);
  return { json: pyDumps(new Map([["include", include]]), { compact: true }), error: null };
}

/** Runner labels of the catalog, for the registry's explicit-runner check. */
export function catalogRunners(job: Mapping): unknown[] {
  const { rows } = postgresMatrixRows(job);
  return (rows ?? []).map((row) => row.get("runner"));
}
