import { describe, expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { catalogRunners } from "./matrix.ts";
import { PLANNER_ROOT } from "./paths.ts";
import {
  gateJobId,
  loadRegistryContext,
  parseWorkflow,
  selectOutputKey,
  verifyPlanRegistry,
  WORKFLOW_JOBS,
  WORKFLOW_YAML,
  WORKFLOWS,
  type ParsedWorkflow,
  type RegistryContext,
} from "./registry.ts";
import { copyWorkflows, tempDir } from "./test-support.ts";

const real = () => loadRegistryContext(PLANNER_ROOT);
const read = (name: string) => readFileSync(join(PLANNER_ROOT, ".github/workflows", name), "utf8");
function mutate(name: string, edit: (text: string) => string): RegistryContext {
  const ctx = real();
  const text = read(name);
  const changed = edit(text);
  expect(changed).not.toBe(text);
  const workflows = new Map<string, ParsedWorkflow>(ctx.workflows ?? []);
  workflows.set(name, parseWorkflow(name, changed));
  return { root: ctx.root, workflows };
}
const verify = (ctx: RegistryContext) => verifyPlanRegistry(ctx, catalogRunners);

describe("plan registry", () => {
  test("gate ids and selector outputs, in registry order", () => {
    expect(WORKFLOWS).toEqual(["web", "rust", "documents", "collab-engine", "install"]);
    expect(WORKFLOWS.map(gateJobId).join(" ")).toBe(
      "web-ci-gate rust-ci-gate documents-ci-gate collab-engine-ci-gate install-ci-gate",
    );
    expect(selectOutputKey("web-native-checks")).toBe("select_web_checks");
    expect(selectOutputKey("workspace-browser-build")).toBe("select_workspace_browser_shard");
    expect(selectOutputKey("postgres-build")).toBe("select_postgres");
    expect(selectOutputKey("upgrade-smoke-arm64")).toBe("select_upgrade_smoke_arm64");
  });

  test("the checked-in workflows satisfy it and register exactly the planned jobs", () => {
    const ctx = real();
    expect(verify(ctx)).toEqual([]);
    for (const workflow of WORKFLOWS) {
      const parsed = ctx.workflows?.get(WORKFLOW_YAML[workflow]);
      if (!parsed?.ok) throw new Error(`${workflow} did not parse`);
      const jobs = Object.keys(parsed.data.jobs as object).filter(
        (j) => j !== "ci-plan" && j !== gateJobId(workflow),
      );
      expect(new Set(jobs)).toEqual(new Set(WORKFLOW_JOBS[workflow]));
    }
  });

  test("wiring the plan depends on is refused by name", () => {
    expect(verify({ root: "/", workflows: null })).toEqual(["missing .github/workflows directory"]);
    const without = new Map(real().workflows ?? []);
    without.delete("documents.yml");
    without.set("extra.yml", { ok: true, data: { jobs: {} } });
    expect(verify({ root: "/", workflows: without })).toEqual([
      "unknown workflow file extra.yml",
      "documents: missing workflow file documents.yml",
    ]);
    const cases: [string, (t: string) => string, string][] = [
      [
        "web.yml",
        (t) => t.replace(/\n {6}select_web_static: [^\n]*/, ""),
        "web: missing selector output select_web_static",
      ],
      [
        "web.yml",
        (t) => t.replace("\n  pull_request:\n", "\n  pull_request:\n    branches: [main]\n"),
        "web: pull_request must be unfiltered so required gates always run",
      ],
      [
        "install.yml",
        (t) => t.replace("types: [checks_requested]", "types: [destroyed]"),
        "install: merge_group must request checks_requested",
      ],
      [
        "rust.yml",
        (t) => t.replace("  fast:\n", "  fast-renamed:\n"),
        "rust: unregistered job id fast-renamed",
      ],
      [
        "rust.yml",
        (t) => t.replace("          fetch-depth: 0\n", "          fetch-depth: 1\n"),
        "rust: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override",
      ],
      [
        "rust.yml",
        (t) => t.replace("      postgres_matrix: ${{ steps.plan.outputs.postgres_matrix }}\n", ""),
        "rust: ci-plan must publish postgres_matrix from the plan step",
      ],
      [
        "rust.yml",
        (t) => t.replaceAll('"runner": "ubuntu-26.04-arm"', '"runner": "ubuntu-24.04-arm"'),
        "rust.yml: postgres requires explicit Ubuntu 26.04 runners",
      ],
      [
        "collab-engine.yml",
        (t) =>
          t.replace(
            "\njobs:\n",
            "\njobs:\n  ci-gate-extra:\n    runs-on: ubuntu-26.04\n    steps: []\n",
          ),
        "collab-engine: unregistered job id ci-gate-extra",
      ],
      ["documents.yml", () => "on: [", "documents: documents.yml: YAML parse failed"],
    ];
    for (const [name, edit, message] of cases) {
      const errors = verify(mutate(name, edit));
      expect(
        errors.some((e) => e.startsWith(message)),
        `${message}\n${errors.join("\n")}`,
      ).toBe(true);
    }
  });

  test("a workflow that is not UTF-8 is a parse failure, not a lossy read", () => {
    const root = tempDir("registry");
    copyWorkflows(root);
    const path = join(root, ".github/workflows/rust.yml");
    writeFileSync(path, Buffer.concat([Buffer.from("# \xff\n", "latin1"), readFileSync(path)]));
    const errors = verify(loadRegistryContext(root));
    expect(errors.some((e) => e.startsWith("rust: rust.yml: YAML parse failed"))).toBe(true);
  });
});
