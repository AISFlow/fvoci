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

  test("a real pass stays successful when tests from other groups are skipped", () => {
    const result = verdict(stats(1, 15));
    expect(result.code).toBe(0);
    expect(result.stderr).toContain("passed 1, failed 0, flaky 0, skipped 15");
  });

  test("a nonzero Playwright exit still fails even if some tests passed", () => {
    expect(verdict(stats(1), 7).code).toBe(1);
  });

  test("no tests, missing reports and invalid passed counts cannot succeed", () => {
    for (const report of [stats(0), undefined, null, {}, stats("1"), stats(-1), stats(0.5)]) {
      expect(verdict(report).code).toBe(1);
    }
  });

  test("positive passes cannot hide failed, flaky or interrupted browser tests", () => {
    for (const key of ["unexpected", "flaky", "interrupted"]) {
      for (const value of [1, -1, 0.5, "0", null]) {
        expect(verdict({ stats: { ...stats(1).stats, [key]: value } }).code).toBe(1);
      }
    }
    expect(verdict({ ...stats(1), errors: [{ message: "interrupted" }] }).code).toBe(1);
    expect(verdict({ ...stats(1), errors: [] }).code).toBe(0);
    for (const status of ["interrupted", "timedOut"]) {
      expect(
        verdict({
          ...stats(1, 1),
          suites: [{ suites: [{ specs: [{ tests: [{ results: [{ status }] }] }] }] }],
        }).code,
      ).toBe(1);
    }
    for (const key of ["unexpected", "flaky", "skipped"]) {
      const incomplete: Record<string, unknown> = { ...stats(1).stats };
      delete incomplete[key];
      expect(verdict({ stats: incomplete }).code).toBe(1);
    }
  });

  test("workspace SSO requires the selected Rust test to pass", () => {
    const result = verdict(undefined, 0, "workspace-sso", cargoPass);
    expect(result.code).toBe(0);
    expect(result.stderr).toContain("test result: ok. 1 passed; 0 failed");
    expect(verdict(undefined, 0, "workspace-sso", cargoPass.replaceAll("\n", "\r\n")).code).toBe(0);
    expect(verdict(undefined, 7, "workspace-sso", cargoPass).code).toBe(1);
  });

  test("zero-exit Rust groups reject zero tests, wrong filters, failures and absent summaries", () => {
    for (const log of [
      undefined,
      "",
      "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out; finished in 0.00s\n",
      cargoPass.replace(ssoTest, "unrelated_test"),
      cargoPass.replace("running 1 test", "running 2 tests"),
      cargoPass.replace("0 failed", "1 failed"),
      cargoPass.replace("0 ignored", "1 ignored"),
      cargoPass.replace("0 measured", "1 measured"),
      cargoPass.replace("ok. 1 passed", "FAILED. 0 passed"),
      cargoPass.replace(" ... ok", " ... ignored"),
      cargoPass.split("test result:")[0],
      cargoPass.slice(cargoPass.indexOf("test result:")),
      `${cargoPass}${cargoPass}`,
    ]) {
      expect(verdict(undefined, 0, "workspace-sso", log).code).toBe(1);
    }
  });
});
