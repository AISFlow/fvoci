// The verdict of scripts/release-check-ci.sh: the latest check run of every CI
// gate (tools/ci/planner/registry.ts WORKFLOW_JOBS -> <workflow>-ci-gate) on
// the release commit concluded success.
//
//   bun tools/release/check-ci.ts < runs.tsv
//
// stdin holds one check run per line as `gh api --jq ... | @tsv` writes it:
// name, status, conclusion ("none" when null) and completed_at (else
// started_at, else empty; only a run that has not completed may lack a time).
// Prints one line per gate; exit 1 when a gate is not green or the input is
// malformed.
import { readFileSync } from "node:fs";
import { WORKFLOWS, gateJobId } from "../ci/planner/registry.ts";
import { Fail } from "./fail.ts";
import { decodeUtf8, repr } from "./py.ts";

export const GATES: readonly string[] = WORKFLOWS.map(gateJobId);

interface Run {
  at: string;
  status: string;
  conclusion: string;
}

const green = (run: Run) => run.status === "completed" && run.conclusion === "success";

// -1, 0 or 1 as run a is older than, as old as, or newer than run b. A run with
// no time has not started yet (verdict refuses a completed one), so it is newer
// than any run that has one.
function age(a: Run, b: Run): number {
  if (a.at === b.at) return 0;
  if (a.at === "") return 1;
  if (b.at === "") return -1;
  return a.at > b.at ? 1 : -1;
}

/** Per-gate report lines and the gates that are not green, in gate order. */
export function verdict(
  gates: readonly string[],
  runs: string,
): { lines: string[]; bad: string[] } {
  const body = runs.endsWith("\n") ? runs.slice(0, -1) : runs;
  const latest = new Map<string, Run>();
  for (const [index, row] of (body === "" ? [] : body.split("\n")).entries()) {
    const fields = row.split("\t");
    if (fields.length !== 4) {
      throw new Fail(
        `check run line ${String(index + 1)} is not 4 tab-separated fields: ${repr(row)}`,
      );
    }
    const [name = "", status = "", conclusion = "", at = ""] = fields;
    if (!gates.includes(name)) continue;
    // Its place among the other runs is unknown, so it could hide a later failure.
    if (status === "completed" && at === "") {
      throw new Fail(`check run line ${String(index + 1)} is ${name} completed with no time`);
    }
    const run = { at, status, conclusion };
    const prior = latest.get(name);
    // Equal times are ambiguous; a run that is not green wins the tie.
    if (!prior || age(run, prior) > 0 || (age(run, prior) === 0 && green(prior) && !green(run))) {
      latest.set(name, run);
    }
  }
  const lines: string[] = [];
  const bad: string[] = [];
  for (const gate of gates) {
    const run = latest.get(gate) ?? { at: "", status: "missing", conclusion: "none" };
    lines.push(`${gate}: ${run.status} ${run.conclusion} ${run.at}`);
    if (!green(run)) bad.push(gate);
  }
  return { lines, bad };
}

if (import.meta.main) {
  try {
    const { lines, bad } = verdict(GATES, decodeUtf8(readFileSync(0)));
    for (const line of lines) process.stdout.write(line + "\n");
    if (bad.length > 0) throw new Fail(`release commit is not green on: ${bad.join(", ")}`);
  } catch (error) {
    process.stderr.write(
      `release-check-ci: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = error instanceof Fail ? error.code : 1;
  }
}
