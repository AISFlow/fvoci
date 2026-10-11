// Web e2e group discovery and sharding on fixture trees (ported from
// scripts/test_web_e2e_groups.py; registered by scripts/test-web-e2e-groups.sh).
import { describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
  DEFAULT_SHARD_COUNT,
  PAIR_FIRST,
  PAIR_SECOND,
  PlanError,
  ROOT,
  assignShards,
  discoverGroups,
  groupSeconds,
  shardMatrix,
  shardPlanLines,
  verifyPlan,
  type Group,
  type Shards,
} from "./groups.ts";

const require = createRequire(import.meta.url);

/** A file of the playwright package the web app resolves (its own or a workspace ancestor's). */
function playwrightFile(...parts: string[]): string {
  let directory = join(ROOT, "apps", "web");
  for (;;) {
    const pkg = join(directory, "node_modules", "playwright");
    if (existsSync(join(pkg, "package.json"))) return join(pkg, ...parts);
    const parent = dirname(directory);
    if (parent === directory) throw new Error("playwright is not installed; run bun ci");
    directory = parent;
  }
}

function withTree(body: (e2e: string) => void): void {
  const e2e = mkdtempSync(join(tmpdir(), "fvoci-web-e2e-groups."));
  try {
    body(e2e);
  } finally {
    rmSync(e2e, { recursive: true, force: true });
  }
}

function writeSpec(directory: string, name: string, content = "// fixture\n"): void {
  if (name.includes("/")) throw new Error(name);
  writeFileSync(join(directory, name), content, "utf8");
}

function minimalPairTree(directory: string, extra: string[] = []): void {
  writeSpec(directory, "workspace-flow.spec.ts");
  writeSpec(directory, "workspace-wiki-flow.spec.ts");
  for (const name of extra) writeSpec(directory, name);
}

/** Expand Playwright default suite suffixes without a glob parser. */
function playwrightDefaultSuffixes(): string[] {
  const suffixes: string[] = [];
  for (const kind of ["spec", "test"])
    for (const prefix of ["", "c", "m"])
      for (const lang of ["j", "t"])
        for (const extX of ["", "x"]) suffixes.push(`.${kind}.${prefix}${lang}s${extX}`);
  return suffixes;
}

function readPinnedPlaywrightTestMatch(): string {
  const text = readFileSync(playwrightFile("lib", "common", "index.js"), "utf8");
  const match = /testMatch:\s*takeFirst\([^,]+,\s*[^,]+,\s*"([^"]+)"\)/.exec(text);
  if (!match?.[1]) throw new Error("default testMatch not found");
  return match[1];
}

function playwrightMatchRels(pattern: string, rels: string[]): string[] {
  const util = require(playwrightFile("lib", "util.js")) as {
    createFileMatcher: (pattern: string) => (path: string) => boolean;
  };
  const matcher = util.createFileMatcher(pattern);
  return rels.filter((rel) => matcher(rel));
}

function planError(run: () => unknown): string {
  try {
    run();
  } catch (error) {
    if (error instanceof PlanError) return error.message;
    throw error;
  }
  throw new Error("expected a plan refusal");
}

const sortedGroups = (groups: Group[]) => groups.map((group) => JSON.stringify(group)).sort();

