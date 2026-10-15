import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { parseJson, root } from "../io.ts";
import {
  engineFeaturesMatch,
  loadCurrent,
  validateOffReport,
  validateProductReport,
} from "./binding.ts";

const titles = [
  ...readFileSync(
    join(root, "apps/web/e2e-pending/workspace-off-selected-backend.spec.ts"),
    "utf8",
  ).matchAll(/ {2}test\("([^"\n]+)"/g),
].map((match) => match[1] as string);
const offReport = (change: (report: Record<string, unknown>) => void = () => undefined) => {
  const caseOf = (title: string) => ({
    title,
    file: "/w/apps/web/e2e-pending/workspace-off-selected-backend.spec.ts",
    ok: true,
    tests: [{ expectedStatus: "passed", results: [{ status: "passed", retry: 0, errors: [] }] }],
  });
  const report: Record<string, unknown> = {
    config: { workers: 1, metadata: { selectedBackend: "postgres", selectedFlow: "off" } },
    errors: [],
    stats: { expected: 8, unexpected: 0, flaky: 0, skipped: 0 },
    // Specs of a suite come before its child suites, in declared order.
    suites: [
      {
        specs: titles.slice(0, 3).map(caseOf),
        suites: [{ specs: titles.slice(3).map(caseOf) }],
      },
    ],
  };
  change(report);
  return parseJson(JSON.stringify(report));
};

describe("OFF report", () => {
  test("all eight registered cases pass once in declared order", () => {
    expect(titles.length).toBe(8);
    expect(validateOffReport(offReport(), "postgres")).toEqual(titles);
  });
  test("backend, flow, stats, order, retry and status faults refuse", () => {
    type R = Record<string, unknown> & {
      config: { workers: unknown; metadata: Record<string, unknown> };
      stats: Record<string, unknown>;
      suites: {
        specs: Record<string, unknown>[];
        suites: { specs: Record<string, unknown>[] }[];
      }[];
    };
    const faults: ((r: R) => void)[] = [
      (r) => {
        r.config.metadata.selectedBackend = "sqlite";
      },
      (r) => {
        r.config.metadata.selectedFlow = "on";
      },
      (r) => {
        r.config.workers = 2;
      },
      (r) => {
        r.errors = [{}];
      },
      (r) => {
        r.stats.flaky = 1;
      },
      (r) => {
        r.stats.expected = 7;
      },
      (r) => {
        r.suites[0]?.specs.reverse();
      },
      (r) => {
        (r.suites[0]?.specs[0] as Record<string, unknown>).ok = false;
      },
      (r) => {
        (r.suites[0]?.specs[0] as Record<string, unknown>).file = "/w/other.spec.ts";
      },
      (r) => {
        const spec = r.suites[0]?.specs[0] as { tests: { results: Record<string, unknown>[] }[] };
        (spec.tests[0]?.results[0] as Record<string, unknown>).retry = 1;
      },
      (r) => {
        r.suites[0]?.suites[0]?.specs.pop();
      },
    ];
    for (const fault of faults)
      expect(() =>
        validateOffReport(offReport(fault as (r: Record<string, unknown>) => void), "postgres"),
      ).toThrow();
  });
  test("a float worker token is not the integer the reporter writes", () => {
    const text = JSON.stringify(offReport()).replace('"workers":1', '"workers":1.0');
    expect(() => validateOffReport(parseJson(text), "postgres")).toThrow();
  });
});

test("engine features are exactly default and worker", () => {
  expect(engineFeaturesMatch(["worker", "default"])).toBe(true);
  for (const features of [["worker"], ["default", "worker", "test-hang"], "worker", null])
    expect(engineFeaturesMatch(features)).toBe(false);
});

