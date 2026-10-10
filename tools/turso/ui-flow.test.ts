import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { write } from "../selected-backend-ci/io.ts";
import { BACKGROUND_TABLES } from "./ui-audit.ts";
import { privateRead, textDigest, UiError, valueDigest, type Record_ } from "./ui-common.ts";
import { Captured, exitedChild, failureOf, fakeScope, must } from "./ui-fakes.ts";
import { consumeIn, executeUi, memberWrapper, type FlowSteps } from "./ui-flow.ts";
import { recheckPhysical, type Manifest } from "./ui-record.ts";

let directory: string;
const saved = { ...process.env };
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-flow-"));
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
  for (const key of Object.keys(process.env))
    if (!(key in saved)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, saved);
});

const before = (): Record_ => ({
  ledger: [],
  schemaSha256: "a".repeat(64),
  lineage: "fvoci-sqlite-060",
  startupHazards: 0,
  liveOutboxLeases: 0,
  setupNeeded: false,
  fingerprints: Object.fromEntries(BACKGROUND_TABLES.map((t) => [t, {}])),
});

describe("consumer flow", () => {
  test("a failed browser still observes the primary and keeps the original when the receipt or log fails", async () => {
    const owner = {
      namespace: "tui-" + "a".repeat(20),
      userId: "cccccccc-cccc-cccc-cccc-cccccccccccc",
      workspaceId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
      email: "tui-" + "a".repeat(20) + "-owner@example.invalid",
      commit: "confirmed",
      freshPrimaryReadback: true,
      lifecycleDrain: "confirmed",
      leases: 0,
    };
    const manifest = {
      sourceInputs: { source: "a".repeat(40), tree: "b".repeat(40), files: {} },
      binaries: { "collab-engine": { path: "/never-executed/engine" } },
    } as unknown as Manifest;
    for (const fault of ["none", "receipt", "log", "closure"]) {
      const root = mkdtempSync(join(directory, "root-"));
      writeFileSync(join(root, "current-build.json"), "pure pinned metadata");
      const modes: string[] = [];
      const output = new Captured();
      const { scope } = fakeScope({ closure: () => fault !== "closure" });
      const steps: FlowSteps = {
        fixture: (_scope, _manifest, mode) => {
          modes.push(mode);
          return Promise.resolve(mode === "owner" ? owner : before());
        },
        start: () =>
          Promise.resolve({
            server: { child: exitedChild(0, "", 123) },
            base: "http://127.0.0.1:12345",
            log: fault === "log" ? 987654 : openSync(join(root, "server.private.log"), "w", 0o600),
          }),
        stop: () =>
          Promise.resolve({ serverExit: 0, portClosed: true, recordedIdentitiesRetired: true }),
        browser: () => Promise.reject(new UiError("UI_ACTUAL_BROWSER_FAILED")),
        serverIdentity: () => ({ pid: 123, startTicks: "200" }),
        maintenanceReceipts: () => ({
          identity: { pid: 123, startTicks: "200" },
          targetSha256: "a".repeat(64),
          receipts: [],
        }),
        currentBuild: () => manifest,
        write: (path, value) => {
          if (fault === "receipt" && path.endsWith("ui-result.private.json"))
            throw new Error("PRIVATE_INVENTED_TOKEN");
          write(path, value);
        },
        root: () => root,
        token: (bytes) => "a".repeat(bytes * 2),
        output,
      };
      expect(
        await failureOf(
          executeUi(
            scope,
            manifest,
            before(),
            { FVOCI_LIBSQL_URL: "libsql://invented.invalid" },
            steps,
          ),
        ),
      ).toBe("UI_ACTUAL_BROWSER_FAILED");
      expect(modes).toEqual(fault === "closure" ? ["owner"] : ["owner", "baseline"]);
      expect(existsSync(join(root, "preservation.private.json"))).toBe(fault !== "closure");
      if (fault === "receipt") {
        expect(output.all).toContain("UI_ACTUAL_BROWSER_FAILED");
        expect(output.all).toContain('"receiptWrite": "failed"');
      } else {
        const result = JSON.parse(
          readFileSync(join(root, "ui-result.private.json"), "utf8"),
        ) as Record_;
        expect(result.originalFailure).toBe("UI_ACTUAL_BROWSER_FAILED");
        expect(result.uiResult).toBe("FAIL");
        if (fault === "closure") expect((result.preservation as Record_).result).toBe("NOTRUN");
        if (fault === "log") expect(result.cleanupErrors).toEqual(["UI_SERVER_LOG_CLOSE_FAILED"]);
      }
      expect(output.all).not.toContain("PRIVATE_INVENTED_TOKEN");
    }
  });

  test("background work refuses before any allocation", async () => {
    const { scope } = fakeScope();
    const wrong = before();
    (wrong.fingerprints as Record<string, Record_>).events = { ["f".repeat(64)]: 1 };
    expect(await failureOf(executeUi(scope, {} as Manifest, wrong, {}))).toContain(
      "UI_EXISTING_BACKGROUND_WORK_REFUSED",
    );
  });

  test("an identical dataset on a changed target is refused before mutation and the VAPID keyring stays absent", async () => {
    const baseline = { rows: 1, setupNeeded: false };
    const manifest = { sourceInputs: { source: "a".repeat(40) } } as unknown as Manifest;
    const target = "libsql://pure-original.invalid";
    const inputs = {
      ui_source_sha: "a".repeat(40),
      ui_baseline_sha256: valueDigest(baseline),
      ui_target_sha256: textDigest(target),
    };
    for (const [endpoint, accepted] of [
      [target, true],
      ["libsql://pure-changed.invalid", false],
    ] as const) {
      for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
      Object.assign(process.env, {
        FVOCI_LIBSQL_URL: endpoint,
        FVOCI_LIBSQL_AUTH_TOKEN: "invented",
      });
      const seen: Record<string, string>[] = [];
      const output = new Captured();
      const { scope } = fakeScope();
      const run = consumeIn(scope, "ui-ack", inputs, {
        currentBuild: () => manifest,
        fixture: () => Promise.resolve(baseline),
        executeUi: (_scope, _manifest, _baseline, environment) => {
          seen.push(environment);
          return Promise.resolve({ cleanupErrors: [] } as never);
        },
        write,
        root: () => directory,
        output,
      });
      if (accepted) {
        await run;
        expect(seen[0]).not.toHaveProperty("ENCRYPTION_KEYS");
        expect(seen[0]).not.toHaveProperty("ENCRYPTION_ACTIVE_KEY_ID");
        expect(must(seen[0]).FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE).toBe("true");
        expect(
          Object.keys(JSON.parse(must(must(seen[0]).PASSWORD_PEPPER_KEYS)) as Record_),
        ).toEqual(["fixture"]);
        expect(output.stdout).toEqual([
          "TURSO_UI_ACK_PASS on=1 restart=1 off=8 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN",
        ]);
      } else {
        expect(await failureOf(run)).toContain("UI_CURRENT_TARGET_BINDING_REQUIRED");
        expect(seen).toEqual([]);
      }
      rmSync(join(directory, "baseline.private.json"));
    }
  });

  test("the baseline phase prints the dataset and target binding only", async () => {
    Object.assign(process.env, {
      FVOCI_LIBSQL_URL: "libsql://pure.invalid",
      FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN",
    });
    const baseline = {
      rows: 3,
      setupNeeded: true,
      startupHazards: 0,
      liveOutboxLeases: 0,
      fingerprints: Object.fromEntries(BACKGROUND_TABLES.map((t) => [t, {}])),
    };
    const output = new Captured();
    const { scope } = fakeScope();
    const steps = {
      currentBuild: () => ({ sourceInputs: { source: "a".repeat(40) } }) as unknown as Manifest,
      fixture: () => Promise.resolve(baseline),
      executeUi: () => Promise.reject(new Error("unexpected")),
      write,
      root: () => directory,
      output,
    };
    expect(
      await failureOf(consumeIn(scope, "ui-baseline", { ui_source_sha: "b".repeat(40) }, steps)),
    ).toContain("UI_REVIEWED_SOURCE_REQUIRED");
    await consumeIn(scope, "ui-baseline", { ui_source_sha: "a".repeat(40) }, steps);
    expect(output.stdout).toEqual([
      "TURSO_UI_BASELINE_PASS source=" +
        "a".repeat(40) +
        " baseline_sha256=" +
        valueDigest(baseline) +
        " target_sha256=" +
        textDigest("libsql://pure.invalid") +
        " rows=3 setup_needed=true startup_admissible=true",
    ]);
    expect(output.all).not.toContain("PRIVATE_TOKEN");
    expect(privateRead(join(directory, "baseline.private.json"))).toEqual(baseline);
  });

  test("a physical file mutation refuses even when metadata is identical", () => {
    const paths = [
      "libsqlite3.a",
      "sqlite3.h",
      "rustc",
      "sysroot",
      "registry",
      "config",
      "libclang",
      "cc",
      "ar",
    ].map((n) => join(directory, n));
    for (const path of paths) writeFileSync(path, "original physical bytes");
    const collect = () => ({
      files: Object.fromEntries(paths.map((p) => [p, textDigest(readFileSync(p, "utf8"))])),
      buildEnvironment: {},
    });
    const recorded = collect();
    recheckPhysical(recorded, collect);
    for (const path of paths) {
      writeFileSync(path, "changed physical bytes");
      expect(() => {
        recheckPhysical(recorded, collect);
      }).toThrow("UI_PHYSICAL_BUILD_INPUTS_CHANGED");
      writeFileSync(path, "original physical bytes");
    }
  });

  test("the private actor capsule rejects a symlink, loose permissions and oversize", () => {
    const path = join(directory, "capsule");
    write(path, { synthetic: true });
    expect(privateRead(path)).toEqual({ synthetic: true });
    const alias = join(directory, "alias");
    symlinkSync(path, alias);
    expect(() => privateRead(alias)).toThrow("UI_PRIVATE_INPUT_REFUSED");
    chmodSync(path, 0o644);
    expect(() => privateRead(path)).toThrow("UI_PRIVATE_INPUT_REFUSED");
    chmodSync(path, 0o600);
    expect(() => privateRead(path, 1)).toThrow("UI_PRIVATE_INPUT_REFUSED");
  });

  test("the member wrapper re-enters this CLI with the running Bun", () => {
    const text = memberWrapper();
    expect(text.startsWith("#!/bin/sh\nexec ")).toBe(true);
    expect(text).toContain(process.execPath);
    expect(text.endsWith("tools/turso/ui.ts --actor\n")).toBe(true);
  });
});
