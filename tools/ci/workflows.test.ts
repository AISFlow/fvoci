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
  secrets?: unknown;
  container?: string | { image?: string };
  env?: Record<string, string>;
  outputs?: Record<string, string>;
  strategy?: {
    "fail-fast"?: boolean;
    matrix?: string | { include?: unknown; exclude?: unknown };
  };
  steps: Step[];
};
type Workflow = {
  on: Record<string, unknown> | string | string[];
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
    "collaboration-install-on": "select_collaboration_install_on",
    "collaboration-postgres-on": "select_collaboration_postgres_on",
    "collaboration-sqlite-on": "select_collaboration_sqlite_on",
    "collaboration-postgres-off": "select_collaboration_postgres_off",
    "collaboration-sqlite-off": "select_collaboration_sqlite_off",
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
  assert.ok(typeof workflow.on === "object" && !Array.isArray(workflow.on));
  assert.equal(workflow.on.pull_request, null, "required gate cannot have path/branch filters");
  assert.ok(Object.hasOwn(workflow.on, "merge_group"), "merge_group trigger is required");
  assert.deepEqual(
    workflow.on.merge_group,
    { types: ["checks_requested"] },
    "merge_group must request checks_requested",
  );
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
  assert.equal(check.env.TESTED_SHA, "${{ github.sha }}");
  assert.match(check.run ?? "", /--needs-json\s+"\$NEEDS_JSON"/);
  assert.match(check.run ?? "", /--tested-sha\s+"\$TESTED_SHA"/);
  if (name === "rust") postgresMatrixFromPlan(workflow);
}

const POSTGRES_MATRIX_FROM_PLAN = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";

function postgresMatrixFromPlan(workflow: Workflow): void {
  const plan = job(workflow, "ci-plan");
  assert.equal(
    plan.outputs?.postgres_matrix,
    "${{ steps.plan.outputs.postgres_matrix }}",
    "ci-plan must publish postgres_matrix from the plan step",
  );
  const postgres = job(workflow, "postgres");
  assert.equal(
    postgres.strategy?.matrix,
    POSTGRES_MATRIX_FROM_PLAN,
    "postgres strategy.matrix must be fromJSON(needs.ci-plan.outputs.postgres_matrix), not a static include",
  );
  assert.notEqual(job(workflow, "postgres-build").strategy?.matrix, POSTGRES_MATRIX_FROM_PLAN);
  assert.notEqual(job(workflow, "collaboration").strategy?.matrix, POSTGRES_MATRIX_FROM_PLAN);
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
          ["collaboration-build", "collaboration-install-on"],
          ["collaboration-build", "collaboration-postgres-on"],
          ["collaboration-build", "collaboration-sqlite-on"],
          ["collaboration-build", "collaboration-postgres-off"],
          ["collaboration-build", "collaboration-sqlite-off"],
        ];
  for (const pair of pairs) {
    const [producerId, consumerId] = pair;
    assert.ok(producerId && consumerId);
    const producer = job(workflow, producerId);
    const consumer = job(workflow, consumerId);
    assert.ok(needs(consumer).includes(producerId), "consumer must depend on its producer");
    const downloads = actionSteps(consumer, "actions/download-artifact");
    const expectedArtifact = `\${{ needs.${producerId}.outputs.artifact_id }}`;
    const producerDownloads =
      name === "web"
        ? downloads.filter((step) => step.with?.["artifact-ids"] === expectedArtifact)
        : downloads;
    const download = producerDownloads[0] ?? downloads[0];
    assert.ok(download, "required step must occur exactly once");
    if (name === "web") {
      assert.equal(download.with?.["artifact-ids"], expectedArtifact);
      assert.equal(producerDownloads.length, 1, "required step must occur exactly once");
    }
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
        download.with.name,
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
      name === "release"
        ? releasePermissions[id]
        : name === "ci-base-image" &&
            (id === "push" || id === "push-manifest") &&
            value.if === "github.ref == 'refs/heads/main'"
          ? { contents: "read", packages: "write" }
          : { contents: "read" };
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
  securityPolicy(workflow, name);
}

