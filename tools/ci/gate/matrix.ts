// The selected rust postgres job must run exactly the matrix its event requires.

import { KNOWN_EVENTS, isRecord, type JsonRecord, type Plan, type Workflow } from "./schema.ts";
import { isBlank, parseJson } from "./text.ts";

export const RUST_WORKFLOW_FILE = "rust.yml";
export const POSTGRES_MATRIX_CATALOG_ENV = "FVOCI_POSTGRES_MATRIX_CATALOG";

/** The include rows of the postgres job's catalog env, or null when it is unusable. */
export function postgresMatrixRows(postgresJob: unknown): JsonRecord[] | null {
  if (!isRecord(postgresJob)) return null;
  const env = postgresJob.env;
  const raw = isRecord(env) ? env[POSTGRES_MATRIX_CATALOG_ENV] : undefined;
  if (typeof raw !== "string" || isBlank(raw)) return null;
  const parsed = parseJson(raw);
  if (!parsed.ok || !Array.isArray(parsed.value) || parsed.value.length === 0) return null;
  const rows: unknown[] = parsed.value;
  return rows.every(isRecord) ? rows : null;
}

/** The catalog rows of a parsed rust.yml mapping, or null when it is unusable. */
export function catalogFromRustWorkflow(data: unknown): JsonRecord[] | null {
  if (!isRecord(data) || !isRecord(data.jobs)) return null;
  return postgresMatrixRows(data.jobs.postgres);
}

/**
 * Pull requests run PG 18 on x64 only; every other event runs every row.
 * The event alone decides: a pull request broadened to full mode still runs
 * the reduced rows, and merge_group runs the full catalog before main.
 */
export function postgresMatrixInclude(
  eventName: string,
  rows: readonly JsonRecord[],
): JsonRecord[] {
  if (eventName !== "pull_request") return [...rows];
  return rows.filter((row) => row.runner === "ubuntu-26.04" && row.pg_major === "18");
}

/**
 * A missing output, malformed JSON or an empty include list never counts as a
 * successful matrix. The expected rows come from the checked-out rust.yml
 * catalog, which is read only after the output itself is well formed.
 * `loadRustWorkflow` returns the parsed rust.yml mapping, or undefined when the
 * file is missing or does not parse.
 */
export function postgresMatrixGateError(
  workflow: Workflow,
  plan: Plan,
  outputs: Readonly<Record<string, string>>,
  eventName: string,
  loadRustWorkflow: () => unknown,
): string | null {
  if (workflow !== "rust" || plan.jobs.postgres?.selected !== true) return null;
  const raw = outputs.postgres_matrix;
  if (raw === undefined || isBlank(raw)) return "POSTGRES_MATRIX_MISSING";
  const parsed = parseJson(raw);
  if (!parsed.ok) return "POSTGRES_MATRIX_MALFORMED";
  const matrix = parsed.value;
  if (
    !isRecord(matrix) ||
    Object.keys(matrix).length !== 1 ||
    !Array.isArray(matrix.include) ||
    matrix.include.length === 0 ||
    !(matrix.include as unknown[]).every((row) => isRecord(row) && Object.keys(row).length > 0)
  ) {
    return "POSTGRES_MATRIX_EMPTY";
  }
  if (!KNOWN_EVENTS.has(eventName)) return "POSTGRES_MATRIX_EVENT";
  const rows = catalogFromRustWorkflow(loadRustWorkflow());
  if (rows === null) return "POSTGRES_MATRIX_CATALOG";
  const expected = postgresMatrixInclude(eventName, rows);
  const include = matrix.include as unknown[];
  if (include.length !== expected.length) return "POSTGRES_MATRIX_ROW_COUNT";
  // Row order is significant; key order inside a row is not.
  if (!Bun.deepEquals(include, expected, true)) return "POSTGRES_MATRIX_ROWS";
  return null;
}
