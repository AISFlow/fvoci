import { YAML } from "bun";
import { expect, test } from "bun:test";
import { strict as assert } from "node:assert";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";

type Step = {
  id?: string;
  uses?: string;
  run?: string;
  if?: string;
  "continue-on-error"?: boolean | string;
  env?: Record<string, string>;
  with?: Record<string, unknown>;
};
type Job = {
  name?: string;
  needs?: string | string[];
  if?: string;
  "continue-on-error"?: boolean | string;
  permissions?: Record<string, string> | string;
  env?: Record<string, string>;
  outputs?: Record<string, string>;
  steps: Step[];
};
type Workflow = {
  on: Record<string, unknown>;
  permissions: Record<string, string> | string;
  env?: Record<string, string>;
  jobs: Record<string, Job>;
};

const root = resolve(import.meta.dir, "../..");
const contracts = {
  rust: {
    fast: "select_fast",
    "native-arm64": "select_native_arm64",
    "postgres-build": "select_postgres",
    postgres: "select_postgres",
    collaboration: "select_collaboration",
  },
  web: {
    "web-static": "select_web_static",
    "web-checks": "select_web_checks",
    "web-native-checks": "select_web_checks",
    "workspace-browser-build": "select_workspace_browser_shard",
    "workspace-browser-shard": "select_workspace_browser_shard",
    "collaboration-build": "select_collaboration_build",
    "collaboration-flow": "select_collaboration_flow",
  },
  documents: { "native-extraction": "select_native_extraction" },
  "collab-engine": { "native-collab-engine": "select_native_collab_engine" },
  install: {
    "install-smoke": "select_install_smoke",
    "backup-restore-smoke": "select_backup_restore_smoke",
    "upgrade-smoke-arm64": "select_upgrade_smoke_arm64",
  },
};
type SelectedWorkflow = keyof typeof contracts;

function load(name: string): Workflow {
  const parsed: unknown = YAML.parse(
    readFileSync(join(root, ".github/workflows", `${name}.yml`), "utf8"),
  );
  // Repository YAML is the fixture; Bun owns parsing. This is not a YAML schema validator.
  return parsed as Workflow;
}

function job(workflow: Workflow, id: string): Job {
  const value = workflow.jobs[id];
  assert.ok(value, `missing job ${id}`);
  return value;
}

function needs(value: Job): string[] {
  return typeof value.needs === "string" ? [value.needs] : (value.needs ?? []);
}

function expression(value: string | undefined): string {
  return (value ?? "")
    .trim()
    .replace(/^\$\{\{\s*|\s*\}\}$/g, "")
    .trim();
}

function required(value: Job | Step): void {
  assert.equal(value["continue-on-error"], undefined, "required work cannot ignore failure");
}

function oneStep(value: Job, predicate: (step: Step) => boolean): Step {
  const found = value.steps.filter(predicate);
  assert.equal(found.length, 1, "required step must occur exactly once");
  const result = found[0];
  assert.ok(result);
  return result;
}

function caller(value: Job, operation: "plan" | "gate", workflow: string): Step {
  const command = new RegExp(`^python3\\s+scripts/ci_selection\\.py\\s+${operation}\\b`, "m");
  const step = oneStep(value, (candidate) => command.test(candidate.run ?? ""));
  required(step);
  assert.equal(step.if, undefined, "selector caller must be unconditional");
  const script = (step.run ?? "").replace(/\\\n\s*/g, " ");
  assert.match(script, /^set -euo pipefail$/m, "selector caller must propagate failure");
  const line = script.split("\n").find((row) => command.test(row));
  assert.ok(line);
  assert.match(line, new RegExp(`--workflow\\s+${workflow}(?:\\s|$)`));
  assert.doesNotMatch(line, /[|;&]/, "selector caller must not mask its result");
  return step;
}

