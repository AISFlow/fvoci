import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { postgresMatrixJson, postgresMatrixRows, rustCatalogRows } from "./matrix.ts";
import { PLANNER_ROOT } from "./paths.ts";
import { pyDumps, pyLoads, type PyValue } from "./pyjson.ts";
import {
  loadRegistryContext,
  parseWorkflow,
  POSTGRES_MATRIX_EXPR,
  type ParsedWorkflow,
  type RegistryContext,
} from "./registry.ts";

type Row = Map<string, PyValue>;
const realRows = (): Row[] => {
  const catalog = rustCatalogRows(loadRegistryContext(PLANNER_ROOT));
  if (catalog.error !== null) throw new Error(catalog.error);
  return catalog.rows;
};
const checks = (json: string) =>
  ((pyLoads(json) as Map<string, PyValue>).get("include") as Row[]).map(
    (row) => row.get("check") as string,
  );
const matrix = (eventName: string, rows = realRows()) => {
  const out = postgresMatrixJson(eventName, rows);
  if (out.error !== null) throw new Error(out.error);
  return out.json;
};

// The designated pull request version: PG 18 on x64, every shard.
const PR_CHECKS = ["postgres", "postgres-c", "postgres-b"];
const FULL_CHECKS = [
  "postgres",
  "postgres-c",
  "postgres-b",
  "postgres-arm64",
  "postgres-arm64-c",
  "postgres-arm64-b",
  "postgres-pg16",
  "postgres-pg16-c",
  "postgres-pg16-b",
  "postgres-pg17",
  "postgres-pg17-c",
  "postgres-pg17-b",
];

describe("postgres matrix", () => {
  test("the real catalog has twelve distinct major x platform x shard rows", () => {
    const rows = realRows();
    expect(rows.map((row) => row.get("check"))).toEqual(FULL_CHECKS);
    const keys = rows.map(
      (row) =>
        `${row.get("pg_major") as string}/${row.get("runner") as string}/${row.get("shard") as string}`,
    );
    expect(new Set(keys).size).toBe(rows.length);
    const shardsOf = (major: string, runner: string) =>
      rows
        .filter((r) => r.get("pg_major") === major && r.get("runner") === runner)
        .map((r) => r.get("shard"));
    for (const [major, runner] of [
      ["16", "ubuntu-26.04"],
      ["17", "ubuntu-26.04"],
      ["18", "ubuntu-26.04"],
      ["18", "ubuntu-26.04-arm"],
    ] as const) {
      expect(new Set(shardsOf(major, runner))).toEqual(new Set(["a", "b", "c"]));
    }
  });

  test("pull requests run one version on x64 with every shard", () => {
    const json = matrix("pull_request");
    expect(checks(json)).toEqual(PR_CHECKS);
    const rows = (pyLoads(json) as Map<string, PyValue>).get("include") as Row[];
    expect(
      new Set(rows.map((row) => `${row.get("pg_major") as string}/${row.get("runner") as string}`)),
    ).toEqual(new Set(["18/ubuntu-26.04"]));
  });

  test("every other event runs the full catalog, unchanged and in order", () => {
    for (const eventName of ["merge_group", "push", "workflow_dispatch", "schedule"]) {
      const json = matrix(eventName);
      expect(checks(json)).toEqual(FULL_CHECKS);
      // The rows are the catalog rows byte for byte: no field added, dropped or reordered.
      expect(json).toBe(pyDumps(new Map([["include", realRows()]]), { compact: true }));
      expect(json.startsWith('{"include":[{"runner":"ubuntu-26.04","pg_major":"18",')).toBe(true);
      expect(json).not.toContain("\n");
    }
  });

  test("the output is a complete strategy.matrix object for fromJSON", () => {
    const parsed = JSON.parse(matrix("pull_request")) as Record<string, unknown>;
    expect(Object.keys(parsed)).toEqual(["include"]);
    expect(Array.isArray(parsed.include)).toBe(true);
  });

  test("a catalog that would shrink, grow or repeat a row is refused for every event", () => {
    const rows = realRows();
    const without = (check: string) => rows.filter((row) => row.get("check") !== check);
    const renamed = (check: string, field: string, value: string) =>
      rows.map((row) => (row.get("check") === check ? new Map([...row, [field, value]]) : row));
    const cases: [string, Row[], string][] = [
      ["drop PG18 x64 c", without("postgres-c"), "missing 18/ubuntu-26.04/c"],
      ["drop PG17 x64 b", without("postgres-pg17-b"), "missing 17/ubuntu-26.04/b"],
      [
        "drop every PG18 row",
        rows.filter((r) => r.get("pg_major") !== "18"),
        "missing 18/ubuntu-26.04/a",
      ],
      ["duplicate a row", [...rows, rows[0] as Row], "duplicate 18/ubuntu-26.04/a"],
      [
        "extra major",
        [...rows, new Map([...(rows[0] as Row), ["pg_major", "19"]])],
        "unexpected 19/ubuntu-26.04/a",
      ],
      ["move a shard", renamed("postgres-b", "shard", "d"), "missing 18/ubuntu-26.04/b"],
      [
        "PG16 on arm",
        renamed("postgres-pg16", "runner", "ubuntu-26.04-arm"),
        "unexpected 16/ubuntu-26.04-arm/a",
      ],
    ];
    for (const [label, catalog, problem] of cases) {
      for (const eventName of ["pull_request", "merge_group", "push", "workflow_dispatch"]) {
        const out = postgresMatrixJson(eventName, catalog);
        expect(out.json, `${label} ${eventName}`).toBeNull();
        expect(out.error ?? "", label).toStartWith(
          "rust: postgres matrix catalog must hold exactly the 12 policy rows (",
        );
        expect(out.error ?? "", label).toContain(problem);
      }
    }
    const untyped = rows.map((row, i) => (i === 0 ? new Map([...row, ["pg_major", null]]) : row));
    expect(postgresMatrixJson("push", untyped).error).toBe(
      "rust: postgres matrix catalog row needs string pg_major, runner and shard",
    );
  });

  test("catalog problems are named", () => {
    const job = (catalog: unknown) => ({ env: { FVOCI_POSTGRES_MATRIX_CATALOG: catalog } });
    expect(postgresMatrixRows({}).error).toBe("rust: postgres matrix catalog missing");
    expect(postgresMatrixRows(job("  ")).error).toBe("rust: postgres matrix catalog missing");
    expect(postgresMatrixRows(job(5)).error).toBe("rust: postgres matrix catalog missing");
    expect(postgresMatrixRows(job("[")).error).toBe("rust: postgres matrix catalog is not JSON");
    expect(postgresMatrixRows(job("[]")).error).toBe(
      "rust: postgres matrix catalog must be a non-empty list",
    );
    expect(postgresMatrixRows(job("{}")).error).toBe(
      "rust: postgres matrix catalog must be a non-empty list",
    );
    expect(postgresMatrixRows(job("[1]")).error).toBe(
      "rust: postgres matrix catalog row must be a mapping",
    );
    expect(rustCatalogRows({ root: "/", workflows: new Map() }).error).toBe(
      "rust: missing workflow file rust.yml",
    );
    expect(
      rustCatalogRows({
        root: "/",
        workflows: new Map([["rust.yml", { ok: true, data: { jobs: {} } }]]),
      }).error,
    ).toBe("rust: postgres job missing");
  });
});