function secretReferences(value: unknown, workflowName: string): void {
  if (typeof value === "string") {
    // Lexical inspection only: never evaluate Actions expressions. Single-quoted
    // literals (including doubled quotes and literal }}) stay intact, so text is
    // not mistaken for a context or an expression boundary.
    // https://docs.github.com/en/actions/reference/workflows-and-actions/expressions
    for (const match of value.matchAll(/\$\{\{((?:'(?:[^']|'')*'|[^'}]|}(?!}))*?)\}\}/g)) {
      const tokens = match[1]?.match(/'(?:[^']|'')*'|[A-Za-z_][\w-]*|[^\s]/g) ?? [];
      for (const [index, token] of tokens.entries()) {
        if (token.toLowerCase() !== "secrets" || tokens[index - 1] === ".") continue;
        const property = tokens[index + 2];
        const secretName =
          tokens[index + 1] === "." && property && /^[A-Za-z_][\w-]*$/.test(property)
            ? property
            : tokens[index + 1] === "[" &&
                property &&
                /^'[A-Za-z_][\w-]*'$/.test(property) &&
                tokens[index + 3] === "]"
              ? property.slice(1, -1)
              : undefined;
        assert.ok(
          secretName === "GITHUB_TOKEN" ||
            (workflowName === "turso-test" &&
              (secretName === "FVOCI_TEST_TURSO_DATABASE_URL" ||
                secretName === "FVOCI_TEST_TURSO_AUTH_TOKEN")),
          "secret context must use an approved workflow/name pair",
        );
      }
    }
  } else if (Array.isArray(value)) {
    for (const item of value) secretReferences(item, workflowName);
  } else if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      secretReferences(key, workflowName);
      secretReferences(item, workflowName);
    }
  }
}

function securityPolicy(workflow: Workflow, name: string): void {
  const events =
    typeof workflow.on === "string"
      ? [workflow.on]
      : Array.isArray(workflow.on)
        ? workflow.on
        : Object.keys(workflow.on);
  assert.ok(!events.includes("pull_request_target"), "pull_request_target is forbidden");
  for (const value of Object.values(workflow.jobs)) {
    assert.ok(!Object.hasOwn(value, "secrets"), "job secrets forwarding is forbidden");
    if (value.container !== undefined) {
      const image = typeof value.container === "string" ? value.container : value.container.image;
      assert.ok(typeof image === "string", "container image must use a fixed SHA256 digest");
      assert.match(
        image,
        /^[\w./:-]+@sha256:[0-9a-f]{64}$/,
        "container image must use a fixed SHA256 digest",
      );
    }
  }
  secretReferences(workflow, name);
}

for (const name of Object.keys(contracts) as SelectedWorkflow[]) {
  test(`${name}: actual YAML keeps the plan/product/stable gate contract`, () => {
    selectionPolicy(load(name), name);
  });
}
for (const name of ["rust", "web"] as const) {
  test(`${name}: actual producer and consumer artifact links`, () => {
    artifactPolicy(load(name), name);
  });
}
for (const file of readdirSync(join(root, ".github/workflows"))) {
  if (file.endsWith(".yml")) {
    const name = file.slice(0, -4);
    test(`${name}: actual permissions and external action pins`, () => {
      accessPolicy(load(name), name);
    });
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
    name: "event-specific required check name",
    mutate: (w) => {
      job(w, "web-ci-gate").name =
        "${{ github.event_name == 'pull_request' && 'web-ci-gate' || 'web-merge-gate' }}";
    },
    error: "required check name must stay stable",
  },
  {
    name: "missing merge_group",
    mutate: (w) => {
      assert.ok(typeof w.on === "object" && !Array.isArray(w.on));
      delete w.on.merge_group;
    },
    error: "merge_group trigger is required",
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
      const product = job(w, "web-static");
      assert.ok(typeof product.if === "string");
      product.if += " || needs.ci-plan.result == 'skipped'";
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
    expect(() => {
      selectionPolicy(workflow, "web");
    }).toThrow(mutation.error);
  });
}

