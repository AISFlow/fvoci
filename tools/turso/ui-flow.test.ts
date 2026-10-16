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
import { digest, listenerAbsent, write } from "../selected-backend-ci/io.ts";
import { BACKGROUND_TABLES } from "./ui-audit.ts";
import { privateRead, UiError, valueDigest, type Record_ } from "./ui-common.ts";
import { Captured, exitedChild, failureOf, fakeScope, must } from "./ui-fakes.ts";
import { AUTH_CASES, AUTH_SPEC } from "./ui-auth-session.ts";
import {
  admitSerialBaseline,
  consumeIn,
  executeAuth,
  executeUi,
  memberWrapper,
  startSmtpSink,
  stopSmtpSink,
  type AuthSteps,
  type FlowSteps,
} from "./ui-flow.ts";
import { registeredTitles } from "./ui-native.ts";
import { linuxKernel, UiProcesses, withProcesses } from "./ui-processes.ts";
import { recheckPhysical, type Manifest } from "./ui-record.ts";

let directory: string;
const saved = { ...process.env };
const encryptionKeys = JSON.stringify({ "turso-test": "ab".repeat(32) });
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
      process.env.RUNNER_TEMP = root;
      const modes: string[] = [];
      const output = new Captured();
      const { scope } = fakeScope({ closure: () => fault !== "closure" });
      const steps: FlowSteps = {
        fixture: (_scope, _manifest, mode) => {
          modes.push(mode);
          return Promise.resolve(mode === "owner" ? owner : before());
        },
        start: (_scope, _manifest, environment) => {
          expect(environment.ENCRYPTION_KEYS).toBe(encryptionKeys);
          expect(environment.ENCRYPTION_ACTIVE_KEY_ID).toBe("turso-test");
          return Promise.resolve({
            server: { child: exitedChild(0, "", 123) },
            base: "http://127.0.0.1:12345",
            log: fault === "log" ? 987654 : openSync(join(root, "server.private.log"), "w", 0o600),
          });
        },
        stop: () =>
          Promise.resolve({ serverExit: 0, portClosed: true, recordedIdentitiesRetired: true }),
        browser: (_scope, _manifest, _directory, _environment, spec, grep) => {
          const selected = new RegExp(must(grep));
          // Playwright greps the full title, including the spec filename.
          expect(selected.test(spec + " " + must(registeredTitles(spec, "on")[0]))).toBe(true);
          expect(selected.test(spec + " selected normal main restart: persisted readback")).toBe(
            false,
          );
          return Promise.reject(new UiError("UI_ACTUAL_BROWSER_FAILED"));
        },
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
            {
              FVOCI_LIBSQL_URL: "libsql://invented.invalid",
              ENCRYPTION_KEYS: encryptionKeys,
              ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
            },
            steps,
          ),
        ),
      ).toBe("UI_ACTUAL_BROWSER_FAILED");
      expect(modes).toEqual(fault === "closure" ? ["owner"] : ["owner", "baseline"]);
      const actorInput = privateRead(join(root, "on/actor-input.private.json")) as Record_;
      expect(actorInput.environment).not.toHaveProperty("ENCRYPTION_KEYS");
      expect(actorInput.environment).not.toHaveProperty("ENCRYPTION_ACTIVE_KEY_ID");
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

  test("an identical dataset on a changed target is refused before mutation and the fixed keyring reaches only ui-ack", async () => {
    const baseline = { rows: 1, setupNeeded: false };
    const manifest = { sourceInputs: { source: "a".repeat(40) } } as unknown as Manifest;
    const target = "libsql://pure-original.invalid";
    const inputs = {
      ui_source_sha: "a".repeat(40),
      ui_baseline_sha256: valueDigest(baseline),
      ui_target_sha256: digest(target),
    };
    for (const [endpoint, accepted] of [
      [target, true],
      ["libsql://pure-changed.invalid", false],
    ] as const) {
      for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
      Object.assign(process.env, {
        FVOCI_LIBSQL_URL: endpoint,
        FVOCI_LIBSQL_AUTH_TOKEN: "invented",
        FVOCI_TEST_TURSO_ENCRYPTION_KEYS: JSON.stringify(
          JSON.parse(encryptionKeys) as unknown,
          null,
          2,
        ),
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
        expect(must(seen[0]).ENCRYPTION_KEYS).toBe(encryptionKeys);
        expect(must(seen[0]).ENCRYPTION_ACTIVE_KEY_ID).toBe("turso-test");
        expect(seen[0]).not.toHaveProperty("FVOCI_TEST_TURSO_ENCRYPTION_KEYS");
        expect(must(seen[0]).FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE).toBe("true");
        expect(
          Object.keys(JSON.parse(must(must(seen[0]).PASSWORD_PEPPER_KEYS)) as Record_),
        ).toEqual(["fixture"]);
        expect(output.stdout).toEqual([
          "TURSO_UI_ACK_PASS on=1 restart=1 off=8 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN",
        ]);
        expect(output.all).not.toContain(encryptionKeys);
        expect(output.all).not.toContain(digest(encryptionKeys));
      } else {
        expect(await failureOf(run)).toContain("UI_CURRENT_TARGET_BINDING_REQUIRED");
        expect(seen).toEqual([]);
      }
      rmSync(join(directory, "baseline.private.json"));
    }
  });

  test.each([
    ["missing", undefined, "UI_ENCRYPTION_KEYRING_REQUIRED"],
    ["empty", "", "UI_ENCRYPTION_KEYRING_REQUIRED"],
    ["invalid JSON", "PRIVATE_INVALID_JSON", "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["null", "null", "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["array", "[]", "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["no key", "{}", "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["wrong id", JSON.stringify({ fixture: "ab".repeat(32) }), "UI_ENCRYPTION_KEYRING_REFUSED"],
    [
      "extra id",
      JSON.stringify({ "turso-test": "ab".repeat(32), extra: "cd".repeat(32) }),
      "UI_ENCRYPTION_KEYRING_REFUSED",
    ],
    ["non-string", '{"turso-test":64}', "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["short", JSON.stringify({ "turso-test": "a".repeat(63) }), "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["long", JSON.stringify({ "turso-test": "a".repeat(65) }), "UI_ENCRYPTION_KEYRING_REFUSED"],
    ["non-hex", JSON.stringify({ "turso-test": "z".repeat(64) }), "UI_ENCRYPTION_KEYRING_REFUSED"],
    [
      "trailing newline",
      JSON.stringify({ "turso-test": "ab".repeat(32) + "\n" }),
      "UI_ENCRYPTION_KEYRING_REFUSED",
    ],
  ] as const)(
    "ui-ack refuses %s keyring before any fixture or mutation",
    async (_label, value, code) => {
      delete process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS;
      if (value !== undefined) process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS = value;
      const output = new Captured();
      const { scope } = fakeScope();
      const calls: string[] = [];
      const failure = await failureOf(
        consumeIn(
          scope,
          "ui-ack",
          { ui_source_sha: "a".repeat(40) },
          {
            currentBuild: () =>
              ({ sourceInputs: { source: "a".repeat(40) } }) as unknown as Manifest,
            fixture: () => {
              calls.push("fixture");
              return Promise.reject(new Error("unexpected"));
            },
            executeUi: () => {
              calls.push("execute");
              return Promise.reject(new Error("unexpected"));
            },
            write: () => {
              calls.push("write");
            },
            root: () => directory,
            output,
          },
        ),
      );
      expect(failure).toBe(code);
      expect(calls).toEqual([]);
      expect(output.stdout).toEqual([]);
      expect(output.stderr).toEqual([]);
    },
  );

  test("ui-auth binds the observed dataset and target and prints its line only after a full PASS", async () => {
    const baseline = { rows: 1, setupNeeded: false };
    const manifest = { sourceInputs: { source: "a".repeat(40) } } as unknown as Manifest;
    const target = "libsql://pure-original.invalid";
    const bound = {
      ui_source_sha: "a".repeat(40),
      ui_baseline_sha256: valueDigest(baseline),
      ui_target_sha256: digest(target),
    };
    const passed = {
      uiResult: "PASS",
      originalFailure: null,
      cleanupErrors: [],
      auditErrors: [],
      counts: { on: 3, off: 3 },
      preservation: { result: "PASS" },
    };
    for (const [label, inputs, keys, result, expected] of [
      ["pass", bound, encryptionKeys, passed, null],
      ["keyring", bound, undefined, passed, "UI_ENCRYPTION_KEYRING_REQUIRED"],
      [
        "dataset",
        { ...bound, ui_baseline_sha256: "0".repeat(64) },
        encryptionKeys,
        passed,
        "UI_CURRENT_DATASET_BINDING_REQUIRED",
      ],
      [
        "target",
        { ...bound, ui_target_sha256: "0".repeat(64) },
        encryptionKeys,
        passed,
        "UI_CURRENT_TARGET_BINDING_REQUIRED",
      ],
      [
        "counts",
        bound,
        encryptionKeys,
        { ...passed, counts: { on: 3, off: 2 } },
        "UI_AUTH_ACCEPTANCE_FAILED",
      ],
      [
        "preservation",
        bound,
        encryptionKeys,
        { ...passed, preservation: { result: "FAIL" } },
        "UI_AUTH_ACCEPTANCE_FAILED",
      ],
      [
        "cleanup",
        bound,
        encryptionKeys,
        { ...passed, cleanupErrors: ["UI_SMTP_SINK_CLOSURE_FAILED"] },
        "UI_AUTH_ACCEPTANCE_FAILED",
      ],
    ] as const) {
      for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
      Object.assign(process.env, { FVOCI_LIBSQL_URL: target, FVOCI_LIBSQL_AUTH_TOKEN: "invented" });
      if (keys !== undefined) process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS = keys;
      const seen: Record<string, string>[] = [];
      const output = new Captured();
      const { scope } = fakeScope();
      const run = consumeIn(scope, "ui-auth", inputs, {
        currentBuild: () => manifest,
        fixture: () => Promise.resolve(baseline),
        executeUi: () => Promise.reject(new Error("the ack consumer must not run")),
        executeAuth: (_scope, _manifest, _baseline, environment) => {
          seen.push(environment);
          return Promise.resolve(result as never);
        },
        write,
        root: () => directory,
        output,
      });
      if (expected === null) {
        await run;
        expect(must(seen[0]).ENCRYPTION_KEYS).toBe(encryptionKeys);
        expect(output.stdout).toEqual([
          "TURSO_UI_AUTH_PASS on=3 off=3 retries=0 ignored=0 events=PASS preservation=PASS source=libsql-remote cost=NOTRUN",
        ]);
      } else {
        expect([label, await failureOf(run)]).toEqual([label, expected]);
        expect(output.stdout.filter((line) => line.startsWith("TURSO_UI_AUTH_PASS"))).toEqual([]);
        // Every binding refuses before the auth consumer starts any allocation.
        if (expected !== "UI_AUTH_ACCEPTANCE_FAILED") expect(seen).toEqual([]);
      }
      rmSync(join(directory, "baseline.private.json"), { force: true });
    }
  });

  test("the baseline phase prints the dataset and target binding only", async () => {
    Object.assign(process.env, {
      FVOCI_LIBSQL_URL: "libsql://pure.invalid",
      FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN",
      FVOCI_TEST_TURSO_ENCRYPTION_KEYS: encryptionKeys,
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
      fixture: (
        _scope: unknown,
        _manifest: unknown,
        _mode: unknown,
        environment: Record<string, string>,
      ) => {
        expect(Object.keys(environment).sort()).toEqual([
          "FVOCI_LIBSQL_AUTH_TOKEN",
          "FVOCI_LIBSQL_URL",
        ]);
        return Promise.resolve(baseline);
      },
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
        digest("libsql://pure.invalid") +
        " rows=3 setup_needed=true startup_admissible=true startup_blockers=none",
    ]);
    expect(output.all).not.toContain("PRIVATE_TOKEN");
    expect(output.all).not.toContain(encryptionKeys);
    expect(output.all).not.toContain(digest(encryptionKeys));
    expect(privateRead(join(directory, "baseline.private.json"))).toEqual(baseline);
  });

  test("baseline blocker diagnostics reveal only fixed categories and preserve refusal", async () => {
    Object.assign(process.env, {
      FVOCI_LIBSQL_URL: "libsql://private-target.invalid",
      FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN_CANARY",
    });
    for (const blocker of ["none", ...BACKGROUND_TABLES, "startupHazards", "liveOutboxLeases"]) {
      const fingerprints: Record<string, unknown> = Object.fromEntries(
        BACKGROUND_TABLES.map((table) => [table, {}]),
      );
      fingerprints.PRIVATE_TABLE_CANARY = { PRIVATE_ROW_CANARY: 1 };
      if (BACKGROUND_TABLES.some((table) => table === blocker))
        fingerprints[blocker] = { PRIVATE_ROW_CANARY: 1 };
      const baseline = {
        ...before(),
        rows: 27,
        setupNeeded: true,
        fingerprints,
        startupHazards: blocker === "startupHazards" ? 1 : 0,
        liveOutboxLeases: blocker === "liveOutboxLeases" ? 1 : 0,
        privateError: "PRIVATE_ERROR_CANARY",
        privateId: "PRIVATE_ID_CANARY",
      };
      const output = new Captured();
      const { scope } = fakeScope();
      await consumeIn(
        scope,
        "ui-baseline",
        { ui_source_sha: "a".repeat(40) },
        {
          currentBuild: () => ({ sourceInputs: { source: "a".repeat(40) } }) as unknown as Manifest,
          fixture: () => Promise.resolve(baseline),
          executeUi: () => Promise.reject(new Error("baseline must not start a mutation")),
          write,
          root: () => directory,
          output,
        },
      );
      const expected =
        blocker === "startupHazards" || blocker === "liveOutboxLeases"
          ? "existing-owner-or-deletion"
          : blocker;
      expect(output.stdout).toHaveLength(1);
      expect(output.stdout[0]).toStartWith("TURSO_UI_BASELINE_PASS ");
      expect(output.stdout[0]).toEndWith(
        ` rows=27 setup_needed=true startup_admissible=${String(blocker === "none")} startup_blockers=${expected}`,
      );
      for (const canary of ["PRIVATE_", "private-target.invalid"])
        expect(output.all).not.toContain(canary);
      if (blocker === "none")
        expect(() => {
          admitSerialBaseline(baseline);
        }).not.toThrow();
      else
        expect(() => {
          admitSerialBaseline(baseline);
        }).toThrow("UI_EXISTING_BACKGROUND_WORK_REFUSED");
      expect(privateRead(join(directory, "baseline.private.json"))).toEqual(baseline);
      rmSync(join(directory, "baseline.private.json"));
    }
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
      files: Object.fromEntries(paths.map((p) => [p, digest(readFileSync(p, "utf8"))])),
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
    // A leading U+FEFF is data: the decoder keeps it and JSON refuses it.
    const bom = join(directory, "bom");
    writeFileSync(bom, '\ufeff{"synthetic":true}', { mode: 0o600 });
    expect(() => privateRead(bom)).toThrow(SyntaxError);
  });

  test("the member wrapper re-enters this CLI with the running Bun and its RUNNER_TEMP", () => {
    const text = memberWrapper("/runner/temp's");
    expect(
      text.startsWith("#!/bin/sh\nRUNNER_TEMP='/runner/temp'\"'\"'s'\nexport RUNNER_TEMP\nexec "),
    ).toBe(true);
    expect(text).toContain(process.execPath + " --no-env-file ");
    expect(text.endsWith("tools/turso/ui.ts --actor\n")).toBe(true);
    Reflect.deleteProperty(process.env, "RUNNER_TEMP");
    expect(() => memberWrapper()).toThrow();
  });
});

describe("dedicated auth allocations", () => {
  const member = "019b0000-0000-7000-8000-00000000000a";
  const ownerId = "019b0000-0000-7000-8000-00000000000b";
  const workspace = "019b0000-0000-7000-8000-00000000000c";
  const target = "019b0000-0000-7000-8000-00000000000d";
  const source = "a".repeat(40),
    tree = "b".repeat(40);
  const manifest = {
    sourceInputs: { source, tree, files: {} },
    binaries: { "collab-engine": { path: "/never-executed/engine" } },
  } as unknown as Manifest;
  const observation = () => ({
    targetId: target,
    body: { contentJson: { type: "doc", content: [] }, version: 1 },
    ids: [target],
    nativeV1: "AQEBAA==",
    revisions: [{ id: target, targetId: target, ySnapshot: "AAA=" }],
    events: [{ id: target, targetId: target, verb: "document.updated" }],
  });
  const actor = (namespace: string, role: "owner" | "member") => ({
    namespace,
    userId: role === "owner" ? ownerId : member,
    workspaceId: workspace,
    email: namespace + "-" + role + "@example.invalid",
    commit: "confirmed",
    freshPrimaryReadback: true,
    lifecycleDrain: "confirmed",
    leases: 0,
  });
  // The three source-bound witnesses the browser attaches for one flow.
  const report = (flow: "on" | "off") => ({
    config: {
      metadata: {
        authSession: true,
        selectedBackend: "libsql-remote",
        selectedFlow: flow,
        source,
        tree,
      },
      workers: 1,
    },
    errors: [],
    suites: [
      {
        specs: AUTH_CASES.map((title) => ({
          file: AUTH_SPEC,
          title,
          ok: true,
          tests: [
            {
              expectedStatus: "passed",
              results: [
                {
                  status: "passed",
                  retry: 0,
                  errors: [],
                  attachments: [
                    {
                      name: "auth-session-document-effects.json",
                      contentType: "application/json",
                      body: Buffer.from(
                        JSON.stringify({
                          schema: 1,
                          scope: "document-effects",
                          backend: "libsql-remote",
                          flow,
                          source,
                          tree,
                          actor: member,
                          owner: ownerId,
                          workspaceId: workspace,
                          targetId: target,
                          before: observation(),
                          after: observation(),
                          refusal: {
                            boundary: "pre-auth-http",
                            method: flow === "on" ? "POST" : "PUT",
                            status: 401,
                            directStatus: 401,
                          },
                          wrongCurrentStatus: 400,
                          oldPasswordStatus: 401,
                          newPasswordStatus: 200,
                          tamperedStatus: 400,
                          spentStatus: 400,
                        }),
                      ).toString("base64"),
                    },
                  ],
                },
              ],
            },
          ],
        })),
      },
    ],
  });
  type Fault = "browser" | "sink-start" | "sink-stop" | "events" | "background";
  type SourceFault =
    | "none"
    | "index"
    | "label"
    | "identity"
    | "other-live"
    | "unknown-live"
    | "scope-error"
    | "hosted-open";
  async function allocate(fault: Fault, sourceFault?: SourceFault) {
    const root = mkdtempSync(join(directory, "root-"));
    writeFileSync(join(root, "current-build.json"), "pure pinned metadata");
    process.env.RUNNER_TEMP = root;
    const calls: string[] = [];
    const starts: Record<string, string>[] = [];
    const browsers: { environment: Record<string, string>; spec: string; config?: string }[] = [];
    const baselines: Record<string, string>[] = [];
    const output = new Captured();
    const { scope } = fakeScope({
      closure: () => sourceFault === undefined,
      retired: () => false,
    });
    const sourceIdentity = { pid: 71, startTicks: "200" };
    const processErrors: string[] = [];
    if (sourceFault !== undefined) {
      scope.allocations.push({
        process: exitedChild(null, "", sourceIdentity.pid),
        label: sourceFault === "label" ? "unrelated-child" : "official-sqld",
        closed: false,
        forced: false,
      });
      must(scope.allocations[0]).key = scope.capture(
        { ...sourceIdentity, parentPid: 1, state: "S" },
        "official-sqld",
        0,
      );
      if (sourceFault === "other-live")
        scope.allocations.push({
          process: exitedChild(null, "", 72),
          label: "unrelated-child",
          closed: false,
          forced: false,
        });
      if (sourceFault === "unknown-live")
        scope.capture({ pid: 72, startTicks: "201", parentPid: 1, state: "S" }, "unknown");
      if (sourceFault !== "hosted-open") process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    }
    const steps: AuthSteps = {
      smtp: (_scope, _manifest, allocation) => {
        calls.push("smtp");
        if (fault === "sink-start") return Promise.reject(new UiError("UI_SMTP_SINK_START_FAILED"));
        return Promise.resolve({
          child: exitedChild(-15, "", 77),
          port: "2525",
          capture: join(allocation, "smtp-capture.private.jsonl"),
        });
      },
      stopSmtp: () => {
        calls.push("stop-smtp");
        return fault === "sink-stop"
          ? Promise.reject(new UiError("UI_SMTP_SINK_CLOSURE_FAILED"))
          : Promise.resolve();
      },
      fixture: (_scope, _manifest, mode, environment) => {
        calls.push("fixture:" + mode);
        if (mode === "owner")
          return Promise.resolve(actor(must(environment.FVOCI_E2E_TURSO_NAMESPACE), "owner"));
        if (mode === "observe") return Promise.resolve({ rows: { [target]: { roomFences: [] } } });
        baselines.push(environment);
        return Promise.resolve({
          ...before(),
          backend: "libsql-remote",
          allocations: { events: {} },
        });
      },
      start: (_scope, _manifest, environment) => {
        calls.push("start");
        starts.push(environment);
        return Promise.resolve({
          server: { child: exitedChild(0, "", 123) },
          base: "http://127.0.0.1:12345",
          log: openSync(join(root, "server-" + String(starts.length) + ".log"), "w", 0o600),
        });
      },
      stop: () => {
        calls.push("stop");
        return Promise.resolve({
          serverExit: 0,
          portClosed: true,
          recordedIdentitiesRetired: true,
        });
      },
      browser: (_scope, _manifest, allocation, environment, spec, _grep, _workspace, config) => {
        calls.push("browser");
        browsers.push({ environment, spec, config });
        if (sourceFault === "scope-error") processErrors.push("UI_PROCESS_OBSERVATION_FAILED");
        if (fault === "browser" || fault === "sink-stop")
          return Promise.reject(new UiError("UI_ACTUAL_BROWSER_FAILED"));
        const namespace = must(environment.FVOCI_E2E_TURSO_NAMESPACE);
        write(join(allocation, "member-" + namespace + ".json"), actor(namespace, "member"));
        return Promise.resolve(report(environment.FVOCI_E2E_SELECTED_FLOW as "on" | "off"));
      },
      serverIdentity: () => ({ pid: 123, startTicks: "200" }),
      maintenanceReceipts: () => ({
        identity: { pid: 123, startTicks: "200" },
        targetSha256: "a".repeat(64),
        receipts: [],
      }),
      currentBuild: () => manifest,
      write,
      root: () => root,
      token: (bytes) => "c".repeat(bytes * 2),
      output,
    };
    const start = { ...before(), startupHazards: fault === "background" ? 1 : 0 };
    const failure = await failureOf(
      executeAuth(
        scope,
        manifest,
        start,
        {
          FVOCI_LIBSQL_URL: "libsql://invented.invalid",
          FVOCI_STORAGE_DIR: join(root, "bootstrap-storage"),
          ENCRYPTION_KEYS: encryptionKeys,
          ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
        },
        steps,
        undefined,
        sourceFault === undefined || sourceFault === "hosted-open"
          ? undefined
          : {
              sourceAllocation: sourceFault === "index" ? 1 : 0,
              sourceIdentity:
                sourceFault === "identity"
                  ? { ...sourceIdentity, startTicks: "201" }
                  : sourceIdentity,
              processErrors,
            },
      ),
    );
    const receipt = existsSync(join(root, "auth-result.private.json"))
      ? (JSON.parse(readFileSync(join(root, "auth-result.private.json"), "utf8")) as Record_)
      : null;
    return { root, calls, starts, browsers, baselines, failure, receipt, output };
  }

  test("existing background work refuses before any sink, actor or server", async () => {
    const result = await allocate("background");
    expect(result.failure).toBe("UI_EXISTING_BACKGROUND_WORK_REFUSED");
    expect(result.calls).toEqual([]);
  });

  test("each allocation names its owned sink to its own normal server only", async () => {
    const result = await allocate("events");
    expect(result.failure).toBe("UI_AUTH_EVENT_EFFECT_COUNTS_FAILED");
    expect(result.calls).toEqual([
      "smtp",
      "fixture:owner",
      "start",
      "fixture:baseline",
      "browser",
      "fixture:observe",
      "fixture:baseline",
      "stop",
      "stop-smtp",
      "fixture:baseline",
    ]);
    const server = must(result.starts[0]);
    expect(server).toMatchObject({
      FVOCI_COLLAB_MAX_ROOMS: "3",
      SMTP_HOST: "127.0.0.1",
      SMTP_PORT: "2525",
      SMTP_FROM: "auth-session@example.invalid",
      FVOCI_REALTIME_MODE: "on",
      FVOCI_STORAGE_DIR: join(result.root, "auth-on/storage"),
    });
    const browser = must(result.browsers[0]);
    expect(browser.spec).toBe(AUTH_SPEC);
    expect(browser.config).toBe("auth-session-playwright.config.ts");
    expect(browser.environment).toMatchObject({
      FVOCI_E2E_SELECTED_FLOW: "on",
      FVOCI_E2E_AUTH_SOURCE: source,
      FVOCI_E2E_AUTH_TREE: tree,
      FVOCI_E2E_SMTP_CAPTURE: join(result.root, "auth-on/smtp-capture.private.jsonl"),
    });
    const capsule = privateRead(join(result.root, "auth-on/actor-input.private.json")) as Record_;
    for (const key of ["SMTP_HOST", "SMTP_PORT", "SMTP_FROM", "ENCRYPTION_KEYS"])
      expect(capsule.environment).not.toHaveProperty(key);
    // Outside an orca-local allocation the consumer is the hosted run itself.
    expect(result.receipt).toMatchObject({
      hosted: "THIS_RUN",
      counts: { on: 3, off: 0 },
      uiResult: "FAIL",
      originalFailure: "UI_AUTH_EVENT_EFFECT_COUNTS_FAILED",
      cleanupErrors: [],
    });
  });

  test("a browser failure still retires the server and sink and observes the primary", async () => {
    const result = await allocate("browser");
    expect(result.failure).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(result.calls).toEqual([
      "smtp",
      "fixture:owner",
      "start",
      "fixture:baseline",
      "browser",
      "stop",
      "stop-smtp",
      "fixture:baseline",
    ]);
    // The final observation reuses the last allocation environment and its storage.
    expect(result.baselines.at(-1)?.FVOCI_STORAGE_DIR).toBe(join(result.root, "auth-on/storage"));
    expect(result.receipt).toMatchObject({
      counts: { on: 0, off: 0 },
      originalFailure: "UI_ACTUAL_BROWSER_FAILED",
    });
  });

  test("a sink closure failure is recorded without replacing the original failure", async () => {
    const result = await allocate("sink-stop");
    expect(result.failure).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(result.receipt).toMatchObject({
      originalFailure: "UI_ACTUAL_BROWSER_FAILED",
      cleanupErrors: ["UI_SMTP_SINK_CLOSURE_FAILED"],
    });
  });

  test("a sink that never started allocates no actor or server", async () => {
    const result = await allocate("sink-start");
    expect(result.failure).toBe("UI_SMTP_SINK_START_FAILED");
    expect(result.calls).toEqual(["smtp", "fixture:baseline"]);
    expect(result.baselines.at(-1)?.FVOCI_STORAGE_DIR).toBe(join(result.root, "bootstrap-storage"));
  });

  test("only the bound local source remains live for the final preservation read", async () => {
    const result = await allocate("browser", "none");
    expect(result.failure).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(result.calls.filter((call) => call === "fixture:baseline")).toHaveLength(2);
    expect(result.receipt?.preservation).toHaveProperty("afterSha256");
    expect(result.receipt?.originalFailure).toBe("UI_ACTUAL_BROWSER_FAILED");
  });

  for (const fault of ["index", "label", "identity"] as const)
    test(`a local source with wrong ${fault} refuses before any auth allocation`, async () => {
      const result = await allocate("browser", fault);
      expect(result.failure).toBe("UI_LOCAL_SOURCE_REFUSED");
      expect(result.calls).toEqual([]);
    });

  for (const fault of ["other-live", "unknown-live", "scope-error", "hosted-open"] as const)
    test(`preservation refuses ${fault} and keeps the original browser failure`, async () => {
      const result = await allocate("browser", fault);
      expect(result.failure).toBe("UI_ACTUAL_BROWSER_FAILED");
      expect(result.calls.filter((call) => call === "fixture:baseline")).toHaveLength(1);
      expect(result.receipt?.preservation).toMatchObject({
        result: "NOTRUN",
        failure: "UI_RESOURCE_CLOSURE_FAILED",
      });
      expect(result.receipt?.originalFailure).toBe("UI_ACTUAL_BROWSER_FAILED");
    });
});

// A real maintained sink child under the owned process scope.
describe.skipIf(process.platform !== "linux")("owned SMTP sink", () => {
  const bun = { bun: { path: process.execPath, sha256: "", version: "" } } as unknown as Manifest;
  test("its own listener serves the reported port and retires on SIGTERM", async () => {
    const output = new Captured();
    const port = await withProcesses(
      async (scope) => {
        const sink = await startSmtpSink(scope, bun, directory);
        expect(sink.capture).toBe(join(directory, "smtp-capture.private.jsonl"));
        expect(listenerAbsent(Number(sink.port))).toBe(false);
        await stopSmtpSink(scope, sink);
        return Number(sink.port);
      },
      new UiProcesses(linuxKernel, { output, write, root: () => directory }),
    );
    expect(listenerAbsent(port)).toBe(true);
  });
  test("a listener that remains after the exit refuses the closure", async () => {
    const output = new Captured();
    expect(
      await failureOf(
        withProcesses(
          async (scope) => {
            const sink = await startSmtpSink(scope, bun, directory);
            await stopSmtpSink(scope, sink, () => false);
          },
          new UiProcesses(linuxKernel, { output, write, root: () => directory }),
        ),
      ),
    ).toBe("UI_SMTP_SINK_CLOSURE_FAILED");
  });
});
