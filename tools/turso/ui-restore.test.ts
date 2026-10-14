import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, openSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { write } from "../selected-backend-ci/io.ts";
import { BACKGROUND_TABLES, OFF, reportCases, RESTORE } from "./ui-audit.ts";
import { failureCode, privateRead, UiError, valueDigest, type Record_ } from "./ui-common.ts";
import { Captured, clone, exitedChild, failureOf, fakeScope } from "./ui-fakes.ts";
import { consumeIn, type FlowSteps } from "./ui-flow.ts";
import type { Manifest } from "./ui-record.ts";
import { executeRestore, RESTORE_TITLES, type RestoreReceipt } from "./ui-restore.ts";

test.each(["sqlite", "libsql-remote"] as const)(
  "restore %s report refuses missing, zero, retry, skip and wrong backend",
  (backend) => {
    const report = {
      config: { workers: 1, metadata: { selectedBackend: backend } },
      errors: [],
      stats: { expected: 6, unexpected: 0, flaky: 0, skipped: 0 },
      suites: [
        {
          specs: RESTORE_TITLES.map((title) => ({
            file: RESTORE,
            title,
            ok: true,
            tests: [
              {
                expectedStatus: "passed",
                results: [{ status: "passed", retry: 0, errors: [], attachments: [] }],
              },
            ],
          })),
        },
      ],
    };
    expect(reportCases(report, RESTORE, [...RESTORE_TITLES], backend)).toHaveLength(6);
    for (const fault of ["zero", "missing", "retry", "skip", "backend", "file"]) {
      const wrong = clone(report);
      const specs = wrong.suites[0]?.specs;
      if (!specs) throw new Error("test report missing");
      if (fault === "zero") {
        specs.splice(0);
        wrong.stats.expected = 0;
      }
      if (fault === "missing") specs.pop();
      if (fault === "retry") {
        const result = specs[0]?.tests[0]?.results[0];
        if (!result) throw new Error("test result missing");
        result.retry = 1;
      }
      if (fault === "skip") wrong.stats.skipped = 1;
      if (fault === "backend")
        wrong.config.metadata.selectedBackend = backend === "sqlite" ? "libsql-remote" : "sqlite";
      if (fault === "file" && specs[0]) specs[0].file = OFF;
      expect(() => reportCases(wrong, RESTORE, [...RESTORE_TITLES], backend), fault).toThrow();
    }
    expect(() => reportCases(report, OFF, [...RESTORE_TITLES], "sqlite")).toThrow();
  },
);

