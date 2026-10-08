import { expect, test } from "bun:test";
import type { JSONReport } from "@playwright/test/reporter";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = resolve(import.meta.dir, "../..");
const cli = fileURLToPath(import.meta.resolve("@playwright/test/cli"));
const configs = [
  ["playwright.config.ts", "focused.spec.ts"],
  ["e2e-pending/collab-playwright.config.ts", "focused.spec.ts"],
  ["e2e-s3/playwright.config.ts", "focused.spec.ts"],
  ["e2e-native-ime/playwright.config.ts", "editor.spec.ts"],
  ["e2e-keycloak/keycloak.config.ts", "oidc-keycloak-flow.spec.ts"],
  ["e2e/perf/perf.config.ts", "focused.perf.ts"],
] as const;

function runFixture(config: string, filename: string, ci: string | undefined, focused = true) {
  // Keep package resolution inside this worktree, with a separate testDir and
  // receipts per invocation. No browser, server or external service is needed.
  const directory = mkdtempSync(join(webRoot, ".forbid-only-"));
  const receipt = join(directory, "executed.txt");
  const configPath = join(directory, "fixture.config.ts");
  try {
    writeFileSync(
      configPath,
      `import config from ${JSON.stringify(resolve(webRoot, config))};
       export default {
         ...config,
         testDir: ${JSON.stringify(directory)},
         outputDir: ${JSON.stringify(join(directory, "output"))},
         reporter: [["json"]],
       };`,
    );
    // Use filenames accepted by each real config's unchanged testMatch. The
    // wrapper also preserves workers, retries, testIgnore and forbidOnly.
    writeFileSync(
      join(directory, filename),
      `import { test } from "@playwright/test";
       import { appendFileSync } from "node:fs";
       test("ordinary", () => appendFileSync(${JSON.stringify(receipt)}, "ordinary\\n"));
       test${focused ? ".only" : ""}("focused", () => appendFileSync(${JSON.stringify(receipt)}, "focused\\n"));`,
    );
    const result = Bun.spawnSync([process.execPath, cli, "test", "--config", configPath], {
      cwd: webRoot,
      env: {
        PATH: process.env.PATH,
        // Satisfy the existing opt-in guard for the browserless native fixture.
        FVOCI_NATIVE_IME_SESSION: "forbid-only-fixture",
        ...(ci === undefined ? {} : { CI: ci }),
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(result.stderr.toString()).toBe("");
    const report = JSON.parse(result.stdout.toString()) as JSONReport;
    expect(report.config.workers).toBe(1);
    expect(report.config.projects).toHaveLength(1);
    expect(report.config.projects[0]?.retries).toBe(0);
    return {
      code: result.exitCode,
      report,
      executed: existsSync(receipt) ? readFileSync(receipt, "utf8") : "",
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

for (const [config, filename] of configs) {
  for (const ci of ["1", "0"]) {
    test(`${config}: CI=${ci} rejects test.only before any test body runs`, () => {
      const { code, report, executed } = runFixture(config, filename, ci);
      expect(code).not.toBe(0);
      expect(report.config.forbidOnly).toBe(true);
      expect(report.errors).toHaveLength(1);
      expect(report.errors[0]?.message).toContain("focused with '.only' is not allowed");
      expect(report.errors[0]?.message).toContain("'forbidOnly' option");
      expect(report.stats.expected).toBe(0);
      expect(report.stats.unexpected).toBe(0);
      expect(executed).toBe("");
    });
  }
  for (const ci of [undefined, ""]) {
    test(`${config}: CI ${ci === undefined ? "unset" : "empty"} keeps local test.only selection`, () => {
      const { code, report, executed } = runFixture(config, filename, ci);
      expect(code).toBe(0);
      expect(report.config.forbidOnly).toBe(false);
      expect(report.errors).toHaveLength(0);
      expect(report.stats.expected).toBe(1);
      expect(report.stats.unexpected).toBe(0);
      expect(executed).toBe("focused\n");
    });
  }
  test(`${config}: CI=1 still runs both tests without test.only`, () => {
    const { code, report, executed } = runFixture(config, filename, "1", false);
    expect(code).toBe(0);
    expect(report.errors).toHaveLength(0);
    expect(report.stats.expected).toBe(2);
    expect(report.stats.unexpected).toBe(0);
    expect(executed).toBe("ordinary\nfocused\n");
  });
}
