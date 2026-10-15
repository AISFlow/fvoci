// Both architectures execute the sealed collaboration inventory shared with xtask.
import { join } from "node:path";
import { get, has, isMapping, pyRepr, sortedPy, type Mapping } from "./py.ts";
import {
  RUST_COLLAB_CI_SCRIPT,
  RUST_COLLAB_INTEGRATION_RUN,
  RUST_COLLAB_INTEGRATION_STEP,
  RUST_COLLAB_MATRIX_RUNNERS,
  executionStepMasked,
  isFile,
  notUtf8,
  readUtf8,
  normalizeRunScript,
  uniqueNamedStep,
  type Result,
} from "./rust-shared.ts";

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

// Retain the registry API while its source moves from shell to the shared TSV.
export function collaborationScriptInventoryFromText(text: string): Result<Set<string>> {
  const tests = new Set<string>();
  const suites: [string, string][] = [];
  for (const line of text
    .split(/\r?\n/)
    .filter(
      (line, index, lines) => !line.startsWith("#") && !(index === lines.length - 1 && line === ""),
    )) {
    const [mode, name, extra] = line.split("\t");
    if (
      !name ||
      extra !== undefined ||
      !/^[A-Za-z0-9_]+$/.test(name) ||
      (mode !== "parallel" && mode !== "serial")
    )
      return [null, "rust: invalid collaboration suite row"];
    if (tests.has(name)) return [null, "rust: duplicate collaboration suite: " + name];
    tests.add(name);
    suites.push([mode, name]);
  }
  if (
    suites.filter(([mode]) => mode === "parallel").length !== 9 ||
    suites.filter(([mode]) => mode === "serial").length !== 1 ||
    suites.at(-1)?.join("\t") !== "serial\ttask_collab_integration" ||
    !tests.has("collab_product")
  )
    return [null, "rust: collaboration requires nine parallel suites and the serial task suite"];
  return [tests, null];
}

export function collaborationScriptInventory(root: string): Result<Set<string>> {
  const path = join(root, RUST_COLLAB_CI_SCRIPT);
  if (!isFile(path))
    return [null, `rust: missing collaboration suite inventory ${RUST_COLLAB_CI_SCRIPT}`];
  const text = readUtf8(path);
  if (text === null) return [null, notUtf8(RUST_COLLAB_CI_SCRIPT)];
  return collaborationScriptInventoryFromText(text);
}