test("restore consumer keeps exact dataset/target binding and refuses incomplete receipts", async () => {
  const directory = mkdtempSync(join(tmpdir(), "fvoci-restore-admission-"));
  const saved = { ...process.env };
  try {
    process.env.FVOCI_LIBSQL_URL = "libsql://invented-restore.invalid";
    process.env.FVOCI_LIBSQL_AUTH_TOKEN = "invented";
    process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS = JSON.stringify({
      "turso-test": "ab".repeat(32),
    });
    const baseline = { rows: 0 };
    const manifest = { sourceInputs: { source: "a".repeat(40) } } as unknown as Manifest;
    const inputs = {
      ui_source_sha: manifest.sourceInputs.source,
      ui_baseline_sha256: valueDigest(baseline),
      ui_target_sha256: (await import("../selected-backend-ci/io.ts")).digest(
        process.env.FVOCI_LIBSQL_URL,
      ),
    };
    const pass = {
      result: "PASS",
      originalFailure: null,
      cleanupErrors: [],
      auditErrors: [],
      counts: [1, 1, 1, 1, 1, 1],
      preservation: { result: "PASS" },
      sourceUnchangedDuringDestination: "PASS",
    } as unknown as RestoreReceipt;
    for (const fault of [
      "none",
      "dataset",
      "target",
      "source",
      "zero",
      "missing",
      "cleanup",
      "audit",
      "preservation",
      "source-change",
      "failure",
    ] as const) {
      let executed = false;
      const output = new Captured();
      const receipt = clone(pass);
      if (fault === "zero") receipt.counts = [0, 0, 0, 0, 0, 0];
      if (fault === "missing") receipt.counts.pop();
      if (fault === "cleanup") receipt.cleanupErrors.push("RESTORE_CLOSE_FAILED");
      if (fault === "audit") receipt.auditErrors.push("UI_OWNERSHIP_AUDIT_FAILED");
      if (fault === "preservation") receipt.preservation.result = "FAIL";
      if (fault === "source-change") receipt.sourceUnchangedDuringDestination = "FAIL";
      if (fault === "failure") receipt.originalFailure = "RESTORE_ACTUAL_BROWSER_FAILED";
      const given = { ...inputs };
      if (fault === "dataset") given.ui_baseline_sha256 = "b".repeat(64);
      if (fault === "target") given.ui_target_sha256 = "b".repeat(64);
      if (fault === "source") given.ui_source_sha = "b".repeat(40);
      const run = consumeIn(fakeScope().scope, "restore", given, {
        currentBuild: () => manifest,
        fixture: () => Promise.resolve(baseline),
        executeUi: () => {
          throw new Error("wrong consumer");
        },
        executeRestore: (_scope, _manifest, _baseline, env) => {
          executed = true;
          expect(env.ENCRYPTION_ACTIVE_KEY_ID).toBe("turso-test");
          expect(env).not.toHaveProperty("FVOCI_TEST_TURSO_ENCRYPTION_KEYS");
          return Promise.resolve(receipt);
        },
        write,
        root: () => {
          const path = join(directory, fault);
          if (!existsSync(path)) mkdirSync(path, 0o700);
          return path;
        },
        output,
      });
      if (fault === "none") {
        await run;
        expect(output.stdout[0]).toStartWith("TURSO_RESTORE_PASS");
      } else {
        expect(await failureOf(run)).not.toBeNull();
        expect(output.stdout).toEqual([]);
      }
      expect(executed).toBe(!["dataset", "target", "source"].includes(fault));
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
    for (const key of Object.keys(process.env))
      if (!(key in saved)) Reflect.deleteProperty(process.env, key);
    Object.assign(process.env, saved);
  }
});

test.each([false, true])(
  "restore original browser failure survives cleanup failure=%s and primary is observed",
  async (cleanupFailure) => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-restore-failure-"));
    const baseline: Record_ = {
      ledger: [],
      schemaSha256: "a".repeat(64),
      lineage: "fvoci-sqlite-060",
      startupHazards: 0,
      liveOutboxLeases: 0,
      setupNeeded: true,
      fingerprints: Object.fromEntries(BACKGROUND_TABLES.map((table) => [table, {}])),
    };
    const manifest = {
      sourceInputs: { source: "a".repeat(40), tree: "b".repeat(40) },
      binaries: { "collab-engine": { path: "/never-executed" } },
    } as unknown as Manifest;
    const modes: string[] = [];
    const scope = fakeScope().scope;
    const log = openSync(join(directory, "source.private.log"), "w", 0o600);
    const steps: FlowSteps = {
      fixture: (_scope, _manifest, mode) => {
        modes.push(mode);
        return Promise.resolve(baseline);
      },
      start: () =>
        Promise.resolve({ server: { child: exitedChild(0) }, base: "http://127.0.0.1:12345", log }),
      stop: () =>
        cleanupFailure
          ? Promise.reject(new UiError("UI_SERVER_CLOSURE_FAILED"))
          : Promise.resolve({ serverExit: 0, portClosed: true, recordedIdentitiesRetired: true }),
      browser: () => Promise.reject(new UiError("RESTORE_ACTUAL_BROWSER_FAILED")),
      serverIdentity: () => ({ pid: 4242, startTicks: "2" }),
      maintenanceReceipts: () => ({
        identity: { pid: 4242, startTicks: "2" },
        targetSha256: "a".repeat(64),
        receipts: [],
      }),
      currentBuild: () => manifest,
      write,
      root: () => directory,
      token: (bytes) => "a".repeat(bytes * 2),
      output: new Captured(),
    };
    try {
      expect(
        await failureOf(
          executeRestore(
            scope,
            manifest,
            baseline,
            { FVOCI_LIBSQL_URL: "libsql://invented.invalid" },
            steps,
          ),
        ),
      ).toBe("RESTORE_ACTUAL_BROWSER_FAILED");
      expect(modes).toEqual(["baseline"]);
      const receipt = privateRead(
        join(directory, "restore/restore-result.private.json"),
      ) as Record_;
      expect(receipt.originalFailure).toBe("RESTORE_ACTUAL_BROWSER_FAILED");
      expect(receipt.result).toBe("FAIL");
      expect(receipt.counts).toEqual([0, 0, 0, 0, 0, 0]);
      expect(receipt.cleanupErrors).toEqual(cleanupFailure ? ["UI_SERVER_CLOSURE_FAILED"] : []);
      if (cleanupFailure)
        expect(scope.allocations.every((allocation) => allocation.closed)).toBe(true);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);

test("failure projection preserves only fixed restore codes", () => {
  expect(failureCode(new UiError("RESTORE_NATIVE_ACK_MISMATCH"))).toBe(
    "RESTORE_NATIVE_ACK_MISMATCH",
  );
  expect(failureCode(new UiError("RESTORE_PRIVATE/token"))).toBe("UI_CONSUMER_FAILED");
  expect(failureCode(new Error("RESTORE_PRIVATE_CANARY"))).toBe("UI_CONSUMER_FAILED");
});
