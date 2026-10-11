import { get, has, isMapping, pyReprList, triggersOf, type Mapping } from "./load.ts";

// Manual opt-in jobs: no path or event policy selects them (not even full mode
// or a fatal plan). Only a workflow_dispatch whose boolean input of the same
// name is exactly true selects the job.
export const OPT_IN_JOBS: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  install: { "upgrade-smoke-arm64": "run_upgrade_smoke_arm" },
};
export const OPT_IN_RUNNER: Readonly<Record<string, string>> = {
  "upgrade-smoke-arm64": "ubuntu-26.04-arm",
};

/** workflow_dispatch declares exactly the opt-in booleans (default false). */
export function verifyOptInWiring(workflow: string, data: Mapping, jobs: Mapping): string[] {
  const errors: string[] = [];
  const triggers = triggersOf(data);
  if (!isMapping(triggers) || !has(triggers, "workflow_dispatch")) {
    return [`${workflow}: workflow_dispatch trigger missing`];
  }
  const dispatch = get(triggers, "workflow_dispatch");
  if (dispatch !== null && !isMapping(dispatch)) {
    return [`${workflow}: workflow_dispatch must be a mapping`];
  }
  const inputs = dispatch === null ? undefined : get(dispatch, "inputs");
  if (inputs !== undefined && inputs !== null && !isMapping(inputs)) {
    return [`${workflow}: workflow_dispatch inputs must be a mapping`];
  }
  const optIns = (Object.hasOwn(OPT_IN_JOBS, workflow) ? OPT_IN_JOBS[workflow] : undefined) ?? {};
  const expected = [...new Set(Object.values(optIns))].sort();
  const declared = isMapping(inputs) ? Object.keys(inputs) : [];
  if (declared.length !== expected.length || !expected.every((name) => declared.includes(name))) {
    return [`${workflow}: workflow_dispatch inputs must be exactly ${pyReprList(expected)}`];
  }
  for (const name of expected) {
    const spec = get(inputs, name);
    if (!isMapping(spec) || get(spec, "type") !== "boolean" || get(spec, "default") !== false) {
      errors.push(`${workflow}: input ${name} must be type boolean with default false`);
    }
  }
  for (const job of Object.keys(optIns)) {
    const spec = get(jobs, job);
    if (!isMapping(spec)) continue;
    if (get(spec, "runs-on") !== OPT_IN_RUNNER[job]) {
      errors.push(`${workflow}: ${job} runs-on must be ${String(OPT_IN_RUNNER[job])}`);
    }
    if (has(spec, "strategy")) {
      errors.push(`${workflow}: ${job} must be a single job without a matrix`);
    }
  }
  return errors;
}
