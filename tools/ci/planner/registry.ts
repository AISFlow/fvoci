import { YAML } from "bun";
import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { readUtf8 } from "./paths.ts";

// The selection registry: the five gated workflows, their product jobs, the
// manual opt-ins, and the parsed workflow files the plan step reads. The
// wiring itself is checked by the complete verify-workflows registry.

export const WORKFLOW_JOBS = {
  web: [
    "web-static",
    "web-checks",
    "web-native-checks",
    "workspace-browser-build",
    "workspace-browser-shard",
    "collaboration-build",
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
  ],
  rust: ["fast", "native-arm64", "postgres-build", "postgres", "collaboration"],
  documents: ["native-extraction"],
  "collab-engine": ["native-collab-engine"],
  install: ["install-image", "install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"],
} as const satisfies Record<string, readonly string[]>;
export type Workflow = keyof typeof WORKFLOW_JOBS;
export const WORKFLOWS = Object.keys(WORKFLOW_JOBS) as Workflow[];

export function isWorkflow(name: string): name is Workflow {
  return Object.hasOwn(WORKFLOW_JOBS, name);
}

// Manual opt-in jobs: no path or event policy selects them (not even full
// mode or a fatal plan). Only a workflow_dispatch whose boolean input of the
// same name is exactly true selects the job; the gate re-derives that.
export const OPT_IN_JOBS: Partial<Record<Workflow, Readonly<Record<string, string>>>> = {
  install: { "upgrade-smoke-arm64": "run_upgrade_smoke_arm" },
};

export const PLAN_JOB_ID = "ci-plan";
export const POSTGRES_MATRIX_EXPR = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";

export function gateJobId(workflow: string): string {
  return `${workflow}-ci-gate`;
}

// ---- Workflow files -------------------------------------------------------

export type Mapping = Record<string, unknown>;
export type ParsedWorkflow = { ok: true; data: Mapping } | { ok: false; error: string };
/** What a registry verifier sees: the checkout and every workflow file, parsed. */
export type RegistryContext = {
  root: string;
  /** null when .github/workflows is not a directory. */
  workflows: ReadonlyMap<string, ParsedWorkflow> | null;
};
export type RegistryVerifier = (ctx: RegistryContext) => string[];

export const isMapping = (value: unknown): value is Mapping =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export function parseWorkflow(name: string, text: string): ParsedWorkflow {
  let data: unknown;
  try {
    data = YAML.parse(text);
  } catch (error) {
    return { ok: false, error: `${name}: YAML parse failed: ${(error as Error).message}` };
  }
  if (!isMapping(data)) return { ok: false, error: `${name}: workflow YAML must be a mapping` };
  return { ok: true, data };
}

/** Every *.yml / *.yaml file under .github/workflows, sorted by name. */
export function loadRegistryContext(root: string): RegistryContext {
  const dir = join(root, ".github", "workflows");
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return { root, workflows: null };
  const workflows = new Map<string, ParsedWorkflow>();
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (!/\.ya?ml$/.test(name) || !statSync(path).isFile()) continue;
    let text: string;
    try {
      text = readUtf8(path);
    } catch (error) {
      workflows.set(name, {
        ok: false,
        error: `${name}: YAML parse failed: ${(error as Error).message}`,
      });
      continue;
    }
    workflows.set(name, parseWorkflow(name, text));
  }
  return { root, workflows };
}
