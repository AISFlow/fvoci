import { describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  PAIR_FIRST,
  PAIR_SECOND,
  assignShards,
  discoverGroups,
  groupSeconds,
  shardPlanLines,
  verifyKnownGroups,
  verifyPlan,
} from "./groups.ts";
import { Fail } from "./proc.ts";

function writeSpec(directory: string, name: string) {
  if (name.includes("/")) throw new Error(name);
  writeFileSync(join(directory, name), "// fixture\n");
}

function pairTree(extra: string[] = []) {
  const directory = mkdtempSync(join(tmpdir(), "fvoci-e2e-groups-"));
  writeSpec(directory, PAIR_FIRST);
  writeSpec(directory, PAIR_SECOND);
  for (const name of extra) writeSpec(directory, name);
  return directory;
}

function playwrightFile(...parts: string[]) {
  let directory = join(import.meta.dir, "../../apps/web");
  for (;;) {
    const candidate = join(directory, "node_modules/playwright", ...parts);
    if (existsSync(candidate)) return candidate;
    const parent = join(directory, "..");
    if (parent === directory) throw new Error("playwright is not installed; run bun ci");
    directory = parent;
  }
}

describe("web e2e groups", () => {
  test("observed costs reduce concentrated makespan", () => {
    const groups = Array.from({ length: 16 }, (_, index) => [`e2e/new-${index}.spec.ts`]);
    groups[0] = ["e2e/v050-task-timer.spec.ts"];
    groups[8] = ["e2e/v050-editor-modes.spec.ts"];
    const balanced = assignShards(groups, 8);
    const old = Array.from({ length: 8 }, (_, index) =>
      groups.filter((_, group) => group % 8 === index),
    );
    const makespan = (shards: string[][][]) =>
      Math.max(
        ...shards.map((shard) => shard.reduce((sum, group) => sum + groupSeconds(group), 0)),
      );
    expect(makespan(balanced)).toBeLessThan(makespan(old) * 0.75);
    expect(
      balanced
        .flat()
        .map((group) => group.join("\0"))
        .sort(),
    ).toEqual(groups.map((group) => group.join("\0")).sort());
    expect(balanced.every((shard) => shard.length > 0)).toBe(true);
  });

  test("longest first and deterministic ties", () => {
    const timer = ["e2e/v050-task-timer.spec.ts"];
    const editor = ["e2e/v050-editor-modes.spec.ts"];
    const alpha = ["e2e/alpha.spec.ts"];
    const beta = ["e2e/beta.spec.ts"];
    const groups = [beta, timer, alpha, editor];
    const expected = [[timer], [editor, alpha, beta]];
    expect(assignShards(groups, 2)).toEqual(expected);
    expect(assignShards([...groups].reverse(), 2)).toEqual(expected);
    expect(assignShards([beta, alpha], 2)).toEqual([[alpha], [beta]]);
  });

  test("unknown specs and workspace pair covered once", () => {
    const directory = pairTree([
      "v050-task-timer.spec.ts",
      ...Array.from({ length: 20 }, (_, index) => `new-${index}.spec.ts`),
    ]);
    verifyPlan(directory, 8);
    const rows = Array.from({ length: 8 }, (_, index) =>
      shardPlanLines(directory, index, 8),
    ).flat();
    const specs = rows.flatMap((row) => row.specs);
    expect(new Set(specs).size).toBe(specs.length);
    expect(new Set(specs)).toEqual(
      new Set(
        readdirSync(directory)
          .filter((name: string) => name.endsWith(".spec.ts"))
          .map((name: string) => `e2e/${name}`),
      ),
    );
    const pair = [`e2e/${PAIR_FIRST}`, `e2e/${PAIR_SECOND}`];
    expect(rows.filter((row) => row.specs.join() === pair.join()).length).toBe(1);
    expect(rows.filter((row) => row.specs.join() === "e2e/v050-task-timer.spec.ts").length).toBe(1);
  });

  test("assignment missing duplicate and split groups fail closed", () => {
    const directory = pairTree(["alpha.spec.ts"]);
    const groups = discoverGroups(directory);
    const pair = groups.find((group) => group.length === 2);
    if (!pair) throw new Error("pair missing");
    const bad = [
      [[groups[0] ?? []]],
      [[...groups, groups[0] ?? []]],
      [[groups[0] ?? [], [pair[0] ?? ""], [pair[1] ?? ""]]],
      [[groups[0] ?? [], ["e2e/foreign.spec.ts"]]],
    ];
    for (const plan of bad) {
      expect(() => verifyKnownGroups(directory, 1, groups, plan)).toThrow(/shard assignment/);
    }
  });

  test("duplicate and malformed discovery fail closed", () => {
    const directory = pairTree();
    const pair = discoverGroups(directory)[0] ?? [];
    expect(() => verifyKnownGroups(directory, 1, [pair, pair])).toThrow(
      /duplicate spec membership/,
    );
    expect(() => verifyKnownGroups(directory, 1, [["../escape.spec.ts"]])).toThrow(
      /invalid spec path/,
    );
  });

  test("missing wiki and invalid shards fail closed", () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-e2e-groups-"));
    writeSpec(directory, PAIR_FIRST);
    expect(() => verifyPlan(directory, 1)).toThrow(/missing paired spec/);
    writeSpec(directory, PAIR_SECOND);
    for (const count of [0, -1]) expect(() => verifyPlan(directory, count)).toThrow(/shard_count/);
    for (const index of [-1, 1])
      expect(() => shardPlanLines(directory, index, 1)).toThrow(/out of range/);
  });

  test("workspace pair is required", () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-e2e-groups-"));
    writeSpec(directory, PAIR_SECOND);
    expect(() => verifyPlan(directory, 2)).toThrow(Fail);
  });

  test("new spec is auto included", () => {
    const directory = pairTree(["brand-new-flow.spec.ts"]);
    verifyPlan(directory, 2);
    const specs = [0, 1].flatMap((index) =>
      shardPlanLines(directory, index, 2).flatMap((line) => line.specs),
    );
    expect(specs).toContain("e2e/brand-new-flow.spec.ts");
  });

  test("nested spec fails closed", () => {
    const directory = pairTree();
    mkdirSync(join(directory, "nested"));
    writeFileSync(join(directory, "nested/hidden-flow.spec.ts"), "// fixture\n");
    expect(() => verifyPlan(directory, 2)).toThrow(/nested spec\.ts/);
  });

  test("unsupported test suffix fails closed", () => {
    const directory = pairTree();
    writeSpec(directory, "collab-wire.test.ts");
    expect(() => verifyPlan(directory, 2)).toThrow(/\.test\.ts/);
  });

  test("empty shard fails verify", () => {
    expect(() => verifyPlan(pairTree(), 8)).toThrow(/shards with no work/);
  });

  test("shard jsonl keeps the pair on one line", () => {
    const directory = pairTree(["alpha-flow.spec.ts", "beta-flow.spec.ts"]);
    const lines = shardPlanLines(directory, 0, 2);
    expect(lines.find((group) => group.specs.length === 2)?.specs).toEqual([
      "e2e/workspace-flow.spec.ts",
      "e2e/workspace-wiki-flow.spec.ts",
    ]);
  });

  test("unsafe basename is rejected", () => {
    const directory = pairTree();
    writeSpec(directory, "bad spec.spec.ts");
    expect(() => verifyPlan(directory, 2)).toThrow(Fail);
  });

  test("pinned playwright testMatch and representative suffixes fail closed", () => {
    const index = readFileSync(playwrightFile("lib/common/index.js"), "utf8");
    const match = index.match(/testMatch:\s*takeFirst\([^,]+,\s*[^,]+,\s*"([^"]+)"\)/);
    expect(match?.[1]).toBe("**/*.@(spec|test).?(c|m)[jt]s?(x)");
    for (const name of [
      "extra-flow.spec.mts",
      "extra-flow.test.mjs",
      "extra-flow.spec.cts",
      "extra-flow.test.cjs",
    ]) {
      const directory = pairTree();
      writeSpec(directory, name);
      expect(() => verifyPlan(directory, 2)).toThrow(new RegExp(name.replaceAll(".", "\\.")));
    }
  });

  test("all default discoverable suffixes fail closed", () => {
    const suffixes: string[] = [];
    for (const kind of ["spec", "test"]) {
      for (const prefix of ["", "c", "m"]) {
        for (const lang of ["j", "t"]) {
          for (const ext of ["", "x"]) suffixes.push(`.${kind}.${prefix}${lang}s${ext}`);
        }
      }
    }
    const names = suffixes
      .filter((suffix) => suffix !== ".spec.ts")
      .map((suffix) => `extra-flow${suffix}`);
    const directory = pairTree();
    for (const name of names) writeSpec(directory, name);
    try {
      verifyPlan(directory, 2);
      throw new Error("expected failure");
    } catch (error) {
      const message = error instanceof Error ? error.message : "";
      for (const name of names) expect(message).toContain(name);
    }
  });

  test("a new spec.ts does not hide a sibling mts", () => {
    const directory = pairTree(["brand-new-flow.spec.ts", "brand-new-flow.spec.mts"]);
    expect(() => verifyPlan(directory, 2)).toThrow(/\.spec\.mts/);
  });
});
