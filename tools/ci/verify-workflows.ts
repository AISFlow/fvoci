import { resolve } from "node:path";
import { loadContext, type VerifyContext, type WorkflowCheck } from "./verify/load.ts";
import {
  gatedWorkflowJobs,
  verifyGateHardening,
  verifyWorkflowRegistry,
} from "./verify/registry.ts";
import { verifyRustBinaryHandoff } from "./verify/rust-handoff.ts";
import { verifySelectedLibraryExecution } from "./verify/rust-library.ts";
import { verifyPostgresBudgetMatrix } from "./verify/rust-postgres.ts";
import { verifyRustSuiteRegistry } from "./verify/rust-registry.ts";
import { RUST_WORKFLOW_FILE } from "./verify/rust-shared.ts";
import { verifyPermissionPins } from "./verify/scopes.ts";
import { verifyWebBrowserBudget, verifyWebBuildHandoff } from "./verify/web-browser.ts";

// Checks owned by this module, in report order.
export const CHECKS: readonly WorkflowCheck[] = [
  verifyWorkflowRegistry,
  verifyGateHardening,
  verifyPermissionPins,
];

export const SLOT_NAMES = ["rust-binary-handoff", "rust-suite-registry", "web-browser"] as const;
export type SlotName = (typeof SLOT_NAMES)[number];
type Slots = Readonly<Record<SlotName, WorkflowCheck | null>>;

// The Rust suite and web browser checks, one slot per call site of the
// original: the binary handoff (with the SQLite prefix cache) runs only for a
// well-formed rust.yml, the suite registry walks Cargo.toml and rust.yml with
// the library and PostgreSQL budget checks at their fixed points (the
// execution, native, collaboration, install and schema checks are inside that
// walk), and the web checks run only for a well-formed web.yml. An empty slot
// makes the CLI fail closed unless --partial is passed explicitly.
export const SLOTS: Slots = {
  "rust-binary-handoff": (ctx) => {
    const jobs = gatedWorkflowJobs(ctx, RUST_WORKFLOW_FILE);
    return jobs === null ? [] : verifyRustBinaryHandoff(jobs);
  },
  "rust-suite-registry": (ctx) =>
    verifyRustSuiteRegistry(ctx, { verifySelectedLibraryExecution, verifyPostgresBudgetMatrix }),
  "web-browser": (ctx) => [...verifyWebBrowserBudget(ctx), ...verifyWebBuildHandoff(ctx)],
};

export function unfilledSlots(slots: Slots = SLOTS): SlotName[] {
  return SLOT_NAMES.filter((name) => slots[name] === null);
}

export function verifyWorkflows(ctx: VerifyContext, slots: Slots = SLOTS): string[] {
  // Without the workflow directory nothing else is meaningful.
  if (ctx.files === null) return verifyWorkflowRegistry(ctx);
  const checks = [...CHECKS, ...SLOT_NAMES.flatMap((name) => slots[name] ?? [])];
  return checks.flatMap((check) => check(ctx));
}

/** Every error the CLI reports for `root` without --partial; plan.ts runs the same. */
export function verifyRepository(root: string, slots: Slots = SLOTS): string[] {
  return [
    ...verifyWorkflows(loadContext(root), slots),
    ...unfilledSlots(slots).map((name) => `verify-workflows: check slot ${name} is not wired`),
  ];
}

const USAGE = "usage: verify-workflows.ts [--repo-root DIR] [--partial]";

type Args = { root: string; partial: boolean };

export function parseArgs(argv: readonly string[]): Args | string {
  const args: Args = { root: resolve(import.meta.dir, "../.."), partial: false };
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index] ?? "";
    if (arg === "--partial") args.partial = true;
    else if (arg === "--repo-root") {
      const value = argv[++index];
      if (value === undefined || value.startsWith("-")) {
        return "argument --repo-root: expected one argument";
      }
      args.root = resolve(value);
    } else if (arg.startsWith("--repo-root="))
      args.root = resolve(arg.slice("--repo-root=".length));
    else return `unrecognized arguments: ${arg}`;
  }
  return args;
}

/** One error per line on stderr; exit 1 if any, 2 on usage errors, else 0. */
export function main(argv: readonly string[], slots: Slots = SLOTS): number {
  const args = parseArgs(argv);
  if (typeof args === "string") {
    process.stderr.write(`${USAGE}\nverify-workflows.ts: error: ${args}\n`);
    return 2;
  }
  const errors = args.partial
    ? verifyWorkflows(loadContext(args.root), slots)
    : verifyRepository(args.root, slots);
  for (const error of errors) process.stderr.write(error + "\n");
  return errors.length > 0 ? 1 : 0;
}

if (import.meta.main) process.exitCode = main(Bun.argv.slice(2));
