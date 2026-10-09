import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { test } from "bun:test";
import { runToolchainCase, toolchainCases, type ToolchainCaseName } from "../eslint/toolchain";
import { groupFixture, pairFiles } from "../web-e2e/fixtures";
import {
  assertShardCoverage,
  assertTimerCoverage,
  assertWorkspacePair,
  listTests,
  timerRuns,
} from "../web-e2e/groups";

const root = resolve(import.meta.dir, "../..");
function legacy(args: readonly string[], cwd = root): { stdout: string; stderr: string } {
  // Missing python3, a failed import or any failed assertion is a real FAIL.
  // Only the existing scripts are passed to Python, never -c or generated code.
  const result = spawnSync(args[0] ?? "", args.slice(1), {
    cwd,
    encoding: "utf8",
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stdout + result.stderr);
  return { stdout: result.stdout, stderr: result.stderr };
}

// Each side executes the same original fixture corpus and checks the same
// semantic diagnostics. Legacy unittest exposes per-case OK, not raw lint
// reports; the typed side additionally asserts rule IDs, severities and counts
// through ESLint's real API. No upstream rule implementation is substituted.
for (const name of Object.keys(toolchainCases) as ToolchainCaseName[]) {
  test("legacy/typed VueToolchain." + name, async () => {
    const result = legacy([
      "python3",
      join(root, "scripts/test_eslint.py"),
      "VueToolchain." + name,
    ]);
    assert.match(result.stderr, /Ran 1 test/);
    assert(result.stderr.includes(name));
    assert.match(result.stderr, /\bOK\b/);
    await runToolchainCase(name);
  });
}

test("legacy/typed normal fixtures retain all specs, workspace order and exact shard union", () => {
  const fixture = groupFixture({});
  try {
    const e2e = join(fixture.directory, "apps/web/e2e");
    const scripts = join(fixture.directory, "scripts");
    mkdirSync(e2e, { recursive: true });
    mkdirSync(scripts);
    const script = join(scripts, "web-e2e-groups.py");
    copyFileSync(join(root, "scripts/web-e2e-groups.py"), script);
    const files = pairFiles([
      "v050-task-timer.spec.ts",
      ...Array.from({ length: 20 }, (_, index) => "new-" + String(index) + ".spec.ts"),
    ]);
    for (const [name, source] of Object.entries(files)) writeFileSync(join(e2e, name), source);
    const options = { ...fixture.options, env: { FVOCI_GROUP_FIXTURE_DIR: e2e } };
    const original = legacy(["python3", script, "list-groups"], fixture.directory);
    const groups = original.stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as { specs: string[] });
    const oldSpecs = groups.flatMap((group) => group.specs);
    const current = listTests(options);
    assert.deepEqual(oldSpecs.slice().sort(), current.map((test) => "e2e/" + test.file).sort());
    assertWorkspacePair(current);
    assert.equal(
      groups.filter(
        (group) =>
          group.specs.join("|") === "e2e/workspace-flow.spec.ts|e2e/workspace-wiki-flow.spec.ts",
      ).length,
      1,
    );
    const oldSummary = JSON.parse(
      legacy(["python3", script, "verify", "--shards", "8"], fixture.directory).stdout,
    ) as { spec_count: number; shard_count: number; groups_per_shard: number[] };
    assert.equal(oldSummary.spec_count, current.length);
    assert.equal(oldSummary.shard_count, 8);
    assert(oldSummary.groups_per_shard.every((count) => count > 0));
    const oldShards: string[] = [];
    const newShards = [];
    for (let index = 0; index < 8; index++) {
      const rows = legacy(
        ["python3", script, "shard-jsonl", "--index", String(index), "--shards", "8"],
        fixture.directory,
      )
        .stdout.trim()
        .split("\n")
        .map((line) => JSON.parse(line) as { specs: string[] });
      oldShards.push(...rows.flatMap((row) => row.specs));
      newShards.push(listTests({ ...options, shard: String(index + 1) + "/8" }));
    }
    assert.deepEqual(oldShards.slice().sort(), oldSpecs.slice().sort());
    assert.equal(new Set(oldShards).size, oldShards.length);
    assertShardCoverage(current, newShards);
    assert.throws(() => {
      assertWorkspacePair(current.filter((test) => test.file !== "workspace-flow.spec.ts"));
    }, /Workspace/);
    assert.throws(() => {
      assertWorkspacePair(current.filter((test) => test.file !== "workspace-wiki-flow.spec.ts"));
    }, /Workspace/);
  } finally {
    fixture.cleanup();
  }
});

