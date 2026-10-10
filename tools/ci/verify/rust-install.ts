// The privileged SQLite install fixture runs on both PG18 B runners from the
// validated production helper artifact, never from a rebuild.
import { get, has, pyEq, pyStrip, sha256Hex, type Mapping } from "./py.ts";
import {
  RUST_POSTGRES_RUNNER_ARCH,
  RUST_SELECTED_INSTALL_IF,
  RUST_SELECTED_INSTALL_RUN_SHA256,
  RUST_SELECTED_INSTALL_STEP,
  RUST_SELECTED_INSTALL_TARGET,
  postgresJobSteps,
  postgresMatrixRows,
  uniqueNamedStep,
  type Result,
} from "./rust-shared.ts";

const HELPER_STEP = "Validate and restore finished postgres executables (no rebuild fallback)";
const HELPER_RUN =
  'python3 scripts/ci_selection.py rust-binaries unpack --cohort postgres --directory "$RUNNER_TEMP/rust-binaries" --sqlite-identity "${{ steps.sqlite.outputs.cache_identity }}"';
const EXPECTED_ENV = {
  FVOCI_COLLAB_ENGINE: "${{ github.workspace }}/crates/collab-engine/target/debug/collab-engine",
};

export function selectedInstallInventory(jobs: Mapping): Result<Set<string>> {
  const [steps, err] = postgresJobSteps(jobs);
  if (err !== null) return [null, err];
  const [step, stepErr] = uniqueNamedStep(steps, RUST_SELECTED_INSTALL_STEP, "postgres");
  if (stepErr !== null) return [null, stepErr];
  if (has(step, "continue-on-error") || get(step, "if") !== RUST_SELECTED_INSTALL_IF) {
    return [null, "rust: selected install step must execute on PG18 B without error masking"];
  }
  const run = get(step, "run");
  if (
    !pyEq(get(step, "env"), EXPECTED_ENV) ||
    typeof run !== "string" ||
    sha256Hex(pyStrip(run)) !== RUST_SELECTED_INSTALL_RUN_SHA256
  ) {
    return [
      null,
      "rust: selected install step must keep exact db-tests build, root inputs, unfiltered execution and count gate",
    ];
  }
  const [helper, helperErr] = uniqueNamedStep(steps, HELPER_STEP, "postgres");
  if (helperErr !== null) return [null, helperErr];
  // list.index() finds the first equal element, as the original compares.
  const position = (target: Mapping) => steps.findIndex((item) => pyEq(item, target));
  if (
    has(helper, "if") ||
    has(helper, "continue-on-error") ||
    get(helper, "run") !== HELPER_RUN ||
    position(helper) >= position(step)
  ) {
    return [
      null,
      "rust: selected install requires preceding mandatory validated production helper artifact",
    ];
  }
  const [rows, rowsErr] = postgresMatrixRows(get(jobs, "postgres") as Mapping);
  if (rowsErr !== null) return [null, rowsErr];
  const runners = rows
    .filter((row) => get(row, "shard") === "b" && get(row, "pg_major") === "18")
    .map((row) => get(row, "runner"));
  const expected = Object.keys(RUST_POSTGRES_RUNNER_ARCH);
  const sameSet =
    runners.every((runner) => typeof runner === "string" && expected.includes(runner)) &&
    expected.every((runner) => runners.includes(runner));
  if (!sameSet || runners.length !== 2) {
    return [null, "rust: selected install requires exactly one PG18 B execution on x64 and arm64"];
  }
  return [new Set([RUST_SELECTED_INSTALL_TARGET]), null];
}
