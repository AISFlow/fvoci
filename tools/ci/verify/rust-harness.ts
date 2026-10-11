// rust.yml harness policy: jobs run the prebuilt xtask or Bun, never Python,
// and the fast job keeps the xtask clippy and Bun encryption-key checks.
import { type Mapping, field, same, steps, str } from "./rust-common.ts";

// The only Python left in rust.yml: the fast job's selected-backend script
// tests, which run on the runner image's python3.
const FAST_PYTHON_LINES: ReadonlySet<string> = new Set([
  "python3 scripts/selected-backend-ci/test_early_driver_failure.py",
  "python3 scripts/selected-backend-ci/test_restart_ledger.py",
  "python3 scripts/selected-backend-ci/test_current_binding_engine_features.py",
]);
const XTASK_WIRING_STEP = "Pinned SQLite build wiring (xtask, no native compilation)";
const FAST_HARNESS_STEPS = [
  { run: "cargo clippy --locked --manifest-path xtask/Cargo.toml --all-targets -- -D warnings" },
  { run: "bun tools/oracle/encryption-keys.ts self-test" },
];

const python = (text: string) => /python/i.test(text);

/** No job runs Python except the fast job's listed lines. */
export function pythonErrors(jobs: Mapping): string[] {
  const errors: string[] = [];
  const refuse = (name: string, what: string) =>
    errors.push(`rust: ${name} must run xtask or Bun, not Python: ${what}`);
  for (const [name, job] of Object.entries(jobs)) {
    const shell = str(field(field(job, "defaults"), "run"), "shell");
    if (python(shell)) refuse(name, shell);
    for (const step of steps(job)) {
      const lines = str(step, "run").split("\n");
      const found = lines.find(
        (line) => python(line) && !(name === "fast" && FAST_PYTHON_LINES.has(line.trim())),
      );
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

export function verifyRustHarness(jobs: Mapping): string[] {
  return [...pythonErrors(jobs), ...fastHarnessErrors(jobs)];
}