describe("web e2e groups", () => {
  test("observed costs reduce concentrated makespan", () => {
    // Reproduce round-robin putting two expensive groups on the same shard.
    // Keep the cost regression independent of future production spec count.
    const groups: Group[] = Array.from({ length: 16 }, (_, index) => [
      `e2e/new-${String(index)}.spec.ts`,
    ]);
    groups[0] = ["e2e/v050-task-timer.spec.ts"];
    groups[8] = ["e2e/v050-editor-modes.spec.ts"];
    const balanced = assignShards(groups, 8);
    const old: Shards = Array.from({ length: 8 }, (_, index) =>
      groups.filter((_, position) => position % 8 === index),
    );
    const makespan = (shards: Shards) =>
      Math.max(
        ...shards.map((shard) => shard.reduce((sum, group) => sum + groupSeconds(group), 0)),
      );
    // The timer's four runtime allocations must not remain counted as one
    // cheap group. Require a material improvement over the original plan.
    expect(makespan(balanced)).toBeLessThan(makespan(old) * 0.75);
    expect(sortedGroups(balanced.flat())).toEqual(sortedGroups(groups));
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
    withTree((e2e) => {
      minimalPairTree(e2e, [
        "v050-task-timer.spec.ts",
        ...Array.from({ length: 20 }, (_, index) => `new-${String(index)}.spec.ts`),
      ]);
      verifyPlan(e2e, 8);
      const rows = Array.from({ length: 8 }, (_, index) => shardPlanLines(e2e, index, 8)).flat();
      const specs = rows.flatMap((row) => row.specs);
      expect(specs.length).toBe(new Set(specs).size);
      expect(new Set(specs)).toEqual(
        new Set(
          [
            "workspace-flow.spec.ts",
            "workspace-wiki-flow.spec.ts",
            "v050-task-timer.spec.ts",
            ...Array.from({ length: 20 }, (_, index) => `new-${String(index)}.spec.ts`),
          ].map((name) => `e2e/${name}`),
        ),
      );
      const pair = JSON.stringify([`e2e/${PAIR_FIRST}`, `e2e/${PAIR_SECOND}`]);
      expect(rows.filter((row) => JSON.stringify(row.specs) === pair)).toHaveLength(1);
      expect(
        rows.filter((row) => JSON.stringify(row.specs) === '["e2e/v050-task-timer.spec.ts"]'),
      ).toHaveLength(1);
    });
  });

  test("assignment missing, duplicate and split groups fail closed", () => {
    withTree((e2e) => {
      minimalPairTree(e2e, ["alpha.spec.ts"]);
      const groups = discoverGroups(e2e);
      const pair = groups.find((group) => group.length === 2) ?? [];
      const first = groups[0] ?? [];
      const badPlans: Shards[] = [
        [[first]],
        [[...groups, first]],
        [[first, [pair[0] ?? ""], [pair[1] ?? ""]]],
        [[first, ["e2e/foreign.spec.ts"]]],
      ];
      for (const plan of badPlans) {
        const message = planError(() =>
          verifyPlan(e2e, 1, { discoverGroups, assignShards: () => plan }),
        );
        expect(message).toContain("shard assignment");
      }
    });
  });

  test("duplicate and malformed discovery fail closed", () => {
    withTree((e2e) => {
      minimalPairTree(e2e);
      const pair = discoverGroups(e2e)[0] ?? [];
      expect(
        planError(() => verifyPlan(e2e, 1, { discoverGroups: () => [pair, pair], assignShards })),
      ).toContain("duplicate spec membership");
      expect(
        planError(() =>
          verifyPlan(e2e, 1, { discoverGroups: () => [["../escape.spec.ts"]], assignShards }),
        ),
      ).toContain("invalid spec path");
    });
  });

  test("missing wiki and invalid shards fail closed", () => {
    withTree((e2e) => {
      writeSpec(e2e, PAIR_FIRST);
      expect(planError(() => verifyPlan(e2e, 1))).toContain("missing paired spec");
      writeSpec(e2e, PAIR_SECOND);
      for (const count of [0, -1]) {
        expect(planError(() => verifyPlan(e2e, count))).toContain("shard_count");
      }
      for (const index of [-1, 1]) {
        expect(planError(() => shardPlanLines(e2e, index, 1))).toContain("out of range");
      }
    });
  });

  test("workspace pair required", () => {
    withTree((e2e) => {
      writeSpec(e2e, "workspace-wiki-flow.spec.ts");
      planError(() => verifyPlan(e2e, 2));
    });
  });

  test("a tree without the workspace pair fails closed", () => {
    withTree((e2e) => {
      writeSpec(e2e, "alpha.spec.ts");
      writeSpec(e2e, "beta.spec.ts");
      expect(planError(() => verifyPlan(e2e, 2))).toContain("workspace pair integrity failed");
    });
  });

  test("new spec auto included", () => {
    withTree((e2e) => {
      minimalPairTree(e2e, ["brand-new-flow.spec.ts"]);
      verifyPlan(e2e, 2);
      const specs = [0, 1].flatMap((index) =>
        shardPlanLines(e2e, index, 2).flatMap((line) => line.specs),
      );
      expect(specs).toContain("e2e/brand-new-flow.spec.ts");
    });
  });

  test("nested spec fails closed", () => {
    withTree((e2e) => {
      minimalPairTree(e2e);
      mkdirSync(join(e2e, "nested"));
      writeSpec(join(e2e, "nested"), "hidden-flow.spec.ts");
      expect(planError(() => verifyPlan(e2e, 2))).toContain("nested spec.ts");
    });
  });

  test("unsupported test suffix", () => {
    withTree((e2e) => {
      minimalPairTree(e2e);
      writeSpec(e2e, "collab-wire.test.ts");
      expect(planError(() => verifyPlan(e2e, 2))).toContain(".test.ts");
    });
  });

  test("empty shard fails verify", () => {
    withTree((e2e) => {
      minimalPairTree(e2e);
      expect(planError(() => verifyPlan(e2e, 8))).toContain("shards with no work");
    });
  });

  test("shard-jsonl keeps the pair on one line", () => {
    withTree((e2e) => {
      minimalPairTree(e2e, ["alpha-flow.spec.ts", "beta-flow.spec.ts"]);
      const pair = shardPlanLines(e2e, 0, 2).find((line) => line.specs.length === 2);
      expect(pair?.specs).toEqual([
        "e2e/workspace-flow.spec.ts",
        "e2e/workspace-wiki-flow.spec.ts",
      ]);
    });
  });

  test("unsafe basename rejected", () => {
    withTree((e2e) => {
      minimalPairTree(e2e);
      writeSpec(e2e, "bad spec.spec.ts");
      planError(() => verifyPlan(e2e, 2));
    });
  });

  test("representative module suffixes fail closed", () => {
    const pattern = readPinnedPlaywrightTestMatch();
    expect(pattern).toBe("**/*.@(spec|test).?(c|m)[jt]s?(x)");
    const names = [
      "extra-flow.spec.mts",
      "extra-flow.test.mjs",
      "extra-flow.spec.cts",
      "extra-flow.test.cjs",
    ];
    const rels = names.map((name) => `e2e/${name}`);
    expect(playwrightMatchRels(pattern, rels)).toEqual(rels);
    for (const name of names) {
      withTree((e2e) => {
        minimalPairTree(e2e);
        writeSpec(e2e, name);
        const message = planError(() => verifyPlan(e2e, 2));
        expect(message).toContain(name);
        expect(message).toContain(name.slice(name.indexOf(".")));
      });
    }
  });

  test("all default discoverable suffixes fail closed", () => {
    const pattern = readPinnedPlaywrightTestMatch();
    const names = playwrightDefaultSuffixes()
      .filter((suffix) => suffix !== ".spec.ts")
      .map((suffix) => `extra-flow${suffix}`);
    const rels = names.map((name) => `e2e/${name}`);
    expect(new Set(playwrightMatchRels(pattern, rels))).toEqual(new Set(rels));
    withTree((e2e) => {
      minimalPairTree(e2e);
      for (const name of names) writeSpec(e2e, name);
      const message = planError(() => verifyPlan(e2e, 2));
      for (const name of names) expect(message).toContain(name);
    });
  });

  test("new spec.ts does not hide sibling mts", () => {
    withTree((e2e) => {
      minimalPairTree(e2e, ["brand-new-flow.spec.ts", "brand-new-flow.spec.mts"]);
      expect(planError(() => verifyPlan(e2e, 2))).toContain(".spec.mts");
    });
  });
});

