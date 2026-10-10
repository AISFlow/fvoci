// The default ARM64 build/policy check stays mandatory in its own job after the
// split from the PostgreSQL matrix.
import { get, has, isMapping, pyEq, pyStrip, type Mapping } from "./py.ts";
import { RUST_NATIVE_ARM64_RUN, RUST_NATIVE_ARM64_STEP, runSteps } from "./rust-shared.ts";

export function verifyNativeArm64Execution(jobs: Mapping): string[] {
  const job = get(jobs, "native-arm64");
  if (!isMapping(job)) return ["rust: native-arm64 job missing"];
  const errors: string[] = [];
  if (get(job, "runs-on") !== "ubuntu-26.04-arm" || has(job, "strategy")) {
    errors.push("rust: native-arm64 must run once on ubuntu-26.04-arm");
  }
  if (!pyEq(get(job, "timeout-minutes"), 15)) {
    errors.push("rust: native-arm64 must keep the 15 minute budget");
  }
  if (has(job, "continue-on-error"))
    errors.push("rust: native-arm64 must fail on build/policy errors");
  const steps = runSteps(job).filter((step) => get(step, "name") === RUST_NATIVE_ARM64_STEP);
  const only = steps[0];
  if (steps.length !== 1 || only === undefined) {
    errors.push("rust: native-arm64 must execute the default build/policy step exactly once");
  } else if (
    pyStrip(get(only, "run") as string) !== RUST_NATIVE_ARM64_RUN ||
    has(only, "if") ||
    has(only, "continue-on-error") ||
    has(only, "env")
  ) {
    errors.push(
      "rust: native-arm64 must execute the exact unconditional default build/policy commands",
    );
  }
  if (
    runSteps(get(jobs, "postgres")).some((step) => get(step, "name") === RUST_NATIVE_ARM64_STEP)
  ) {
    errors.push("rust: default ARM build/policy must not share the PostgreSQL job budget");
  }
  return errors;
}