function selectionPolicy(workflow: Workflow, name: SelectedWorkflow): void {
  const products = contracts[name];
  const gateId = `${name}-ci-gate`;
  assert.deepEqual(
    Object.keys(workflow.jobs).sort(),
    ["ci-plan", gateId, ...Object.keys(products)].sort(),
    "every product job must belong to the stable gate",
  );
  assert.ok(Object.hasOwn(workflow.on, "pull_request"));
  assert.equal(workflow.on.pull_request, null, "required gate cannot have path/branch filters");
  const plan = job(workflow, "ci-plan");
  required(plan);
  assert.equal(plan.if, undefined);
  const checkout = oneStep(plan, (step) => step.uses?.startsWith("actions/checkout@") ?? false);
  assert.deepEqual(checkout.with, { "fetch-depth": 0 }, "plan must use tested merge history");
  for (const env of [workflow.env, plan.env, ...plan.steps.map((step) => step.env)]) {
    assert.equal(env?.GITHUB_SHA, undefined, "plan cannot override trusted source");
  }
  caller(plan, "plan", name);
  for (const output of ["plan_ok", "plan_json", ...Object.values(products)]) {
    assert.equal(plan.outputs?.[output], `\${{ steps.plan.outputs.${output} }}`);
  }
  for (const [id, output] of Object.entries(products)) {
    const product = job(workflow, id);
    required(product);
    assert.ok(needs(product).includes("ci-plan"), "product must depend on plan");
    assert.equal(
      expression(product.if),
      `needs.ci-plan.outputs.${output} == 'true'`,
      "product must use its boolean selector without result bypasses",
    );
  }
  const gate = job(workflow, gateId);
  required(gate);
  assert.equal(gate.name, gateId, "required check name must stay stable");
  assert.equal(expression(gate.if), "always()", "gate must run after upstream failure");
  assert.deepEqual(
    needs(gate).sort(),
    ["ci-plan", ...Object.keys(products)].sort(),
    "gate needs must include plan and every producer/consumer",
  );
  // Result semantics remain in ci_selection.py + test-ci-selection.sh: plan must
  // succeed; selected jobs must succeed; only unselected jobs may be skipped.
  // TS checks the real caller and inputs, never evaluates Actions or copies that gate.
  const check = caller(gate, "gate", name);
  assert.equal(check.env?.NEEDS_JSON, "${{ toJSON(needs) }}");
  assert.equal(check.env?.TESTED_SHA, "${{ github.sha }}");
  assert.match(check.run ?? "", /--needs-json\s+"\$NEEDS_JSON"/);
  assert.match(check.run ?? "", /--tested-sha\s+"\$TESTED_SHA"/);
}

function actionSteps(value: Job, action: string): Step[] {
  return value.steps.filter((step) => step.uses?.startsWith(`${action}@`));
}

function artifactPolicy(workflow: Workflow, name: "rust" | "web"): void {
  const pairs =
    name === "rust"
      ? [
          ["postgres-build", "postgres"],
          ["postgres-build", "collaboration"],
        ]
      : [
          ["workspace-browser-build", "workspace-browser-shard"],
          ["collaboration-build", "collaboration-flow"],
        ];
  for (const pair of pairs) {
    const [producerId, consumerId] = pair;
    assert.ok(producerId && consumerId);
    const producer = job(workflow, producerId);
    const consumer = job(workflow, consumerId);
    assert.ok(needs(consumer).includes(producerId), "consumer must depend on its producer");
    const download = oneStep(
      consumer,
      (step) => step.uses?.startsWith("actions/download-artifact@") ?? false,
    );
    required(download);
    assert.equal(download.if, undefined);
    assert.equal(download.with?.["run-id"], undefined, "handoff must stay in the current run");
    assert.equal(download.with?.repository, undefined);
    const uploads = actionSteps(producer, "actions/upload-artifact");
    const upload =
      name === "web"
        ? oneStep(producer, (step) => uploads.includes(step) && step.id === "publish")
        : oneStep(
            producer,
            (step) => uploads.includes(step) && step.with?.name === download.with?.name,
          );
    required(upload);
    assert.equal(upload.if, undefined);
    assert.equal(upload.with?.["if-no-files-found"], "error", "missing packet must fail producer");
    if (name === "web") {
      assert.equal(producer.outputs?.artifact_id, "${{ steps.publish.outputs.artifact-id }}");
      assert.equal(
        download.with?.["artifact-ids"],
        `\${{ needs.${producerId}.outputs.artifact_id }}`,
      );
      assert.equal(
        download.with?.name,
        undefined,
        "web consumer must download the exact artifact ID",
      );
    } else {
      const artifactName = download.with?.name;
      assert.ok(typeof artifactName === "string");
      for (const binding of ["runner.arch", "github.sha", "github.run_attempt"]) {
        assert.ok(artifactName.includes(`\${{ ${binding} }}`), `artifact must bind ${binding}`);
      }
    }
  }
}