describe("groups CLI", () => {
  const cli = join(import.meta.dir, "groups.ts");
  const run = (...args: string[]) =>
    Bun.spawnSync([process.execPath, cli, ...args], { stdout: "pipe", stderr: "pipe" });

  test("verify prints one compact JSON summary of the real tree", () => {
    const result = run("verify", "--shards", String(DEFAULT_SHARD_COUNT));
    expect(result.exitCode).toBe(0);
    const text = result.stdout.toString();
    expect(text.endsWith("}\n")).toBe(true);
    const summary = JSON.parse(text) as { shard_count: number; groups_per_shard: number[] };
    expect(Object.keys(summary)).toEqual([
      "group_count",
      "spec_count",
      "shard_count",
      "groups_per_shard",
    ]);
    expect(summary.shard_count).toBe(DEFAULT_SHARD_COUNT);
    expect(text).not.toContain(" ");
  });

  test("shard-jsonl covers every real spec once across shards", () => {
    const specs = Array.from({ length: DEFAULT_SHARD_COUNT }, (_, index) => {
      const result = run(
        "shard-jsonl",
        "--index",
        String(index),
        "--shards",
        String(DEFAULT_SHARD_COUNT),
      );
      expect(result.exitCode).toBe(0);
      return result.stdout
        .toString()
        .trimEnd()
        .split("\n")
        .flatMap((line) => (JSON.parse(line) as { specs: string[] }).specs);
    }).flat();
    expect(specs.length).toBe(new Set(specs).size);
    const tree = readdirSync(join(ROOT, "apps", "web", "e2e"))
      .filter((name) => name.endsWith(".spec.ts"))
      .map((name) => `e2e/${name}`);
    expect([...specs].sort()).toEqual(tree.sort());
  });

  test("policy queries print one compact line from the module constants", () => {
    const line = (query: string) => {
      const result = run(query);
      expect(result.exitCode).toBe(0);
      expect(result.stderr.toString()).toBe("");
      const text = result.stdout.toString();
      expect(text.endsWith("\n") && !text.slice(0, -1).includes("\n")).toBe(true);
      return text.slice(0, -1);
    };
    expect(line("shards")).toBe(String(DEFAULT_SHARD_COUNT));
    expect(JSON.parse(line("matrix"))).toEqual(shardMatrix());
    expect(shardMatrix().shard).toEqual([...Array(DEFAULT_SHARD_COUNT).keys()]);
    expect(line("matrix")).not.toContain(" ");
  });

  test("web.yml browser shard matrix follows the groups.ts shard count", () => {
    // Bun owns YAML parsing, as in tools/ci/workflows.test.ts. Until the
    // wiring request lands the matrix is a literal; afterwards it must be the
    // ci-plan output produced by `groups.ts matrix`.
    const workflow = Bun.YAML.parse(
      readFileSync(join(ROOT, ".github", "workflows", "web.yml"), "utf8"),
    ) as { jobs: Record<string, { strategy?: { matrix?: unknown } }> };
    const matrix = workflow.jobs["workspace-browser-shard"]?.strategy?.matrix;
    if (typeof matrix === "string") {
      expect(matrix).toBe("${{ fromJSON(needs.ci-plan.outputs.browser_matrix) }}");
    } else {
      expect(matrix).toEqual(shardMatrix());
    }
  });

  test("refusals: plan errors exit 1, usage errors exit 2, nothing on stdout", () => {
    for (const [args, code] of [
      [["verify", "--shards", "0"], 1],
      [["shard-jsonl", "--index", "8", "--shards", "8"], 1],
      [["shard-jsonl"], 2],
      [["verify", "--shards"], 2],
      [["verify", "--shards", "8x"], 2],
      [["verify", "--sh", "8"], 2],
      [["list-groups", "--shards", "8"], 2],
      [["shards", "--shards", "8"], 2],
      [["matrix", "extra"], 2],
      [["toString"], 2],
      [["bogus"], 2],
      [[], 2],
    ] as const) {
      const result = run(...args);
      expect(result.exitCode).toBe(code);
      expect(result.stdout.toString()).toBe("");
      expect(result.stderr.toString()).not.toBe("");
    }
  });
});
