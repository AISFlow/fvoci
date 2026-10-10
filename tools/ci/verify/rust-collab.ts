// The collaboration job runs the maintained script on both architectures; the
// script's literal `cargo test ... --test X \` lines are its target inventory.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  get,
  has,
  isMapping,
  pyRepr,
  pyRstripChar,
  pySplit,
  pySplitlines,
  pyStrip,
  sortedPy,
  intersect,
  type Mapping,
} from "./py.ts";
import { validateCargoTestInvocation } from "./rust-exec.ts";
import {
  RUST_COLLAB_CI_SCRIPT,
  RUST_COLLAB_INTEGRATION_RUN,
  RUST_COLLAB_INTEGRATION_STEP,
  RUST_COLLAB_MATRIX_RUNNERS,
  cargoCommandSuppressionError,
  cargoTestFlagsInText,
  executionStepMasked,
  isFile,
  normalizeRunScript,
  uniqueNamedStep,
  type Result,
} from "./rust-shared.ts";

// libtest scheduling the script may pass after `--`: never a filter, skip or
// ignored selection.
export const RUST_COLLAB_LIBTEST_ARGS: ReadonlySet<string> = new Set([
  "--nocapture",
  "--test-threads=1",
]);

export function verifyCollaborationWorkflowExecution(jobs: Mapping): string[] {
  const job = get(jobs, "collaboration");
  if (!isMapping(job)) return ["rust: collaboration job missing"];
  const strategy = get(job, "strategy");
  if (!isMapping(strategy)) return ["rust: collaboration job strategy missing"];
  const matrix = get(strategy, "matrix");
  if (!isMapping(matrix)) return ["rust: collaboration job matrix missing"];
  const include = get(matrix, "include");
  if (!Array.isArray(include) || include.length === 0)
    return ["rust: collaboration job matrix.include missing"];
  const runners = new Set<string>();
  for (const row of include) {
    if (!isMapping(row)) return ["rust: collaboration matrix.include row must be a mapping"];
    const runner = get(row, "runner");
    if (typeof runner !== "string") return ["rust: collaboration matrix row missing runner"];
    runners.add(runner);
  }
  const missing = sortedPy(
    [...RUST_COLLAB_MATRIX_RUNNERS].filter((runner) => !runners.has(runner)),
  );
  if (missing.length > 0)
    return ["rust: collaboration matrix missing runners: " + missing.join(", ")];
  const steps = get(job, "steps");
  if (!Array.isArray(steps)) return ["rust: collaboration job steps missing"];
  const name = RUST_COLLAB_INTEGRATION_STEP;
  const [step, stepErr] = uniqueNamedStep(steps, name, "collaboration");
  if (stepErr !== null) return [stepErr];
  const masked = executionStepMasked(step, "collaboration", name);
  if (masked) return [masked];
  if (has(step, "if"))
    return ["rust: collaboration integration step must not have an if condition"];
  const run = get(step, "run");
  if (typeof run !== "string")
    return [`rust: collaboration step ${pyRepr(name)} must have a string run command`];
  if (normalizeRunScript(run) !== normalizeRunScript(RUST_COLLAB_INTEGRATION_RUN)) {
    return [`rust: collaboration integration step must execute ${RUST_COLLAB_INTEGRATION_RUN}`];
  }
  return [];
}

// Every `cargo test` line with its `--test` continuation lines and an
// optional final `-- ...` libtest line, joined with single spaces.
export function collaborationScriptCargoCommands(text: string): string[] {
  const lines = pySplitlines(text);
  const continuation = (line: string) => pyStrip(pyRstripChar(line, "\\"));
  const commands: string[] = [];
  lines.forEach((line, index) => {
    const stripped = pyStrip(line);
    if (!stripped.startsWith("cargo test ")) return;
    const parts = [continuation(stripped)];
    for (let next = index + 1; next < lines.length; next += 1) {
      const candidate = pyStrip(lines[next] as string);
      if (candidate.startsWith("--test ")) {
        parts.push(continuation(candidate));
        continue;
      }
      if (candidate.startsWith("-- ")) parts.push(continuation(candidate));
      break;
    }
    commands.push(parts.join(" "));
  });
  return commands;
}

export function collaborationScriptInventoryFromText(text: string): Result<Set<string>> {
  const context = "collaboration CI script cargo test";
  const commands = collaborationScriptCargoCommands(text);
  if (commands.length === 0)
    return [null, "rust: collaboration CI script missing cargo test invocation"];
  const tests = new Set<string>();
  for (const command of commands) {
    const suppression = cargoCommandSuppressionError(command, context, RUST_COLLAB_LIBTEST_ARGS);
    if (suppression) return [null, suppression];
    const at = command.indexOf(" -- ");
    const cargoArgs = at === -1 ? command : command.slice(0, at);
    const shapeErr = validateCargoTestInvocation(pySplit(cargoArgs), context, true);
    if (shapeErr) return [null, shapeErr];
    const invocationTests = cargoTestFlagsInText(cargoArgs);
    const repeated = sortedPy(intersect(tests, invocationTests));
    if (repeated.length > 0) {
      return [
        null,
        "rust: collaboration CI script runs --test targets more than once: " + repeated.join(", "),
      ];
    }
    for (const name of invocationTests) tests.add(name);
  }
  if (tests.size === 0) return [null, "rust: collaboration CI script declares no --test targets"];
  return [tests, null];
}

export function collaborationScriptInventory(root: string): Result<Set<string>> {
  const path = join(root, RUST_COLLAB_CI_SCRIPT);
  if (!isFile(path))
    return [null, `rust: missing collaboration CI script ${RUST_COLLAB_CI_SCRIPT}`];
  return collaborationScriptInventoryFromText(readFileSync(path, "utf8"));
}