function accessPolicy(workflow: Workflow, name: string): void {
  assert.deepEqual(
    workflow.permissions,
    { contents: "read" },
    "workflow default must stay read-only",
  );
  const releasePermissions: Record<string, Record<string, string>> = {
    verify: { contents: "read", checks: "read", packages: "read" },
    build: { contents: "read", packages: "write" },
    index: { contents: "read", packages: "write" },
    dist: { contents: "read" },
    smoke: { contents: "read" },
    publish: { contents: "read", packages: "write" },
    release: { contents: "write" },
  };
  for (const [id, value] of Object.entries(workflow.jobs)) {
    const allowed: Record<string, string> | undefined =
      name === "release" ? releasePermissions[id] : { contents: "read" };
    assert.ok(allowed, "unknown release job permissions require review");
    if (value.permissions !== undefined) {
      assert.ok(typeof value.permissions === "object", "blanket permissions are forbidden");
      for (const [scope, level] of Object.entries(value.permissions)) {
        assert.equal(level, allowed[scope], `unexpected permission ${id}.${scope}`);
      }
    }
    for (const step of value.steps) {
      if (step.uses !== undefined && !step.uses.startsWith("./")) {
        assert.match(
          step.uses,
          /^[\w.-]+\/[\w./-]+@[0-9a-f]{40}$/,
          "external action must use a full SHA",
        );
      }
    }
  }
}

for (const name of Object.keys(contracts) as SelectedWorkflow[]) {
  test(`${name}: actual YAML keeps the plan/product/stable gate contract`, () => {
    selectionPolicy(load(name), name);
  });
}
for (const name of ["rust", "web"] as const) {
  test(`${name}: actual producer and consumer artifact links`, () =>
    artifactPolicy(load(name), name));
}
for (const file of readdirSync(join(root, ".github/workflows"))) {
  if (file.endsWith(".yml")) {
    const name = file.slice(0, -4);
    test(`${name}: actual permissions and external action pins`, () =>
      accessPolicy(load(name), name));
  }
}