test("negative: static postgres include restored", () => {
  const workflow = load("rust");
  const postgres = job(workflow, "postgres");
  postgres.strategy = {
    "fail-fast": false,
    matrix: {
      include: [{ runner: "ubuntu-26.04", pg_major: "18", check: "postgres" }],
    },
  };
  expect(() => {
    selectionPolicy(workflow, "rust");
  }).toThrow("not a static include");
});

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
    expect(() => {
      artifactPolicy(workflow, "web");
    }).toThrow(mutation.error);
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
    expect(() => {
      accessPolicy(workflow, "web");
    }).toThrow(mutation.error);
  });
}

for (const id of ["push", "push-manifest"]) {
  test(`negative: ci-base-image ${id} main guard removal`, () => {
    const workflow = load("ci-base-image");
    delete job(workflow, id).if;
    expect(() => {
      accessPolicy(workflow, "ci-base-image");
    }).toThrow(`unexpected permission ${id}.packages`);
  });
  test(`negative: ci-base-image ${id} main guard bypass`, () => {
    const workflow = load("ci-base-image");
    job(workflow, id).if = "github.ref == 'refs/heads/main' || success()";
    expect(() => {
      accessPolicy(workflow, "ci-base-image");
    }).toThrow(`unexpected permission ${id}.packages`);
  });
}
test("negative: ci-base-image PR build packages write", () => {
  const workflow = load("ci-base-image");
  job(workflow, "build").permissions = { contents: "read", packages: "write" };
  expect(() => {
    accessPolicy(workflow, "ci-base-image");
  }).toThrow("unexpected permission build.packages");
});
test("negative: main publishing permission moved to another workflow", () => {
  const workflow = load("web");
  workflow.jobs.push = structuredClone(job(load("ci-base-image"), "push"));
  expect(() => {
    accessPolicy(workflow, "web");
  }).toThrow("unexpected permission push.packages");
});

const securityMutations: Mutation[] = [
  ...[
    ["new secret name", "${{ secrets.NEW_SECRET }}"],
    ["Turso URL secret moved to another workflow", "${{ secrets.FVOCI_TEST_TURSO_DATABASE_URL }}"],
    ["Turso auth secret moved to another workflow", "${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}"],
    ["indexed new secret", "${{ secrets['X'] }}"],
    ["whole secrets context serialization", "${{ toJSON(secrets) }}"],
    ["dynamic secret index", "${{ secrets[env.NAME] }}"],
    ["secrets object filter", "${{ secrets.* }}"],
    ["case variant secrets context", "${{ SECRETS.NEW_SECRET }}"],
    [
      "new secret after allowed expression",
      "${{ secrets.GITHUB_TOKEN }} ${{ secrets.NEW_SECRET }}",
    ],
    ["new secret after quoted closing braces", "${{ '}}' || secrets.NEW_SECRET }}"],
    ["new secret after escaped quote", "${{ 'it''s text' || secrets.NEW_SECRET }}"],
  ].map(([name, reference]) => {
    assert.ok(name && reference);
    return {
      name,
      mutate: (w: Workflow) => {
        job(w, "web-static").env = { POLICY_FIXTURE: reference };
      },
      error: "approved workflow/name pair",
    };
  }),
  ...["inherit", { FORWARDED: "${{ secrets.GITHUB_TOKEN }}" }, null].map((value) => ({
    name: `job secrets forwarding ${JSON.stringify(value)}`,
    mutate: (w: Workflow) => {
      job(w, "web-static").secrets = value;
    },
    error: "job secrets forwarding",
  })),
  ...[
    "example/image:latest",
    { image: "example/image:latest" },
    "example/image@sha256:abc",
    { image: "example/image@sha256:abc" },
    { image: "${{ matrix.image }}@sha256:" + "a".repeat(64) },
    {},
  ].map((container) => ({
    name: `unpinned container ${JSON.stringify(container)}`,
    mutate: (w: Workflow) => {
      job(w, "web-static").container = container;
    },
    error: "fixed SHA256 digest",
  })),
  ...[
    "pull_request_target",
    ["pull_request", "pull_request_target"],
    { pull_request: null, pull_request_target: null },
  ].map((events) => ({
    name: `pull_request_target event ${JSON.stringify(events)}`,
    mutate: (w: Workflow) => {
      w.on = events;
    },
    error: "pull_request_target is forbidden",
  })),
];
for (const mutation of securityMutations) {
  test(`negative: ${mutation.name}`, () => {
    const workflow = load("web");
    mutation.mutate(workflow);
    expect(() => {
      securityPolicy(workflow, "web");
    }).toThrow(mutation.error);
  });
}

