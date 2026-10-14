// Bind the exact lib cohorts to existing jobs that own their required inputs.
// The runner and this verifier read the same registry; the pin prevents an
// unknown/removed/duplicated selector from silently changing CI coverage.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { type Mapping, same, steps, str } from "./rust-common.ts";
import { RUST_XTASK } from "./rust-shared.ts";

export const NATIVE_LIBRARY_FILE = "xtask/native-library-cohorts.tsv";
const REGISTRY_SHA256 = "064f2d817aa95b237278139d81f4bf38b416ac7462c8399c042e7902a794d1de";
const DIRECTORY =
  ' --directory "$RUNNER_TEMP/rust-binaries" --sqlite-identity "${{ steps.sqlite.outputs.cache_identity }}"';
const DATABASE =
  "postgres://postgres:ci-ephemeral-only@127.0.0.1:${{ job.services.postgres.ports['5432'] }}/postgres";
const HELPER = "${{ github.workspace }}/crates/collab-engine/target/debug/collab-engine";
const SERVER = "${{ github.workspace }}/target/db-tests/debug/fvoci-server";
const SQLD = "${{ runner.temp }}/native-library-sqld/sqld";

export const NATIVE_LIBRARY_STEPS: readonly (readonly [string, Mapping])[] = [
  [
    "fast-sqlite",
    {
      name: "Additional SQLite library controls (105 exact tests)",
      run: RUST_XTASK + " selected-library --cohort no-input --target-dir target/db-lib",
    },
  ],
  [
    "postgres",
    {
      name: "Native PostgreSQL library control (1 exact test)",
      if: "matrix.shard == 'a'",
      env: { TEST_DATABASE_URL: DATABASE },
      run: RUST_XTASK + " native-library run --cohort postgres" + DIRECTORY,
    },
  ],
  [
    "postgres",
    {
      name: "Restricted PostgreSQL app library control (1 exact test)",
      if: "matrix.shard == 'a'",
      env: { PG_CONTAINER: "${{ job.services.postgres.id }}", PREPARATION_DATABASE_URL: DATABASE },
      run: RUST_XTASK + " native-library run --cohort pg-app" + DIRECTORY,
    },
  ],
  [
    "postgres",
    {
      name: "Native S3 library control (1 exact test)",
      if: "matrix.shard == 'b'",
      run:
        "bash scripts/start-test-minio.sh " +
        RUST_XTASK +
        " native-library run --cohort s3" +
        DIRECTORY,
    },
  ],
  [
    "collaboration",
    {
      name: "Native collaboration library controls (28 exact tests)",
      env: {
        TEST_DATABASE_URL: DATABASE,
        FVOCI_COLLAB_ENGINE: HELPER,
        FVOCI_IMPORT_TEST_SERVER_BIN: SERVER,
      },
      run: RUST_XTASK + " native-library run --cohort collaboration" + DIRECTORY,
    },
  ],
  [
    "collaboration",
    {
      name: "Native preview library controls (6 exact tests)",
      env: { FVOCI_PREVIEW_TEST_HELPER: SERVER },
      run: RUST_XTASK + " native-library run --cohort preview" + DIRECTORY,
    },
  ],
  [
    "collaboration",
    {
      name: "Prepare pinned official native sqld",
      run:
        RUST_XTASK + ' native-library prepare-sqld --directory "$RUNNER_TEMP/native-library-sqld"',
    },
  ],
  ...(
    [
      ["sqld", "Official sqld library controls (15 exact tests)"],
      ["sqld-ignored", "Explicit official sqld library controls (15 exact tests)"],
      ["sqld-migration", "Explicit local sqld migration controls (6 exact tests)"],
    ] as const
  ).map(
    ([cohort, name]) =>
      [
        "collaboration",
        {
          name,
          env:
            cohort === "sqld-ignored"
              ? { FVOCI_TEST_SQLD: SQLD, FVOCI_COLLAB_ENGINE: HELPER }
              : { FVOCI_TEST_SQLD: SQLD },
          run: RUST_XTASK + " native-library run --cohort " + cohort + DIRECTORY,
        },
      ] as const,
  ),
];

export function verifyNativeLibraryExecution(jobs: Mapping, root: string): string[] {
  const errors: string[] = [];
  try {
    const bytes = readFileSync(join(root, NATIVE_LIBRARY_FILE));
    if (createHash("sha256").update(bytes).digest("hex") !== REGISTRY_SHA256)
      throw new Error("changed");
  } catch {
    errors.push("rust: native library exact cohort registry must retain all178 selectors");
  }
  for (const [owner, job] of Object.entries(jobs)) {
    for (const step of steps(job)) {
      const run = str(step, "run");
      if (
        (run.includes(" native-library ") || run.includes(" selected-library --cohort ")) &&
        !NATIVE_LIBRARY_STEPS.some(([job, expected]) => owner === job && same(step, expected))
      ) {
        errors.push("rust: native library unregistered or duplicate execution is forbidden");
      }
    }
  }
  for (const [job, expected] of NATIVE_LIBRARY_STEPS) {
    const found = Object.entries(jobs).flatMap(([owner, value]) =>
      steps(value)
        .filter((step) => step.name === expected.name)
        .map((step) => ({ owner, step })),
    );
    if (found.length !== 1 || found[0]?.owner !== job || !same(found[0].step, expected)) {
      errors.push(
        `rust: native library ${String(expected.name)} must execute exactly once with its required inputs in ${job}`,
      );
      continue;
    }
    const list = steps(jobs[job]);
    const at = list.indexOf(found[0].step);
    const before =
      job === "fast-sqlite"
        ? "Selected SQLite library controls (56 exact tests)"
        : `Validate and restore finished ${job === "postgres" ? "postgres" : "helper"} executables (no rebuild fallback)`;
    const validated = list.findIndex((step) => step.name === before);
    if (validated < 0 || at <= validated)
      errors.push("rust: native library execution must follow validated input preparation");
    if (String(expected.run).includes("--cohort sqld")) {
      const prepare = list.findIndex((step) => step.name === "Prepare pinned official native sqld");
      if (prepare < 0 || at <= prepare)
        errors.push("rust: native library sqld execution must follow pinned official preparation");
    }
  }
  return errors;
}
