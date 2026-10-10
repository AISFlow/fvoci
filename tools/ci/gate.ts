// Fail-closed required gate for one CI workflow.
//
//   bun tools/ci/gate.ts --workflow <name> [--needs-json <json>] --tested-sha <sha>
//
// --needs-json falls back to NEEDS_JSON. GITHUB_EVENT_NAME and GITHUB_EVENT_PATH
// describe this run's event. Prints `gate: ok` and exits 0 when the run passes;
// otherwise prints `gate: <reason>` to stderr and exits 1. Usage errors exit 2.

import { readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgv, type OptionSpec } from "./argv.ts";
import { evaluateGate } from "./gate/evaluate.ts";
import { RUST_WORKFLOW_FILE } from "./gate/matrix.ts";
import type { EventFile } from "./gate/opt-in.ts";
import { WORKFLOWS, isWorkflow, type Workflow } from "./gate/schema.ts";
import { strip } from "./gate/text.ts";

const ROOT = resolve(import.meta.dir, "../..");
const USAGE = `usage: gate.ts [-h] --workflow {${WORKFLOWS.join(",")}} [--needs-json NEEDS_JSON] --tested-sha TESTED_SHA`;

type Args = { workflow: Workflow; needsJson: string | undefined; testedSha: string };
type Parsed = { kind: "args"; args: Args } | { kind: "help" } | { kind: "error"; message: string };

const OPTIONS: readonly OptionSpec[] = [
  { flag: "--workflow", dest: "workflow", required: true, choices: WORKFLOWS },
  { flag: "--needs-json", dest: "needsJson" },
  { flag: "--tested-sha", dest: "testedSha", required: true },
];

export function parseArgs(argv: readonly string[]): Parsed {
  const parsed = parseArgv(argv, OPTIONS);
  if (parsed.kind !== "values") return parsed;
  const { workflow, needsJson, testedSha } = parsed.values;
  // The parser enforces required options and choices.
  if (workflow === undefined || !isWorkflow(workflow) || testedSha === undefined) {
    throw new Error("parseArgv returned incomplete values");
  }
  return { kind: "args", args: { workflow, needsJson, testedSha } };
}

// Strict UTF-8 that keeps a byte order mark, so a BOM-prefixed file is malformed JSON.
const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

function readUtf8(path: string): string {
  return UTF8.decode(readFileSync(path));
}

function readEvent(env: NodeJS.ProcessEnv): EventFile {
  const path = strip(env.GITHUB_EVENT_PATH ?? "");
  if (!path) return { kind: "missing-path" };
  try {
    return { kind: "parsed", value: JSON.parse(readUtf8(path)) as unknown };
  } catch {
    return { kind: "malformed" };
  }
}

function loadRustWorkflow(root: string): unknown {
  const path = join(root, ".github", "workflows", RUST_WORKFLOW_FILE);
  try {
    if (!statSync(path).isFile()) return undefined;
    return Bun.YAML.parse(readUtf8(path));
  } catch {
    return undefined;
  }
}

export type GateRun = { code: number; stdout: string; stderr: string };

export function runGate(argv: readonly string[], env: NodeJS.ProcessEnv, root = ROOT): GateRun {
  const parsed = parseArgs(argv);
  if (parsed.kind === "help") return { code: 0, stdout: `${USAGE}\n`, stderr: "" };
  if (parsed.kind === "error") {
    return { code: 2, stdout: "", stderr: `${USAGE}\ngate.ts: error: ${parsed.message}\n` };
  }
  const { args } = parsed;
  const error = evaluateGate({
    workflow: args.workflow,
    needsJson: args.needsJson ?? env.NEEDS_JSON,
    testedSha: args.testedSha,
    eventName: strip(env.GITHUB_EVENT_NAME ?? ""),
    readEvent: () => readEvent(env),
    loadRustWorkflow: () => loadRustWorkflow(root),
  });
  if (error !== null) return { code: 1, stdout: "", stderr: `gate: ${error}\n` };
  return { code: 0, stdout: "gate: ok\n", stderr: "" };
}

if (import.meta.main) {
  const result = runGate(Bun.argv.slice(2), process.env);
  process.stdout.write(result.stdout);
  process.stderr.write(result.stderr);
  process.exitCode = result.code;
}
