import { afterEach, describe, expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PLANNER_ROOT } from "./paths.ts";
import { gateJobId, loadRegistryContext, WORKFLOW_JOBS, WORKFLOWS } from "./registry.ts";
import { copyWorkflows, removeScratch, tempDir } from "./test-support.ts";

afterEach(removeScratch);

describe("plan registry", () => {
  test("gate ids, in registry order", () => {
    expect(WORKFLOWS).toEqual(["web", "rust", "documents", "collab-engine", "install"]);
    expect(WORKFLOWS.map(gateJobId).join(" ")).toBe(
      "web-ci-gate rust-ci-gate documents-ci-gate collab-engine-ci-gate install-ci-gate",
    );
  });

  test("the checked-in workflows register exactly the planned jobs", () => {
    const ctx = loadRegistryContext(PLANNER_ROOT);
    for (const workflow of WORKFLOWS) {
      const parsed = ctx.workflows?.get(`${workflow}.yml`);
      if (!parsed?.ok) throw new Error(`${workflow} did not parse`);
      const jobs = Object.keys(parsed.data.jobs as object).filter(
        (j) => j !== "ci-plan" && j !== gateJobId(workflow),
      );
      expect(new Set(jobs)).toEqual(new Set(WORKFLOW_JOBS[workflow]));
    }
  });

  test("a workflow that is not UTF-8 is a parse failure, not a lossy read", () => {
    const root = tempDir("registry");
    copyWorkflows(root);
    const path = join(root, ".github/workflows/rust.yml");
    writeFileSync(path, Buffer.concat([Buffer.from("# \xff\n", "latin1"), readFileSync(path)]));
    const parsed = loadRegistryContext(root).workflows?.get("rust.yml");
    expect(parsed?.ok).toBe(false);
    expect(parsed?.ok === false && parsed.error.startsWith("rust.yml: YAML parse failed")).toBe(
      true,
    );
  });
});
