#!/usr/bin/env bun
// ci-plan: compute the CI selection plan of one workflow for this run.
//
//   bun tools/ci/plan.ts --workflow <name> --event-json <path> --output-plan <path>
//                        [--github-output <path>] [--repo-root <dir>]
//
// GITHUB_EVENT_NAME and GITHUB_SHA come from the runner. Exit 0 writes the plan
// file, then the step outputs, then a one-line summary on stdout. Exit 1 is a
// refusal with the reason on stderr and no outputs; exit 2 is a usage error.

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { dispatchOptIns, EventShapeError, resolveSelectionInputs } from "./planner/events.ts";
import { repoGit } from "./planner/git.ts";
import { catalogRunners, postgresMatrixJson, rustCatalogRows } from "./planner/matrix.ts";
import { renderGithubOutput, renderPlanFile, renderSummary } from "./planner/output.ts";
import { PLANNER_ROOT } from "./planner/paths.ts";
import { buildPlan } from "./planner/plan.ts";
import { pyLoads } from "./planner/pyjson.ts";
import {
  isWorkflow,
  loadRegistryContext,
  verifyPlanRegistry,
  WORKFLOWS,
  type RegistryVerifier,
  type Workflow,
} from "./planner/registry.ts";

export type PlanArgs = {
  workflow: Workflow;
  repoRoot: string;
  eventJson: string;
  outputPlan: string;
  githubOutput: string | null;
};

class UsageError extends Error {}

const CHOICES = [...WORKFLOWS].sort();
const USAGE =
  `usage: plan.ts [-h] --workflow {${CHOICES.join(",")}} [--repo-root REPO_ROOT] ` +
  "--event-json EVENT_JSON --output-plan OUTPUT_PLAN [--github-output GITHUB_OUTPUT]\n";
const OPTIONS = [
  "--workflow",
  "--repo-root",
  "--event-json",
  "--output-plan",
  "--github-output",
  "--help",
] as const;
type Option = (typeof OPTIONS)[number];

const invalidChoice = (value: string) =>
  new UsageError(
    `argument --workflow: invalid choice: '${value}' (choose from ${CHOICES.join(", ")})`,
  );
const ignoredArgument = (value: string) =>
  new UsageError(`argument -h/--help: ignored explicit argument '${value}'`);

/**
 * `--opt value`, `--opt=value`, unique long-option prefixes, last wins.
 * Stricter than argparse: a separate value never starts with "-", every
 * --workflow occurrence must be a known workflow, and -h/--help take no
 * attached text. Unknown tokens are reported after the required check.
 */
export function parseArgs(argv: readonly string[]): PlanArgs | "help" {
  const values = new Map<Option, string>();
  const extras: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i] as string;
    if (!arg.startsWith("--") || arg === "--") {
      if (arg === "-h") return "help";
      if (arg.startsWith("-h=")) throw ignoredArgument(arg.slice(3));
      if (arg.startsWith("-h") && arg.length > 2) throw ignoredArgument(arg.slice(2));
      extras.push(arg);
      continue;
    }
    const eq = arg.indexOf("=");
    const name = eq < 0 ? arg : arg.slice(0, eq);
    const matches = OPTIONS.filter((option) => option.startsWith(name));
    const option =
      OPTIONS.find((o) => o === name) ?? (matches.length === 1 ? matches[0] : undefined);
    if (option === undefined) {
      if (matches.length > 1) {
        throw new UsageError(`ambiguous option: ${name} could match ${matches.join(", ")}`);
      }
      extras.push(arg);
      continue;
    }
    if (option === "--help") {
      if (eq >= 0) throw ignoredArgument(arg.slice(eq + 1));
      return "help";
    }
    let value: string | undefined;
    if (eq >= 0) value = arg.slice(eq + 1);
    else {
      value = argv[i + 1];
      if (value === undefined || value.startsWith("-")) {
        throw new UsageError(`argument ${option}: expected one argument`);
      }
      i++;
    }
    if (option === "--workflow" && !isWorkflow(value)) throw invalidChoice(value);
    values.set(option, value);
  }
  const missing = ["--workflow", "--event-json", "--output-plan"].filter(
    (o) => !values.has(o as Option),
  );
  if (missing.length > 0)
    throw new UsageError(`the following arguments are required: ${missing.join(", ")}`);
  if (extras.length > 0) throw new UsageError(`unrecognized arguments: ${extras.join(" ")}`);
  const workflow = values.get("--workflow") as string;
  if (!isWorkflow(workflow)) throw invalidChoice(workflow);
  return {
    workflow,
    repoRoot: resolve(values.get("--repo-root") ?? PLANNER_ROOT),
    eventJson: values.get("--event-json") as string,
    outputPlan: values.get("--output-plan") as string,
    githubOutput: values.get("--github-output") ?? null,
  };
}

