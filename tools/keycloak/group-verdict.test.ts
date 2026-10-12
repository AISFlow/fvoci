import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HELPER } from "./fixtures.ts";

const runner = readFileSync(join(import.meta.dir, "../../scripts/keycloak-oidc-e2e.sh"), "utf8");
const marker = 'echo "=== summary (Keycloak ';
const start = runner.indexOf(marker);
if (start < 0) throw new Error("Keycloak runner summary block is missing");
const summary = runner.slice(start);
let dir = "";
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "kc-group-verdict-"));
  mkdirSync(join(dir, "out"));
});
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

function verdict(report: unknown, status = 0, mode = "flows", log?: string) {
  const file = join(dir, "out", `playwright-${mode}.json`);
  if (report === undefined) rmSync(file, { force: true });
  else writeFileSync(file, JSON.stringify(report));
  const logFile = join(dir, "out", `${mode}.log`);
  if (log === undefined) rmSync(logFile, { force: true });
  else writeFileSync(logFile, log);
  const child = Bun.spawnSync(
    [
      "bash",
      "-euo",
      "pipefail",
      "-c",
      `
kc_bun=$1
kc_helper=$2
WORK=$3
KC_IMAGE=fixture
ISSUER=fixture
MODES=("$5")
declare -A GROUP_STATUS=(["$5"]=$4)
kc() { "$kc_bun" --no-env-file "$kc_helper" "$@"; }
${summary}`,
      "--",
      process.execPath,
      HELPER,
      dir,
      String(status),
      mode,
    ],
    { env: { PATH: process.env.PATH ?? "" }, stdout: "pipe", stderr: "pipe" },
  );
  return { code: child.exitCode, stdout: child.stdout.toString(), stderr: child.stderr.toString() };
}

const stats = (expected: unknown, skipped = 0) => ({
  stats: { expected, unexpected: 0, flaky: 0, skipped },
});
type BrowserMode = "flows" | "failures" | "wrong-secret";
type BrowserTest = {
  expectedStatus: string;
  status: string;
  annotations: { type: string; description?: string }[];
  results: { status: string }[];
};
// Match the single-project JSON reporter tree of oidc-keycloak-flow.spec.ts.
function browserReport(mode: BrowserMode = "flows") {
  const counts = { flows: 8, failures: 7, "wrong-secret": 1 };
  return {
    ...stats(counts[mode], 16 - counts[mode]),
    errors: [],
    suites: [
      {
        specs: [],
        suites: Object.entries(counts).map(([group, count]) => ({
          specs: Array.from({ length: count }, () => {
            const skipped = group !== mode;
            const item: BrowserTest = {
              expectedStatus: skipped ? "skipped" : "passed",
              status: skipped ? "skipped" : "expected",
              annotations: skipped
                ? [{ type: "skip", description: `FVOCI_KC_E2E_MODE=${group}` }]
                : [],
              results: [{ status: skipped ? "skipped" : "passed" }],
            };
            return { ok: true, tests: [item] };
          }),
        })),
      },
    ],
  };
}
const browserTests = (report: ReturnType<typeof browserReport>) =>
  report.suites.flatMap((suite) =>
    suite.suites.flatMap((group) => group.specs.flatMap((spec) => spec.tests)),
  );
function at<T>(items: T[], index = 0): T {
  const item = items[index];
  if (item === undefined) throw new Error("missing browser fixture item");
  return item;
}
const fixtureTest = (report: ReturnType<typeof browserReport>, index: number) =>
  at(browserTests(report), index);
const firstGroup = (report: ReturnType<typeof browserReport>) => at(at(report.suites).suites);
const firstSpec = (report: ReturnType<typeof browserReport>) => at(firstGroup(report).specs);
const ssoTest = "keycloak_workspace_sso_with_a_test_entitlement";
const cargoPass = `running 1 test
test ${ssoTest} ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.01s
`;

