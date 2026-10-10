// Selected SQLite library cohort: the exact filters the fast job must execute and
// the per-test result rule. A zero-match Cargo run or an ignored test is not execution.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  type Mapping,
  type VerifyContext,
  PY_NON_WS,
  field,
  indexOfSame,
  rustJobs,
  same,
  steps,
} from "./rust-common.ts";

export const RUST_SELECTED_LIBRARY_STEP = "Selected SQLite library controls (54 exact tests)";
// The list `xtask selected-library` runs, one exact libtest name per LF-terminated
// line. Order is part of the contract: the slice pins below bind each accepted cohort.
export const RUST_SELECTED_LIBRARY_FILTERS_FILE = "xtask/selected-library-filters.txt";
const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** The filter file under `root`, or null when it is unreadable, not UTF-8 or not LF-terminated. */
export function readSelectedLibraryFilters(root: string): string[] | null {
  let text: string;
  try {
    text = UTF8.decode(readFileSync(join(root, RUST_SELECTED_LIBRARY_FILTERS_FILE)));
  } catch {
    return null;
  }
  return text.endsWith("\n") ? text.slice(0, -1).split("\n") : null;
}

// The verifier checkout's own list; the workflow check reads the tree it verifies.
export const RUST_SELECTED_LIBRARY_FILTERS: readonly string[] =
  readSelectedLibraryFilters(resolve(import.meta.dir, "../../..")) ?? [];

// [name, start, end) slices of the table and the sha256 of their "\n"-joined UTF-8 text.
export const RUST_SELECTED_LIBRARY_SLICE_PINS: readonly (readonly [
  string,
  number,
  number,
  string,
])[] = [
  ["original18", 0, 18, "f9221b4d6b32402a3e125643d3fc67dfb600df8ef27b8e76013bb8b46ade7c80"],
  ["get10", 18, 28, "157c4dc2ec55fbece3f7aeb85433c6e6552c8e2e91075f191dea15cab8c37e86"],
  ["metadata5", 28, 33, "8c8191b4b26800b5ed8980bc75e8c3ac6b1f6df55611dc9a3665c16db139ae46"],
  ["aux2", 33, 35, "7e7702addc4c016b2e885adb2447d0324da4c857dfa874fb8bc0e9dddc88a71d"],
  ["original35", 0, 35, "479c869c1734119e7ee091d490dbda12d1df5f59fcd73eea78478649f23b1c87"],
  ["bootstrap9", 35, 44, "1baee892b0bc67c4289d4e6ee57b801963d08793f9107f99f72d0e41bc7a18a0"],
  ["original44", 0, 44, "b1d7fff1c44346a300454eff3b0721e1a2078842308a1847b9e9411e23741cb1"],
  ["member10", 44, 54, "a77024442b3b63a39a70012a251fe5ed4948543b8a10156a09ace6d31d7ff09c"],
  ["all54", 0, 54, "aa10daa88b6f9860b43bfb628edb535f185cd4dacd7f1faad27f7d2178ab7498"],
];

// The maintained step body, byte-exact (the YAML block scalar adds the final newline).
export const RUST_SELECTED_LIBRARY_RUN = `set -euo pipefail
python3 - <<'PYLIB'
import subprocess
from scripts.ci_selection import RUST_SELECTED_LIBRARY_FILTERS, selected_library_result_error

for test_filter in RUST_SELECTED_LIBRARY_FILTERS:
    result = subprocess.run(
        ["cargo", "test", "--locked", "--offline", "--features", "db-tests", "--lib", test_filter, "--", "--exact"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    print(result.stdout, end="", flush=True)
    error = selected_library_result_error(test_filter, result.returncode, result.stdout)
    if error:
        raise SystemExit(error)
PYLIB`;
export const RUST_DEFAULT_LIBRARY_STEP: Mapping = {
  run: "cargo test --locked --offline --lib --bin fvoci-server",
  env: { CARGO_TARGET_DIR: "target/default" },
};

const sha256 = (text: string) => createHash("sha256").update(text, "utf8").digest("hex");

