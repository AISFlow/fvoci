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

const SETUP_BUN = "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6";

/**
 * Jobs that run Bun, directly or through scripts/release-*.sh, and the
 * .bun-version they read: dist runs the tooling/ provenance entry, so it pins
 * the tooling commit's Bun.
 */
const BUN_VERSION_FILES: Readonly<Record<string, string>> = {
  verify: ".bun-version",
  index: ".bun-version",
  dist: "tooling/.bun-version",
  smoke: ".bun-version",
  publish: ".bun-version",
  release: ".bun-version",
};

// Registry tooling runs from the workflow ref: these jobs read no file of the
// tagged tree, and older tags have no tools/release.
const TOOLING_CHECKOUT_JOBS = ["index", "publish"] as const;
const WORKFLOW_REF = "${{ github.sha }}";

/** The Bun entries each job runs, as written. */
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

const isPython = (step: Value): boolean => {
  const uses = get(step, "uses");
  const shell = get(step, "shell");
  return (
    /python/i.test(runOf(step)) ||
    (typeof shell === "string" && /python/i.test(shell)) ||
    (typeof uses === "string" && uses.startsWith("actions/setup-python@"))
  );
};

const isCheckout = (step: Value): boolean => {
  const uses = get(step, "uses");
  return typeof uses === "string" && uses.startsWith("actions/checkout@");
};

/** No step runs Python; Bun is set up from a pinned .bun-version right after the checkouts. */
function verifyReleaseRuntime(data: Mapping, name: string): string[] {
  const errors: string[] = [];
  const jobs = get(data, "jobs");
  for (const job of isMapping(jobs) ? Object.keys(jobs) : []) {
    if (stepsOf(data, job).some(isPython)) {
      errors.push(`${name}: ${job} runs Python; release steps run the tools/release Bun entries`);
    }
  }
  for (const [job, versionFile] of Object.entries(BUN_VERSION_FILES)) {
    const steps = stepsOf(data, job);
    const lastCheckout = steps.map(isCheckout).lastIndexOf(true);
    const setup = { uses: SETUP_BUN, with: { "bun-version-file": versionFile } };
    if (lastCheckout < 0 || !deepEqual(steps[lastCheckout + 1], setup)) {
      errors.push(
        `${name}: ${job} must set up Bun (${SETUP_BUN}, bun-version-file ${versionFile}) right after its checkout`,
      );
    }
  }
  for (const job of TOOLING_CHECKOUT_JOBS) {
    const checkouts = stepsOf(data, job).filter(isCheckout);
    if (checkouts.length !== 1 || get(get(checkouts[0], "with"), "ref") !== WORKFLOW_REF) {
      errors.push(`${name}: ${job} must check out only the workflow ref ${pyRepr(WORKFLOW_REF)}`);
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