test("security policy permits fixed digest containers and GITHUB_TOKEN references", () => {
  const workflow = load("web");
  job(workflow, "web-static").container = "example/image@sha256:" + "a".repeat(64);
  job(workflow, "web-checks").container = { image: "example/image@sha256:" + "b".repeat(64) };
  workflow.env = {
    TOKEN: "${{ secrets.GITHUB_TOKEN }}",
    INDEXED_TOKEN: "${{ secrets['GITHUB_TOKEN'] }}",
  };
  securityPolicy(workflow, "web");
});
test("secret names remain fixed for property and index access in the Turso workflow", () => {
  const workflow = load("turso-test");
  workflow.env = {
    URL: "${{ secrets['FVOCI_TEST_TURSO_DATABASE_URL'] }}",
    TOKEN: "${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}",
  };
  securityPolicy(workflow, "turso-test");
  workflow.env.TOKEN = "${{ secrets.NEW_SECRET }}";
  expect(() => {
    securityPolicy(workflow, "turso-test");
  }).toThrow("approved workflow/name pair");
});
test("secret context inspection ignores ordinary text and expression string literals", () => {
  const workflow = load("web");
  job(workflow, "web-static").steps.push({
    run: "import secrets\nsecrets.token_hex(16)\n${{ 'secrets.NEW_SECRET' }}\n${{ vars.secrets }}",
  });
  securityPolicy(workflow, "web");
});

// Two jobs saving the Rust target caches under one key race on the same
// cache entry; each writer of target/ or crates/collab-engine/target keeps
// its own key.
function uniqueBuildCacheWriters(workflow: Workflow): void {
  const writers: [string, unknown][] = [];
  for (const [name, config] of Object.entries(workflow.jobs))
    for (const step of config.steps) {
      const action = step.uses ?? "",
        paths = typeof step.with?.path === "string" ? step.with.path : "";
      if (
        (action.startsWith("actions/cache@") || action.startsWith("actions/cache/save@")) &&
        paths
          .split("\n")
          .some((path) => ["target", "crates/collab-engine/target"].includes(path.trim()))
      )
        writers.push([name, step.with?.key]);
    }
  assert.ok(writers.length, "no build-cache writers checked");
  const keys = writers.map(([, key]) => key);
  assert.equal(new Set(keys).size, keys.length, "duplicate Web build-cache writer keys");
}
test("Web build-cache writers keep unique keys", () => {
  uniqueBuildCacheWriters(load("web"));
});
for (const path of ["target", "crates/collab-engine/target"])
  test(`negative: browser build reuses the collaboration ${path} cache key`, () => {
    const workflow = load("web");
    const save = (name: string) =>
      oneStep(
        job(workflow, name),
        (step) => (step.uses ?? "").startsWith("actions/cache/save@") && step.with?.path === path,
      );
    const ordinary = save("workspace-browser-build"),
      selected = save("collaboration-build");
    assert.ok(ordinary.with && selected.with);
    ordinary.with.key = selected.with.key;
    expect(() => {
      uniqueBuildCacheWriters(workflow);
    }).toThrow("duplicate Web build-cache writer keys");
  });