describe("Keycloak runner group verdict from reporter fixtures", () => {
  test("a zero-exit all-skipped group fails", () => {
    const result = verdict(stats(0, 16));
    expect(result.code).toBe(1);
    expect(result.stderr).toContain("group flows: exit 1");
    expect(result.stderr).toContain("passed 0, failed 0, flaky 0, skipped 16");
  });

  test.each(["flows", "failures", "wrong-secret"] as const)(
    "a complete %s group passes with only other-mode skips",
    (mode) => {
      const report = browserReport(mode);
      const result = verdict(report, 0, mode);
      expect(result.code).toBe(0);
      expect(result.stderr).toContain(
        `passed ${String(report.stats.expected)}, failed 0, flaky 0, skipped ${String(report.stats.skipped)}`,
      );
    },
  );

  test("a nonzero Playwright exit still fails even if some tests passed", () => {
    expect(verdict(browserReport(), 7).code).toBe(1);
  });

  test("no tests, missing reports and invalid passed counts cannot succeed", () => {
    for (const report of [stats(0), undefined, null, {}, stats("1"), stats(-1), stats(0.5)]) {
      expect(verdict(report).code).toBe(1);
    }
  });

  test("positive passes cannot hide failed, flaky or interrupted browser tests", () => {
    for (const key of ["unexpected", "flaky", "interrupted"]) {
      for (const value of [1, -1, 0.5, "0", null]) {
        const report = browserReport();
        expect(verdict({ ...report, stats: { ...report.stats, [key]: value } }).code).toBe(1);
      }
    }
    expect(verdict({ ...browserReport(), errors: [{ message: "interrupted" }] }).code).toBe(1);
    expect(verdict({ ...browserReport(), errors: [] }).code).toBe(0);
    for (const status of ["interrupted", "timedOut"]) {
      const report = browserReport();
      at(fixtureTest(report, 0).results).status = status;
      expect(verdict(report).code).toBe(1);
    }
    for (const key of ["unexpected", "flaky", "skipped"]) {
      const incomplete = Object.fromEntries(
        Object.entries(browserReport().stats).filter(([name]) => name !== key),
      );
      expect(verdict({ ...browserReport(), stats: incomplete }).code).toBe(1);
    }
    for (const skipped of [-1, 0.5, "8", null]) {
      const report = browserReport();
      expect(verdict({ ...report, stats: { ...report.stats, skipped } }).code).toBe(1);
    }
  });

  test("review reproduction 1: did-not-run cannot count as an intended mode skip", () => {
    expect(
      verdict({
        ...stats(1, 15),
        suites: [
          { specs: [{ tests: [{ expectedStatus: "passed", results: [{ status: "skipped" }] }] }] },
        ],
      }).code,
    ).toBe(1);
  });
  test("review reproduction 2: an empty result cannot count as a skip", () => {
    expect(
      verdict({
        ...stats(1, 1),
        suites: [
          { specs: [{ tests: [{ expectedStatus: "passed", status: "skipped", results: [] }] }] },
        ],
      }).code,
    ).toBe(1);
  });
  test("review reproduction 3: counts outside the spec composition fail", () => {
    expect(verdict(stats(1)).code).toBe(1);
  });
  test("review reproduction 4: correct flows counts without suites fail", () => {
    expect(verdict(stats(8, 8)).code).toBe(1);
  });

  const rejectedBrowserReports: [string, (report: ReturnType<typeof browserReport>) => void][] = [
    [
      "did-not-run with correct counts",
      (r) => {
        fixtureTest(r, 0).results = [{ status: "skipped" }];
      },
    ],
    [
      "missing passing result",
      (r) => {
        fixtureTest(r, 0).results = [];
      },
    ],
    [
      "missing skipped result",
      (r) => {
        fixtureTest(r, 8).results = [];
      },
    ],
    [
      "failed result hidden by stats",
      (r) => {
        at(fixtureTest(r, 0).results).status = "failed";
      },
    ],
    [
      "unexpected expectedStatus",
      (r) => {
        fixtureTest(r, 0).expectedStatus = "failed";
      },
    ],
    [
      "retried passing test",
      (r) => {
        fixtureTest(r, 0).results.push({ status: "passed" });
      },
    ],
    [
      "non-skipped expectedStatus on mode skip",
      (r) => {
        fixtureTest(r, 8).expectedStatus = "passed";
      },
    ],
    [
      "non-skipped result on mode skip",
      (r) => {
        fixtureTest(r, 8).results.push({ status: "passed" });
      },
    ],
    [
      "missing skip annotation",
      (r) => {
        fixtureTest(r, 8).annotations = [];
      },
    ],
    [
      "fixme instead of mode skip",
      (r) => {
        at(fixtureTest(r, 8).annotations).type = "fixme";
      },
    ],
    [
      "unrelated skip",
      (r) => {
        at(fixtureTest(r, 8).annotations).description = "opt-in";
      },
    ],
    [
      "current-mode skip",
      (r) => {
        at(fixtureTest(r, 8).annotations).description = "FVOCI_KC_E2E_MODE=flows";
      },
    ],
    [
      "wrong other-mode composition",
      (r) => {
        at(fixtureTest(r, 8).annotations).description = "FVOCI_KC_E2E_MODE=wrong-secret";
      },
    ],
    [
      "extra skip annotation",
      (r) => {
        fixtureTest(r, 8).annotations.push({ type: "skip", description: "opt-in" });
      },
    ],
    [
      "fixme on passed test",
      (r) => {
        fixtureTest(r, 0).annotations.push({ type: "fixme" });
      },
    ],
    [
      "false spec verdict",
      (r) => {
        firstSpec(r).ok = false;
      },
    ],
    [
      "missing test in spec",
      (r) => {
        firstSpec(r).tests = [];
      },
    ],
    [
      "extra project test",
      (r) => {
        const spec = firstSpec(r);
        spec.tests.push(structuredClone(at(spec.tests)));
      },
    ],
    [
      "spec added without updating contract",
      (r) => {
        const group = firstGroup(r);
        group.specs.push(structuredClone(at(group.specs)));
        r.stats.expected = 9;
      },
    ],
    [
      "spec removed without updating contract",
      (r) => {
        firstGroup(r).specs.pop();
        r.stats.expected = 7;
      },
    ],
    [
      "statistics differ from results",
      (r) => {
        r.stats.expected = 7;
        r.stats.skipped = 9;
      },
    ],
    [
      "empty suites",
      (r) => {
        r.suites = [];
      },
    ],
  ];
  test.each(rejectedBrowserReports)("zero-exit browser group rejects %s", (_reason, change) => {
    const report = browserReport();
    change(report);
    expect(verdict(report).code).toBe(1);
  });
  test("the report must match the requested mode", () => {
    expect(verdict(browserReport("flows"), 0, "failures").code).toBe(1);
    expect(verdict(browserReport(), 0, "unknown").code).toBe(1);
  });

  test("workspace SSO requires the selected Rust test to pass", () => {
    const result = verdict(undefined, 0, "workspace-sso", cargoPass);
    expect(result.code).toBe(0);
    expect(result.stderr).toContain("test result: ok. 1 passed; 0 failed");
    expect(verdict(undefined, 0, "workspace-sso", cargoPass.replaceAll("\n", "\r\n")).code).toBe(0);
    expect(verdict(undefined, 7, "workspace-sso", cargoPass).code).toBe(1);
  });

  const rejectedCargoLogs: [string, string | undefined][] = [
    ["missing log", undefined],
    ["empty log", ""],
    [
      "zero tests",
      "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out; finished in 0.00s\n",
    ],
    ["wrong filter", cargoPass.replace(ssoTest, "unrelated_test")],
    ["inconsistent count", cargoPass.replace("running 1 test", "running 2 tests")],
    ["failed tests", cargoPass.replace("0 failed", "1 failed")],
    ["ignored tests", cargoPass.replace("0 ignored", "1 ignored")],
    ["measured tests", cargoPass.replace("0 measured", "1 measured")],
    ["failed summary", cargoPass.replace("ok. 1 passed", "FAILED. 0 passed")],
    ["ignored result", cargoPass.replace(" ... ok", " ... ignored")],
    ["missing summary", cargoPass.split("test result:")[0]],
    ["missing execution", cargoPass.slice(cargoPass.indexOf("test result:"))],
    ["duplicate execution", `${cargoPass}${cargoPass}`],
  ];
  test.each(rejectedCargoLogs)("zero-exit Rust group rejects %s", (_reason, log) => {
    expect(verdict(undefined, 0, "workspace-sso", log).code).toBe(1);
  });
});