describe("postgres matrix consumer", () => {
  const MATRIX_LINE = `      matrix: ${POSTGRES_MATRIX_EXPR}\n`;
  const rustText = () => readFileSync(join(PLANNER_ROOT, ".github/workflows/rust.yml"), "utf8");
  const withRust = (edit: (text: string) => string): RegistryContext => {
    const ctx = loadRegistryContext(PLANNER_ROOT);
    const text = rustText();
    const changed = edit(text);
    expect(changed).not.toBe(text);
    const workflows = new Map<string, ParsedWorkflow>(ctx.workflows ?? []);
    workflows.set("rust.yml", parseWorkflow("rust.yml", changed));
    return { root: ctx.root, workflows };
  };
  const oneRow = () => pyDumps(new Map([["include", [realRows()[0] as Row]]]), { compact: true });

  test("the checked-in postgres job consumes exactly the plan output", () => {
    expect(rustText()).toContain(MATRIX_LINE);
    expect(rustCatalogRows(loadRegistryContext(PLANNER_ROOT)).error).toBeNull();
  });

  test("any other strategy.matrix is refused, even with a complete catalog", () => {
    const replace = (by: string) => (text: string) => text.replace(MATRIX_LINE, by);
    const fromOutput = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix).include }}";
    const cases: [string, (text: string) => string][] = [
      ["one-row literal include", replace(`      matrix: ${oneRow()}\n`)],
      [
        "another plan output",
        replace("      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_exclude) }}\n"),
      ],
      ["inline fromJSON literal", replace(`      matrix: \${{ fromJSON('${oneRow()}') }}\n`)],
      ["include read from the output", replace(`      matrix:\n        include: ${fromOutput}\n`)],
      [
        "output plus an exclude",
        replace(
          `      matrix:\n        include: ${fromOutput}\n        exclude:\n          - shard: b\n`,
        ),
      ],
      ["no matrix", replace("")],
      [
        "no strategy",
        (text) => text.replace(`    strategy:\n      fail-fast: false\n${MATRIX_LINE}`, ""),
      ],
    ];
    for (const [label, edit] of cases) {
      const out = rustCatalogRows(withRust(edit));
      expect(out.rows, label).toBeNull();
      expect(out.error, label).toBe(
        `rust: postgres strategy.matrix must be exactly ${POSTGRES_MATRIX_EXPR}`,
      );
    }
  });
});
