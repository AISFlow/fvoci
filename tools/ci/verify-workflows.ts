import { resolve } from "node:path";
import { loadContext, type VerifyContext, type WorkflowCheck } from "./verify/load.ts";
import { verifyGateHardening, verifyWorkflowRegistry } from "./verify/registry.ts";
import { verifyPermissionPins } from "./verify/scopes.ts";

// Checks owned by this module, in report order.
export const CHECKS: readonly WorkflowCheck[] = [
  verifyWorkflowRegistry,
  verifyGateHardening,
  verifyPermissionPins,
];

export const SLOT_NAMES = [
  "rust-library",
  "rust-postgres",
  "rust-sqlite",
  "rust-exec",
  "rust-collab",
  "rust-native",
  "rust-install",
  "rust-schema",
  "web-browser",
] as const;
export type SlotName = (typeof SLOT_NAMES)[number];

// Rust suite and web browser checks, wired in by the modules that own them. An empty
// slot makes the CLI fail closed unless --partial is passed explicitly.
export const SLOTS: Readonly<Record<SlotName, WorkflowCheck | null>> = {
  "rust-library": null,
  "rust-postgres": null,
  "rust-sqlite": null,
  "rust-exec": null,
  "rust-collab": null,
  "rust-native": null,
  "rust-install": null,
  "rust-schema": null,
  "web-browser": null,
};

export function unfilledSlots(
  slots: Readonly<Record<SlotName, WorkflowCheck | null>> = SLOTS,
): SlotName[] {
  return SLOT_NAMES.filter((name) => slots[name] === null);
}

export function verifyWorkflows(
  ctx: VerifyContext,
  slots: Readonly<Record<SlotName, WorkflowCheck | null>> = SLOTS,
): string[] {
  const checks = [...CHECKS, ...SLOT_NAMES.flatMap((name) => slots[name] ?? [])];
  return checks.flatMap((check) => check(ctx));
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
export function main(argv: readonly string[]): number {
  const args = parseArgs(argv);
  if (typeof args === "string") {
    process.stderr.write(`${USAGE}\nverify-workflows.ts: error: ${args}\n`);
    return 2;
  }
  const errors = verifyWorkflows(loadContext(args.root));
  if (!args.partial) {
    errors.push(
      ...unfilledSlots().map((name) => `verify-workflows: check slot ${name} is not wired`),
    );
  }
  for (const error of errors) process.stderr.write(error + "\n");
  return errors.length > 0 ? 1 : 0;
}

if (import.meta.main) process.exitCode = main(Bun.argv.slice(2));
