import {
  deepEqual,
  get,
  isMapping,
  pyRepr,
  sameKeys,
  triggersOf,
  type Mapping,
  type Value,
} from "./load.ts";
import { RELEASE_WRITE_SCOPES, verifyWorkflowWriteScopes } from "./scopes.ts";

export const RELEASE_WORKFLOW_FILE = "release.yml";

const SETUP_BUN = {
  uses: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
  with: { "bun-version-file": ".bun-version" },
};

/** Jobs that run Bun, directly or through scripts/release-*.sh. */
const BUN_JOBS = ["verify", "index", "dist", "smoke", "publish", "release"] as const;

/** The Bun entries each job runs as written (they replaced python3 scripts). */
const RELEASE_COMMANDS: Readonly<Record<string, readonly string[]>> = {
  verify: ['bash scripts/release-check-ci.sh "${{ steps.source.outputs.sha }}"'],
  index: [
    'index="$(bun tools/release/release-api.ts push-index --image "$IMAGE" --amd64 "$amd64" --arm64 "$arm64")"',
    'bun tools/release/release-api.ts describe --image "$IMAGE" --digest "$index" >"$RUNNER_TEMP/index.out"',
  ],
  // The tooling/ checkout is the workflow ref; it stamps the tag's dist.
  dist: [
    'bun tooling/tools/release/provenance.ts --dist "$RUNNER_TEMP/dist" \\\n' +
      '  --tooling-sha "${{ github.sha }}" --tooling-ref "${{ github.ref }}"',
  ],
  publish: [
    'bun tools/release/release-api.ts tag --image "$IMAGE" --digest "$INDEX" --tag "$VERSION"',
    'bun tools/release/release-api.ts tag --image "$IMAGE" --digest "$INDEX" --tag "$MINOR" --floating',
  ],
};

function stepsOf(data: Mapping, job: string): Value[] {
  const steps = get(get(get(data, "jobs"), job), "steps");
  return Array.isArray(steps) ? steps : [];
}

const runOf = (step: Value): string => {
  const run = get(step, "run");
  return typeof run === "string" ? run : "";
};

const isCheckout = (step: Value): boolean => {
  const uses = get(step, "uses");
  return typeof uses === "string" && uses.startsWith("actions/checkout@");
};

/** No step runs Python; Bun is set up from .bun-version right after the checkouts. */
function verifyReleaseRuntime(data: Mapping, name: string): string[] {
  const errors: string[] = [];
  const jobs = get(data, "jobs");
  for (const job of isMapping(jobs) ? Object.keys(jobs) : []) {
    if (stepsOf(data, job).some((step) => /python/i.test(runOf(step)))) {
      errors.push(`${name}: ${job} runs Python; release steps run the tools/release Bun entries`);
    }
  }
  for (const job of BUN_JOBS) {
    const steps = stepsOf(data, job);
    const lastCheckout = steps.map(isCheckout).lastIndexOf(true);
    if (lastCheckout < 0 || !deepEqual(steps[lastCheckout + 1], SETUP_BUN)) {
      errors.push(
        `${name}: ${job} must set up Bun (${SETUP_BUN.uses}, bun-version-file .bun-version) right after its checkout`,
      );
    }
  }
  for (const [job, commands] of Object.entries(RELEASE_COMMANDS)) {
    const runs = stepsOf(data, job).map(runOf);
    for (const command of commands) {
      if (!runs.some((run) => run.includes(command))) {
        errors.push(`${name}: ${job} must run ${pyRepr(command)}`);
      }
    }
  }
  return errors;
}

/** Only tag pushes and manual dispatch; read-only default token; scoped writes. */
export function verifyReleaseWorkflow(data: Mapping, name = RELEASE_WORKFLOW_FILE): string[] {
  const errors: string[] = [];
  const triggers = triggersOf(data);
  if (!sameKeys(triggers, ["push", "workflow_dispatch"])) {
    errors.push(`${name}: triggers must be exactly push (tags) and workflow_dispatch`);
  } else {
    const push = get(triggers, "push");
    const tags = get(push, "tags");
    if (
      !sameKeys(push, ["tags"]) ||
      !Array.isArray(tags) ||
      tags.length === 0 ||
      !tags.every((tag) => typeof tag === "string" && tag.startsWith("v0."))
    ) {
      errors.push(`${name}: push must list only v0.* tags`);
    }
  }
  // One queue for every tag: runs for two patch tags must not race on :0.y.
  const concurrency = get(data, "concurrency");
  const group = get(concurrency, "group");
  if (
    !isMapping(concurrency) ||
    typeof group !== "string" ||
    group.includes("${{") ||
    get(concurrency, "cancel-in-progress") !== false
  ) {
    errors.push(`${name}: concurrency must be one fixed group with cancel-in-progress: false`);
  }
  return [
    ...errors,
    ...verifyWorkflowWriteScopes(data, name, RELEASE_WRITE_SCOPES),
    ...verifyReleaseRuntime(data, name),
  ];
}
