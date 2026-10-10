import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { parseJson, root } from "../io.ts";
import { engineFeaturesMatch, loadCurrent, validateOffReport } from "./binding.ts";

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
