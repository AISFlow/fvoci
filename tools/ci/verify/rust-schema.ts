// Schema baseline: two local SDK controls, the configured PG catalog tool and
// the PG app-role gate. The catalog uses an owned prepared DB/owner, not a
// normal-server credential.
import { get, has, pyEq, pyStrip, sha256Hex, type Mapping } from "./py.ts";
import {
  postgresJobSteps,
  postgresMatrixRows,
  uniqueNamedStep,
  type Result,
} from "./rust-shared.ts";

export const RUST_SCHEMA_BASELINE_STEP =
  "Schema baseline SQLite controls and prepared PostgreSQL catalog";
export const RUST_SCHEMA_BASELINE_RUN_SHA256 =
  "919415320553e025982e0a41708b202c1fa159a0019de7d3e3b100680ed59c52";
export const RUST_SCHEMA_BASELINE_TARGET = "schema_baseline_integration";

const EXPECTED_ENV = {
  PG_CONTAINER: "${{ job.services.postgres.id }}",
  PREPARATION_DATABASE_URL:
    "postgres://postgres:ci-ephemeral-only@127.0.0.1:${{ job.services.postgres.ports['5432'] }}/postgres",
};
const EXPECTED_A_ROWS = [
  ["ubuntu-26.04", "16"],
  ["ubuntu-26.04", "17"],
  ["ubuntu-26.04", "18"],
  ["ubuntu-26.04-arm", "18"],
]
  .map((pair) => JSON.stringify(pair))
  .sort();

export function schemaBaselineInventory(jobs: Mapping): Result<Set<string>> {
  const [steps, err] = postgresJobSteps(jobs);
  if (err !== null) return [null, err];
  const [step, stepErr] = uniqueNamedStep(steps, RUST_SCHEMA_BASELINE_STEP, "postgres");
  if (stepErr !== null) return [null, stepErr];
  const run = get(step, "run", "");
  if (
    get(step, "if") !== "matrix.shard == 'a'" ||
    has(step, "continue-on-error") ||
    !pyEq(get(step, "env"), EXPECTED_ENV) ||
    typeof run !== "string" ||
    sha256Hex(pyStrip(run)) !== RUST_SCHEMA_BASELINE_RUN_SHA256
  ) {
    return [
      null,
      "rust: schema baseline requires exact configured extraction, 2+1+1 actual controls and owned cleanup",
    ];
  }
  const [rows, rowsErr] = postgresMatrixRows(get(jobs, "postgres") as Mapping);
  if (rowsErr !== null) return [null, rowsErr];
  const actual = rows
    .filter((row) => get(row, "shard") === "a")
    .map((row) => [get(row, "runner"), get(row, "pg_major")]);
  const keys = actual.every((pair) => pair.every((part) => typeof part === "string"))
    ? actual.map((pair) => JSON.stringify(pair)).sort()
    : null;
  if (keys === null || keys.join("\n") !== EXPECTED_A_ROWS.join("\n")) {
    return [null, "rust: schema baseline requires PG16/17/18 x64 and PG18 arm64 A execution"];
  }
  return [new Set([RUST_SCHEMA_BASELINE_TARGET]), null];
}
