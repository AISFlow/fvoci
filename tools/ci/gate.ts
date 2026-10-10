// Fail-closed required gate for one CI workflow.
//
//   bun tools/ci/gate.ts --workflow <name> [--needs-json <json>] --tested-sha <sha>
//
// --needs-json falls back to NEEDS_JSON. GITHUB_EVENT_NAME and GITHUB_EVENT_PATH
// describe this run's event. Prints `gate: ok` and exits 0 when the run passes;
// otherwise prints `gate: <reason>` to stderr and exits 1. Usage errors exit 2.

import { readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { evaluateGate } from "./gate/evaluate.ts";
import { RUST_WORKFLOW_FILE } from "./gate/matrix.ts";
import type { EventFile } from "./gate/opt-in.ts";
import { WORKFLOWS, isWorkflow, type Workflow } from "./gate/schema.ts";
import { strip } from "./gate/text.ts";

const ROOT = resolve(import.meta.dir, "../..");
const USAGE = `usage: gate.ts [-h] --workflow {${WORKFLOWS.join(",")}} [--needs-json NEEDS_JSON] --tested-sha TESTED_SHA`;

type Args = { workflow: Workflow; needsJson: string | undefined; testedSha: string };
type Parsed = { kind: "args"; args: Args } | { kind: "help" } | { kind: "error"; message: string };

const OPTIONS = {
  "--workflow": "workflow",
  "--needs-json": "needsJson",
  "--tested-sha": "testedSha",
};
type OptionName = keyof typeof OPTIONS;

const LONG_OPTIONS = [...Object.keys(OPTIONS), "--help"];

// The planner's argparse CLI accepts a unique prefix of a long option.
function resolveOption(name: string): string | { error: string } | null {
  if (name === "-h" || LONG_OPTIONS.includes(name)) return name;
  if (!name.startsWith("--") || name === "--") return null;
  const matches = LONG_OPTIONS.filter((option) => option.startsWith(name));
  if (matches.length > 1) {
    return { error: `ambiguous option: ${name} could match ${matches.join(", ")}` };
  }
  return matches[0] ?? null;
}

// argparse refuses an option-like token as a value; negative numbers and
// tokens containing a space are values.
function looksLikeOption(token: string): boolean {
  return (
    token.startsWith("-") &&
    token !== "-" &&
    !/^-\d+$|^-\d*\.\d+$/.test(token) &&
    !token.includes(" ")
  );
}

export function parseArgs(argv: readonly string[]): Parsed {
  const values: Partial<Record<(typeof OPTIONS)[OptionName], string>> = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i] as string;
    const eq = arg.startsWith("--") ? arg.indexOf("=") : -1;
    const option = looksLikeOption(arg) ? resolveOption(eq > 0 ? arg.slice(0, eq) : arg) : null;
    if (option === null) return { kind: "error", message: `unrecognized arguments: ${arg}` };
    if (typeof option === "object") return { kind: "error", message: option.error };
    if (option === "-h" || option === "--help") return { kind: "help" };
    let value: string | undefined;
    if (eq > 0) value = arg.slice(eq + 1);
    else if (i + 1 < argv.length && !looksLikeOption(argv[i + 1] as string)) value = argv[++i];
    if (value === undefined) {
      return { kind: "error", message: `argument ${option}: expected one argument` };
    }
    values[OPTIONS[option as OptionName]] = value;
  }
  const missing = (["--workflow", "--tested-sha"] as const).filter(
    (name) => values[OPTIONS[name]] === undefined,
  );
  if (missing.length > 0) {
    return {
      kind: "error",
      message: `the following arguments are required: ${missing.join(", ")}`,
    };
  }
  const workflow = values.workflow as string;
  if (!isWorkflow(workflow)) {
    return {
      kind: "error",
      message: `argument --workflow: invalid choice: '${workflow}' (choose from ${WORKFLOWS.join(", ")})`,
    };
  }
  return {
    kind: "args",
    args: { workflow, needsJson: values.needsJson, testedSha: values.testedSha as string },
  };
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