describe("current binding refusals before any resource", () => {
  const withEnv = async (
    values: Record<string, string | undefined>,
    run: () => Promise<unknown>,
  ) => {
    const saved = { ...process.env };
    Object.assign(process.env, {
      FVOCI_CI_OWNER: "owner",
      FVOCI_CI_SELECTED_RUNS: "/nonexistent/runtime",
    });
    for (const [key, value] of Object.entries(values))
      if (value === undefined) Reflect.deleteProperty(process.env, key);
      else process.env[key] = value;
    try {
      return await run().then(
        () => "accepted",
        (error: unknown) => (error as Error).message,
      );
    } finally {
      for (const key of Object.keys(process.env))
        if (!(key in saved)) Reflect.deleteProperty(process.env, key);
      Object.assign(process.env, saved);
    }
  };
  test("an absent, relative, symlinked or false binding is NOT GRANTED", async () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-binding-"));
    try {
      const driver = join(root, "tools/selected-backend-ci/drivers/postgres.ts");
      expect(
        await withEnv({ FVOCI_ROOT_CURRENT_BINDING: undefined }, () =>
          loadCurrent("postgres", driver),
        ),
      ).toContain("NOT GRANTED");
      expect(
        await withEnv({ FVOCI_ROOT_CURRENT_BINDING: "relative.json" }, () =>
          loadCurrent("postgres", driver),
        ),
      ).not.toBe("accepted");
      const template = join(directory, "binding.json");
      writeFileSync(template, JSON.stringify({ schema: 1, ready: false }));
      expect(
        await withEnv({ FVOCI_ROOT_CURRENT_BINDING: template }, () =>
          loadCurrent("postgres", driver),
        ),
      ).toContain("NOT GRANTED");
      symlinkSync(template, join(directory, "link.json"));
      expect(
        await withEnv({ FVOCI_ROOT_CURRENT_BINDING: join(directory, "link.json") }, () =>
          loadCurrent("postgres", driver),
        ),
      ).not.toBe("accepted");
      // A ready binding outside CI and without a local allocation is refused.
      writeFileSync(template, JSON.stringify({ schema: 1, ready: true, source: "a".repeat(40) }));
      expect(
        await withEnv(
          {
            FVOCI_ROOT_CURRENT_BINDING: template,
            FVOCI_SELECTED_EXECUTION_MODE: undefined,
            GITHUB_ACTIONS: undefined,
            CI: undefined,
          },
          () => loadCurrent("postgres", driver),
        ),
      ).not.toBe("accepted");
      expect(
        await withEnv(
          { FVOCI_ROOT_CURRENT_BINDING: template, FVOCI_SELECTED_EXECUTION_MODE: "other" },
          () => loadCurrent("postgres", driver),
        ),
      ).not.toBe("accepted");
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
});

describe("SQLite ON PRODUCT4 report", () => {
  const binding: import("../../../apps/web/e2e-pending/selected-final-contract.ts").FinalBinding = {
    schema: 1,
    backend: "sqlite",
    source: "a".repeat(40),
    tree: "b".repeat(40),
    browser: "chromium",
    flow: "on",
    suite: "products",
    namespace: "tui-" + "c".repeat(20),
    workspaceId: "00000000-0000-0000-0000-000000000001",
    workspaceSlug: "tui-" + "c".repeat(20),
    setupNeeded: false,
    schemaCurrent: true,
    commit: "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
    owner: {
      userId: "00000000-0000-0000-0000-000000000002",
      email: "tui-" + "c".repeat(20) + "-owner@example.invalid",
      password: "fixture-owner",
    },
    member: {
      userId: "00000000-0000-0000-0000-000000000003",
      email: "tui-" + "c".repeat(20) + "-member@example.invalid",
      password: "fixture-member",
    },
    outsider: {
      userId: "00000000-0000-0000-0000-000000000004",
      email: "tui-" + "c".repeat(20) + "-outsider@example.invalid",
      password: "fixture-outsider",
    },
  };
  test("exact current metadata and four unchanged cases are required", async () => {
    const { PRODUCT_TITLES, assertFinalMetadata } =
      await import("../../../apps/web/e2e-pending/selected-final-contract.ts");
    const report = () => ({
      config: {
        workers: 1,
        metadata: {
          actorBindingSha256: "d".repeat(64),
          source: binding.source,
          tree: binding.tree,
          selectedBackend: "sqlite",
          selectedFlow: "on",
          browser: "chromium",
          suite: "products",
          titles: [...PRODUCT_TITLES],
        },
      },
      errors: [],
      stats: { expected: 4, unexpected: 0, flaky: 0, skipped: 0 },
      suites: [
        {
          specs: PRODUCT_TITLES.map((title) => ({
            file: "selected-final-acceptance.ts",
            title,
            ok: true,
            tests: [
              { expectedStatus: "passed", results: [{ status: "passed", retry: 0, errors: [] }] },
            ],
          })),
        },
      ],
    });
    expect(
      validateProductReport(parseJson(JSON.stringify(report())), binding, "d".repeat(64)),
    ).toEqual([...PRODUCT_TITLES]);
    // The parent matches public policy values without initializing the browser
    // SDK. Every metadata field must retain the maintained Zod adapter's guard.
    const checkMetadata = (metadata: unknown) => {
      const value = report();
      return validateProductReport(
        parseJson(JSON.stringify({ ...value, config: { ...value.config, metadata } })),
        binding,
        "d".repeat(64),
      );
    };
    const extended = { ...report().config.metadata, unrelated: true };
    assertFinalMetadata(extended, "d".repeat(64), binding);
    expect(checkMetadata(extended)).toEqual([...PRODUCT_TITLES]);
    for (const key of Object.keys(report().config.metadata)) {
      for (const fault of ["missing", "wrong-value", "wrong-type"]) {
        const metadata = structuredClone(report().config.metadata) as Record<string, unknown>;
        if (fault === "missing") Reflect.deleteProperty(metadata, key);
        else metadata[key] = fault === "wrong-type" ? 1 : key === "titles" ? ["wrong"] : "wrong";
        expect(() => assertFinalMetadata(metadata, "d".repeat(64), binding)).toThrow();
        expect(() => checkMetadata(metadata)).toThrow();
      }
    }
    const changes: ((value: ReturnType<typeof report>) => void)[] = [
      (r) => {
        r.config.metadata.selectedBackend = "libsql-remote";
      },
      (r) => {
        r.config.metadata.source = "e".repeat(40);
      },
      (r) => {
        r.config.metadata.actorBindingSha256 = "f".repeat(64);
      },
      (r) => {
        r.stats.skipped = 1;
      },
      (r) => {
        r.stats.flaky = 1;
      },
      (r) => {
        r.stats.expected = 3;
      },
      (r) => {
        r.suites[0]?.specs.reverse();
      },
      (r) => {
        r.suites[0]?.specs.pop();
      },
      (r) => {
        const result = r.suites[0]?.specs[0]?.tests[0]?.results[0];
        if (result) result.retry = 1;
      },
      (r) => {
        const result = r.suites[0]?.specs[0]?.tests[0]?.results[0];
        if (result) result.status = "skipped";
      },
    ];
    for (const change of changes) {
      const changed = report();
      change(changed);
      expect(() =>
        validateProductReport(parseJson(JSON.stringify(changed)), binding, "d".repeat(64)),
      ).toThrow();
    }
  });
});