type Mutation = {
  name: string;
  mutate: (workflow: Workflow) => void;
  error: string;
};
const selectionMutations: Mutation[] = [
  {
    name: "gate needs omission",
    mutate: (w) => {
      job(w, "web-ci-gate").needs = ["ci-plan"];
    },
    error: "gate needs",
  },
  {
    name: "gate always removal",
    mutate: (w) => {
      delete job(w, "web-ci-gate").if;
    },
    error: "gate must run",
  },
  {
    name: "gate success-only condition",
    mutate: (w) => {
      job(w, "web-ci-gate").if = "success()";
    },
    error: "gate must run",
  },
  {
    name: "unstable required check name",
    mutate: (w) => {
      job(w, "web-ci-gate").name = "optional";
    },
    error: "check name",
  },
  {
    name: "product missing plan dependency",
    mutate: (w) => {
      delete job(w, "web-static").needs;
    },
    error: "depend on plan",
  },
  {
    name: "product selector accepts skipped",
    mutate: (w) => {
      job(w, "web-static").if += " || needs.ci-plan.result == 'skipped'";
    },
    error: "boolean selector",
  },
  {
    name: "unregistered product job",
    mutate: (w) => {
      w.jobs.extra = structuredClone(job(w, "web-static"));
    },
    error: "every product job",
  },
  {
    name: "plan source override",
    mutate: (w) => {
      job(w, "ci-plan").env = { GITHUB_SHA: "untrusted" };
    },
    error: "trusted source",
  },
  {
    name: "plan checkout ref override",
    mutate: (w) => {
      job(w, "ci-plan").steps[0] = {
        uses: "actions/checkout@" + "a".repeat(40),
        with: { "fetch-depth": 0, ref: "main" },
      };
    },
    error: "tested merge",
  },
  {
    name: "conditional gate caller",
    mutate: (w) => {
      caller(job(w, "web-ci-gate"), "gate", "web").if = "false";
    },
    error: "unconditional",
  },
  {
    name: "ignored gate failure",
    mutate: (w) => {
      caller(job(w, "web-ci-gate"), "gate", "web")["continue-on-error"] = true;
    },
    error: "ignore failure",
  },
  {
    name: "incomplete gate result input",
    mutate: (w) => {
      caller(job(w, "web-ci-gate"), "gate", "web").env = {
        NEEDS_JSON: "{}",
        TESTED_SHA: "${{ github.sha }}",
      };
    },
    error: "toJSON(needs)",
  },
  {
    name: "echo instead of gate execution",
    mutate: (w) => {
      const s = caller(job(w, "web-ci-gate"), "gate", "web");
      s.run = 'echo "python3 scripts/ci_selection.py gate --workflow web"';
    },
    error: "exactly once",
  },
];
for (const mutation of selectionMutations) {
  test(`negative: ${mutation.name}`, () => {
    const workflow = load("web");
    mutation.mutate(workflow);
    expect(() => selectionPolicy(workflow, "web")).toThrow(mutation.error);
  });
}

const artifactMutations: Mutation[] = [
  {
    name: "producer dependency removal",
    mutate: (w) => {
      job(w, "workspace-browser-shard").needs = "ci-plan";
    },
    error: "depend on its producer",
  },
  {
    name: "wrong producer artifact ID",
    mutate: (w) => {
      const s = actionSteps(job(w, "workspace-browser-shard"), "actions/download-artifact")[0];
      assert.ok(s);
      s.with = { "artifact-ids": "${{ needs.collaboration-build.outputs.artifact_id }}" };
    },
    error: "workspace-browser-build",
  },
  {
    name: "missing producer packet tolerated",
    mutate: (w) => {
      const s = oneStep(job(w, "workspace-browser-build"), (s) => s.id === "publish");
      s.with = { "if-no-files-found": "warn" };
    },
    error: "missing packet",
  },
];
for (const mutation of artifactMutations) {
  test(`negative: ${mutation.name}`, () => {
    const workflow = load("web");
    mutation.mutate(workflow);
    expect(() => artifactPolicy(workflow, "web")).toThrow(mutation.error);
  });
}

const accessMutations: Mutation[] = [
  {
    name: "workflow contents write expansion",
    mutate: (w) => {
      w.permissions = { contents: "write" };
    },
    error: "read-only",
  },
  {
    name: "job packages write expansion",
    mutate: (w) => {
      job(w, "web-static").permissions = { packages: "write" };
    },
    error: "unexpected permission",
  },
  {
    name: "blanket job write permissions",
    mutate: (w) => {
      job(w, "web-static").permissions = "write-all";
    },
    error: "blanket permissions",
  },
  {
    name: "mutable external action pin",
    mutate: (w) => {
      job(w, "web-static").steps[0] = { uses: "actions/checkout@v4" };
    },
    error: "full SHA",
  },
];
for (const mutation of accessMutations) {
  test(`negative: ${mutation.name}`, () => {
    const workflow = load("web");
    mutation.mutate(workflow);
    expect(() => accessPolicy(workflow, "web")).toThrow(mutation.error);
  });
}
