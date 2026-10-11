// rust.yml harness policy: jobs run the prebuilt xtask or Bun, never Python,
// and the fast job keeps the xtask clippy and Bun encryption-key checks.
import { type Mapping, field, same, steps, str } from "./rust-common.ts";

const XTASK_WIRING_STEP = "Pinned SQLite build wiring (xtask, no native compilation)";
const FAST_HARNESS_STEPS = [
  { run: "cargo clippy --locked --manifest-path xtask/Cargo.toml --all-targets -- -D warnings" },
  { run: "bun tools/oracle/encryption-keys.ts self-test" },
];
// The fast job runs no `bun ci`, so its selected-backend list holds only the
// tests that need no node_modules (drivers/sqlite.test.ts needs Playwright).
// web-checks runs the whole directory after `bun ci`.
const FAST_SCRIPT_TESTS_STEP = "Script unit tests without a database";
const FAST_SCRIPT_TESTS = [
  "bun test scripts/schema-baseline/compare-catalogs.test.ts",
  "bun test " +
    [
      "lane-controls.test.ts",
      "drivers/binding.test.ts",
      "drivers/common.test.ts",
      "drivers/restart.test.ts",
      "drivers/postgres.test.ts",
      "drivers/install.test.ts",
    ]
      .map((file) => "./tools/selected-backend-ci/" + file)
      .join(" "),
];

const python = (text: string) => /python/i.test(text);

/** No job runs Python. */
export function pythonErrors(jobs: Mapping): string[] {
  const errors: string[] = [];
  const refuse = (name: string, what: string) =>
    errors.push(`rust: ${name} must run xtask or Bun, not Python: ${what}`);
  for (const [name, job] of Object.entries(jobs)) {
    const shell = str(field(field(job, "defaults"), "run"), "shell");
    if (python(shell)) refuse(name, shell);
    for (const step of steps(job)) {
      const found = str(step, "run").split("\n").find(python);
      if (found !== undefined) refuse(name, found);
      for (const key of ["shell", "uses"]) {
        if (python(str(step, key))) refuse(name, str(step, key));
      }
    }
  }
  return errors;
}

/** The fast job runs each harness step once, after the xtask tests. */
export function fastHarnessErrors(jobs: Mapping): string[] {
  const fast = steps(jobs.fast);
  const wiring = fast.findIndex((step) => step.name === XTASK_WIRING_STEP);
  return FAST_HARNESS_STEPS.flatMap((step) => {
    const at = fast.flatMap((item, index) => (same(item, step) ? [index] : []));
    return at.length === 1 && wiring >= 0 && (at[0] ?? -1) > wiring
      ? []
      : [`rust: fast must run "${step.run}" once after the xtask tests`];
  });
}

/** The fast job runs exactly the pinned script tests, unmasked. */
export function fastScriptTestErrors(jobs: Mapping): string[] {
  const found = steps(jobs.fast).filter((step) => step.name === FAST_SCRIPT_TESTS_STEP);
  const step = found[0];
  return found.length === 1 &&
    step !== undefined &&
    same(Object.keys(step).sort(), ["name", "run"]) &&
    same(str(step, "run").split("\n"), [...FAST_SCRIPT_TESTS, ""])
    ? []
    : [`rust: fast "${FAST_SCRIPT_TESTS_STEP}" must run exactly the pinned script tests`];
}

export function verifyRustHarness(jobs: Mapping): string[] {
  return [...pythonErrors(jobs), ...fastHarnessErrors(jobs), ...fastScriptTestErrors(jobs)];
}
