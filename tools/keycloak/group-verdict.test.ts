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

function verdict(report: unknown, status = 0, mode = "flows") {
  const file = join(dir, "out", `playwright-${mode}.json`);
  if (report === undefined) rmSync(file, { force: true });
  else writeFileSync(file, JSON.stringify(report));
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

  test("workspace SSO keeps its Rust result path", () => {
    writeFileSync(join(dir, "out", "workspace-sso.log"), "test result: ok. 1 passed; 0 failed\n");
    const result = verdict(undefined, 0, "workspace-sso");
    expect(result.code).toBe(0);
    expect(result.stderr).toContain("test result: ok. 1 passed; 0 failed");
  });
});