export type Io = {
  env: Readonly<Record<string, string | undefined>>;
  stdout: (text: string) => void;
  stderr: (text: string) => void;
};

/** The registry step is injected so the complete verify-workflows registry can take this slot. */
export function verifyRegistryDefault(...[ctx]: Parameters<RegistryVerifier>): string[] {
  return verifyPlanRegistry(ctx, catalogRunners);
}

export function runPlan(
  args: PlanArgs,
  io: Io,
  verifyRegistry: RegistryVerifier = verifyRegistryDefault,
): number {
  const ctx = loadRegistryContext(args.repoRoot);
  const errors = verifyRegistry(ctx);
  if (errors.length > 0) {
    io.stderr("plan: workflow registry validation failed\n" + errors.map((e) => `${e}\n`).join(""));
    return 1;
  }
  const eventName = (io.env.GITHUB_EVENT_NAME ?? "").trim();
  if (!eventName) {
    io.stderr("plan: GITHUB_EVENT_NAME required\n");
    return 1;
  }
  let event;
  try {
    event = pyLoads(
      new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
        readFileSync(args.eventJson),
      ),
    );
  } catch (error) {
    io.stderr(`plan: event JSON unreadable: ${(error as Error).message}\n`);
    return 1;
  }
  const tested = (io.env.GITHUB_SHA ?? "").trim();
  let resolved;
  try {
    resolved = resolveSelectionInputs(repoGit(args.repoRoot), event, eventName, tested);
  } catch (error) {
    if (!(error instanceof EventShapeError)) throw error;
    io.stderr(`plan: event payload invalid: ${error.message}\n`);
    return 1;
  }
  const optIns = dispatchOptIns(args.workflow, eventName, event);
  const plan = buildPlan({
    workflow: args.workflow,
    eventName,
    baseSha: resolved.baseSha,
    headSha: resolved.headSha,
    mergeBaseSha: resolved.mergeBaseSha,
    testedSha: resolved.testedSha,
    paths: resolved.paths,
    fatalError: resolved.fatalError || optIns.error,
    forceFullReason: resolved.forceFullReason,
    optInInputs: optIns.chosen,
  });
  let postgresMatrix: string | null = null;
  if (args.workflow === "rust") {
    const catalog = rustCatalogRows(ctx);
    const matrix = catalog.error === null ? postgresMatrixJson(eventName, catalog.rows) : catalog;
    if (matrix.error !== null) {
      io.stderr(`plan: ${matrix.error}\n`);
      return 1;
    }
    postgresMatrix = matrix.json;
  }
  writeFileSync(args.outputPlan, renderPlanFile(plan));
  if (args.githubOutput !== null)
    writeFileSync(args.githubOutput, renderGithubOutput(plan, postgresMatrix));
  io.stdout(renderSummary(plan));
  return 0;
}

export function main(argv: readonly string[], io: Io): number {
  let args;
  try {
    args = parseArgs(argv);
  } catch (error) {
    if (!(error instanceof UsageError)) throw error;
    io.stderr(`${USAGE}plan.ts: error: ${error.message}\n`);
    return 2;
  }
  if (args === "help") {
    io.stdout(`${USAGE}\nCompute CI selection plan for one workflow.\n`);
    return 0;
  }
  try {
    return runPlan(args, io);
  } catch (error) {
    io.stderr(`plan: ${(error as Error).message}\n`);
    return 1;
  }
}

if (import.meta.main) {
  process.exitCode = main(process.argv.slice(2), {
    env: process.env,
    stdout: (text) => process.stdout.write(text),
    stderr: (text) => process.stderr.write(text),
  });
}
