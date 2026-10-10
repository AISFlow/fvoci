import { describe, expect, test } from "bun:test";
import { postgresMatrixJson, postgresMatrixRows, rustCatalogRows } from "./matrix.ts";
import { PLANNER_ROOT } from "./paths.ts";
import { pyDumps, pyLoads, type PyValue } from "./pyjson.ts";
import { loadRegistryContext } from "./registry.ts";

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

  test("an event that would run no row is refused", () => {
    const x64Only = realRows().filter((row) => row.get("pg_major") !== "18");
    expect(postgresMatrixJson("pull_request", x64Only)).toEqual({
      json: null,
      error: "rust: postgres matrix has no rows for pull_request",
    });
    expect(postgresMatrixJson("merge_group", x64Only).error).toBeNull();
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