test("legacy/typed missing pair, empty discovery and invalid shards are failures", () => {
  for (const files of [
    {},
    { "workspace-wiki-flow.spec.ts": "" },
    { "workspace-flow.spec.ts": "" },
  ]) {
    const fixture = groupFixture({});
    try {
      const e2e = join(fixture.directory, "apps/web/e2e");
      const scripts = join(fixture.directory, "scripts");
      mkdirSync(e2e, { recursive: true });
      mkdirSync(scripts);
      const script = join(scripts, "web-e2e-groups.py");
      copyFileSync(join(root, "scripts/web-e2e-groups.py"), script);
      for (const name of Object.keys(files))
        writeFileSync(
          join(e2e, name),
          'import { test } from "@playwright/test"; test("fixture", () => {});',
        );
      const original = spawnSync("python3", [script, "verify", "--shards", "1"], {
        encoding: "utf8",
      });
      if (original.error) throw original.error;
      assert.notEqual(original.status, 0);
      assert.notEqual(original.status, null);
      assert.throws(() => {
        assertWorkspacePair(
          listTests({ ...fixture.options, env: { FVOCI_GROUP_FIXTURE_DIR: e2e } }),
        );
      });
    } finally {
      fixture.cleanup();
    }
  }
});

test("legacy group regression module still executes all original 16 cases", () => {
  const result = legacy(["python3", join(root, "scripts/test_web_e2e_groups.py"), "-v"]);
  assert.match(result.stderr, /Ran 16 tests/);
  assert.match(result.stderr, /\bOK\b/);
});

test("legacy timer lifecycle controls and typed actual discovery preserve four fresh runs", () => {
  // Existing stub fixture includes selected failure, preceding failure, unsafe
  // selection, timer four dispatches and caller-owned evidence namespace tests.
  const result = legacy([
    "bash",
    join(root, "scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh"),
  ]);
  assert(result.stdout.includes("ok"));
  const options = {
    config: join(root, "apps/web/playwright.config.ts"),
    cwd: join(root, "apps/web"),
    selection: ["v050-task-timer.spec.ts"],
  };
  const full = listTests(options);
  const runs = timerRuns.map((run) =>
    listTests({ ...options, selection: ["v050-task-timer.spec.ts", ...run.args] }),
  );
  assertTimerCoverage(full, runs);
});

test("source row inventory has all 16 group and 19 toolchain names", () => {
  const legacyNames = [
    ...readFileSync(join(root, "scripts/test_eslint.py"), "utf8").matchAll(
      /^ {4}def (test_\w+)\(/gm,
    ),
  ].map((match) => match[1]);
  assert.equal(legacyNames.length, 19);
  assert.deepEqual(legacyNames.sort(), Object.keys(toolchainCases).sort());
  const source = readFileSync(join(root, "scripts/test_web_e2e_groups.py"), "utf8");
  const originalNames = [...source.matchAll(/^ {4}def (test_\w+)\(/gm)].map((match) => match[1]);
  const current = readFileSync(join(root, "tools/web-e2e/groups.test.ts"), "utf8");
  assert.equal(originalNames.length, 16);
  for (const name of originalNames) {
    assert(name);
    assert(current.includes('test("' + name));
  }
});