/** True when the filters keep all 54 unique entries and every slice pin. */
export function selectedLibraryRegistryIntact(
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): boolean {
  return (
    filters.length === 54 &&
    new Set(filters).size === 54 &&
    RUST_SELECTED_LIBRARY_SLICE_PINS.every(
      ([, start, end, pin]) => sha256(filters.slice(start, end).join("\n")) === pin,
    )
  );
}

// Python re.MULTILINE semantics: lines end at "\n" only and "." also spans "\r".
const runningLine = /^running ([0-9]+) tests?$/s;
const testLine = new RegExp(`^test (${PY_NON_WS}+) \\.\\.\\. (${PY_NON_WS}+).*$`, "s");
const summaryLine = /^test result: (.*)$/s;
const passedSummary =
  /^ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+(?:\.[0-9]+)?s$/;

/** Accept exactly the requested, nonignored library test, never compilation. */
export function selectedLibraryResultError(
  testFilter: string,
  returncode: number,
  output: string,
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): string | null {
  if (!filters.includes(testFilter)) return "rust: unregistered selected library filter";
  if (returncode !== 0) return "rust: selected library test command failed";
  const lines = output.split("\n");
  const counts = lines.flatMap((line) => runningLine.exec(line)?.[1] ?? []);
  if (counts.length !== 1 || counts[0] !== "1") {
    return "rust: selected library command must run exactly one test";
  }
  const executed = lines.flatMap((line) => {
    const match = testLine.exec(line);
    return match ? [[match[1], match[2]]] : [];
  });
  if (!same(executed, [[testFilter, "ok"]])) {
    return "rust: selected library result must be the exact requested test and ok";
  }
  const summaries = lines.flatMap((line) => summaryLine.exec(line)?.[1] ?? []);
  if (summaries.length !== 1 || !passedSummary.test(summaries[0] ?? "")) {
    return "rust: selected library result must pass one test without failure or ignore";
  }
  return null;
}

/** Bind the maintained step, its environment and the exact 54-filter cohort. */
export function verifySelectedLibraryExecution(
  jobs: Mapping,
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): string[] {
  const errors: string[] = [];
  if (!selectedLibraryRegistryIntact(filters)) {
    errors.push(
      "rust: selected library registry must retain all54 exact filters (original44 prefix and member10)",
    );
  }
  const fast = jobs.fast;
  const list = steps(fast);
  const matches = list.filter((step) => step.name === RUST_SELECTED_LIBRARY_STEP);
  if (matches.length !== 1) {
    errors.push("rust: selected library step must appear exactly once in fast");
    return errors;
  }
  const expected = {
    name: RUST_SELECTED_LIBRARY_STEP,
    env: { CARGO_TARGET_DIR: "target/db-lib" },
    run: RUST_SELECTED_LIBRARY_RUN + "\n",
  };
  if (!same(matches[0], expected)) {
    errors.push(
      "rust: selected library step must keep exact unconditional command without extra env or flags",
    );
  }
  const plain = list.flatMap((step, index) =>
    same(step, RUST_DEFAULT_LIBRARY_STEP) ? [index] : [],
  );
  if (plain.length !== 1 || (plain[0] ?? 0) >= indexOfSame(list, matches[0])) {
    errors.push(
      "rust: selected library step must preserve preceding default plain library command",
    );
  }
  if (field(fast, "env") !== undefined) {
    errors.push("rust: selected library fast job must not add an environment override");
  }
  return errors;
}

export function verifySelectedLibraryExecutionCtx(ctx: VerifyContext): string[] {
  const jobs = rustJobs(ctx);
  if (!jobs) return [];
  const filters = readSelectedLibraryFilters(ctx.root);
  if (filters) return verifySelectedLibraryExecution(jobs, filters);
  return [
    `rust: selected library filters must be the LF-terminated UTF-8 file ${RUST_SELECTED_LIBRARY_FILTERS_FILE}`,
    ...verifySelectedLibraryExecution(jobs, []),
  ];
}
