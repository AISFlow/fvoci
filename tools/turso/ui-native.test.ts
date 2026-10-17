import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
  existsSync,
  fstatSync,
  closeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { sha, write } from "../selected-backend-ci/io.ts";
import { OFF, ON } from "./ui-audit.ts";
import { AUTH_SPEC } from "./ui-auth-session.ts";
import {
  browserFailureDiagnostic,
  type BrowserDiagnosticSources,
} from "./ui-browser-diagnostic.ts";
import { failureCode, UiError, valueDigest, type Record_ } from "./ui-common.ts";
import { Captured, exitedChild, failureOf, fakeScope, must } from "./ui-fakes.ts";
import { browser, fixture, start, stop } from "./ui-native.ts";
import { linuxKernel, UiProcesses, withProcesses } from "./ui-processes.ts";
import type { Manifest } from "./ui-record.ts";

let directory: string;
const saved = { ...process.env };
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-native-"));
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
  for (const key of Object.keys(process.env))
    if (!(key in saved)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, saved);
});

const fixtureManifest = {
  binaries: { "fvoci-e2e-fixture": { path: "/never/executed" } },
} as unknown as Manifest;
const baselinePacket = (): Record_ => ({
  originalFailure: "TURSO_UI_BASELINE_FAILED",
  nativeOutcome: {
    operation: "failed",
    rollback: "unknown",
    commit: "not-attempted",
    baselineFailure: {
      phase: "table-contract",
      category: "protocol",
      tableComparison: {
        expectedCount: 99,
        actualCount: 98,
        setEqual: false,
        orderEqual: false,
        actualOnlyCount: 0,
        expectedOnlyCount: 1,
        actualOnlyUnderscoreCount: 0,
        firstMismatchIndex: 0,
        actualMismatchExpectedIndex: 1,
      },
    },
    baselineRollbackFailure: { phase: "rollback", category: "request" },
  },
  lifecycleDrain: "unconfirmed",
  drainOutcome: "failed",
  rows: ["PRIVATE_ROW_CANARY"],
  token: "PRIVATE_AUTH_CANARY",
});

describe("native fixture", () => {
  test("closed flat baseline and serial envelopes bind the same complete logical dataset", async () => {
    const dataset = {
      schema: 1,
      backend: "libsql-remote",
      schemaCurrent: true,
      schemaSha256: "a".repeat(64),
      lineage: "fvoci-sqlite-060",
      ledger: [
        [
          ["integer", "12"],
          ["text", "fvoci-sqlite-060"],
        ],
      ],
      rows: 26,
      setupNeeded: true,
      startupHazards: 0,
      liveOutboxLeases: 0,
      fingerprints: { event_sequence: { ["b".repeat(64)]: 1 } },
      allocations: { event_sequence: { ["b".repeat(64)]: {} } },
      operations: {
        event_sequence: [
          [
            ["integer", "1"],
            ["integer", "0"],
          ],
        ],
      },
      instanceConfig: [],
      futureDatasetField: { exact: ["integer", "9007199254740993"] },
    };
    const lifecycle = {
      nativeOutcome: { operation: "confirmed", rollback: "confirmed", commit: "not-attempted" },
      lifecycleDrain: "confirmed",
      leases: 0,
      drainOutcome: "confirmed",
      serverCloseReceipt: "not-exposed-by-sdk",
    };
    const read = async (mode: string, data: typeof dataset) => {
      const packet =
        mode === "baseline"
          ? { ...data, ...lifecycle }
          : { dataset: data, checks: { clockMicros: "123456789" }, ...lifecycle };
      const { scope, calls } = fakeScope({
        spawn: (args) => {
          expect(args).toEqual(["/never/executed", mode]);
          return exitedChild(0, JSON.stringify(packet));
        },
      });
      const value = await fixture(scope, fixtureManifest, mode, {});
      expect(calls.finish).toHaveLength(1);
      return value;
    };
    const baseline = await read("baseline", dataset);
    const serial = await read("serial-observe", dataset);
    expect(baseline).toEqual(dataset);
    expect(serial).toEqual({ dataset, checks: { clockMicros: "123456789" }, ...lifecycle });
    expect(valueDigest(baseline)).toBe(valueDigest(serial.dataset));
    expect(valueDigest({ ...dataset, ...lifecycle })).not.toBe(valueDigest(serial.dataset));
    const changed = await read("baseline", {
      ...dataset,
      operations: {
        event_sequence: [
          [
            ["integer", "1"],
            ["integer", "1"],
          ],
        ],
      },
    });
    expect(valueDigest(changed)).not.toBe(valueDigest(serial.dataset));
  });

  test("baseline normalization cannot bypass native drain or process closure refusal", async () => {
    for (const fault of ["drain", "lease", "sdk-close", "process-close"]) {
      const packet = {
        schema: 1,
        rows: 26,
        nativeOutcome: { operation: "confirmed", rollback: "confirmed", commit: "not-attempted" },
        lifecycleDrain: fault === "drain" ? "unconfirmed" : "confirmed",
        leases: fault === "lease" ? 1 : 0,
        drainOutcome: "confirmed",
        serverCloseReceipt: fault === "sdk-close" ? "fabricated" : "not-exposed-by-sdk",
      };
      const { scope, calls } = fakeScope({
        spawn: () => exitedChild(0, JSON.stringify(packet)),
        finish: () => {
          if (fault === "process-close") throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 0;
        },
      });
      expect(await failureOf(fixture(scope, fixtureManifest, "baseline", {}))).toContain(
        fault === "process-close" ? "UI_PROCESS_CLOSURE_FAILED" : "UI_NATIVE_DRAIN_FAILED",
      );
      expect(calls.finish).toHaveLength(1);
    }
  });

  test("baseline nonzero publishes before the packet write and process cleanup", async () => {
    for (const fault of ["none", "write", "finish", "both", "diagnostic-write"]) {
      const events: string[] = [];
      const output = new Captured();
      let first = true;
      const { scope, calls } = fakeScope({
        spawn: () => exitedChild(7, JSON.stringify(baselinePacket())),
        root: () => directory,
        output: {
          out: (line) => {
            output.out(line);
          },
          err: (line) => {
            events.push("diagnostic");
            if (fault === "diagnostic-write" && first) {
              first = false;
              throw new Error("PRIVATE_STDERR_WRITE_CANARY");
            }
            output.err(line);
          },
        },
        write: (path, value) => {
          events.push("write");
          if (fault === "write" || fault === "both") throw new Error("PRIVATE_WRITE_CANARY");
          write(path, value);
        },
        finish: () => {
          events.push("finish");
          if (fault === "finish" || fault === "both")
            throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 7;
        },
      });
      let caught: unknown;
      try {
        await fixture(scope, fixtureManifest, "baseline", {});
      } catch (error) {
        caught = error;
      }
      expect((caught as Error).message).toBe("TURSO_UI_BASELINE_FAILED");
      expect(events.slice(0, 3)).toEqual(["diagnostic", "write", "finish"]);
      expect(calls.finish).toHaveLength(1);
      expect(output.all).not.toContain("PRIVATE_");
      if (fault !== "diagnostic-write") {
        const published = JSON.parse(output.stderr[0] as string) as Record<string, Record_>;
        expect(published.baselineFailure).toEqual(
          (baselinePacket().nativeOutcome as Record<string, unknown>).baselineFailure as Record_,
        );
        expect(must(published.nativeOutcome).rollback).toBe("unknown");
      } else expect(output.all).toContain("UI_NATIVE_BASELINE_DIAGNOSTIC_WRITE_FAILED");
      for (const name of readdirSync(directory)) rmSync(join(directory, name));
    }
  });

  test("native nonzero precedes finish and packet write failures", async () => {
    const packet = {
      originalFailure: "TURSO_UI_ACTOR_FAILED",
      nativeOutcome: { operation: "failed", rollback: "unknown", commit: "not-attempted" },
      lifecycleDrain: "unconfirmed",
      leases: 1,
    };
    for (const fault of ["none", "finish", "write", "both", "malformed", "invalid-code"]) {
      const run = mkdtempSync(join(directory, "run-"));
      const payload =
        fault === "invalid-code" ? { ...packet, originalFailure: "PRIVATE_SDK_DETAIL" } : packet;
      const output = new Captured();
      const { scope, calls } = fakeScope({
        spawn: () => exitedChild(7, fault === "malformed" ? "not-json" : JSON.stringify(payload)),
        root: () => run,
        output,
        finish: () => {
          if (fault === "finish" || fault === "both")
            throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 7;
        },
        write: (path, value) => {
          if (fault === "write" || fault === "both") throw new Error("PRIVATE_WRITE_DETAIL");
          write(path, value);
        },
      });
      let actual = "accepted";
      try {
        await fixture(scope, fixtureManifest, "owner", {});
      } catch (error) {
        actual = failureCode(error);
      }
      expect(actual).toBe(
        fault === "malformed"
          ? "UI_CONSUMER_FAILED"
          : fault === "invalid-code"
            ? "UI_NATIVE_FAILURE_CODE_REFUSED"
            : "TURSO_UI_ACTOR_FAILED",
      );
      expect(calls.finish).toHaveLength(1);
      const receipts = readdirSync(run).filter((n) =>
        /^fixture-failure-.*\.private\.json$/.test(n),
      );
      expect(receipts).toHaveLength(fault === "none" || fault === "finish" ? 1 : 0);
      if (receipts.length)
        expect(
          (JSON.parse(readFileSync(join(run, must(receipts[0])), "utf8")) as Record_).receipt,
        ).toEqual(packet);
      if (["finish", "write", "both"].includes(fault)) {
        const diagnostic = JSON.parse(output.stderr[0] as string) as Record<string, unknown>;
        expect(diagnostic.originalFailure).toBe("TURSO_UI_ACTOR_FAILED");
        expect(
          (diagnostic.nativeCleanupErrors as string[]).includes(
            "UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED",
          ),
        ).toBe(fault === "write" || fault === "both");
      }
      for (const secret of ["PRIVATE_STDERR", "PRIVATE_WRITE_DETAIL", "PRIVATE_SDK_DETAIL"])
        expect(output.all).not.toContain(secret);
    }
  });

  test("the child receives only the allowlisted environment and the canonical input", async () => {
    for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
    process.env.PATH = "/pure/path";
    process.env.GITHUB_TOKEN = "PRIVATE_GITHUB_TOKEN";
    let seen: Record<string, string> = {};
    let fed = "";
    const ok = {
      lifecycleDrain: "confirmed",
      leases: 0,
      serverCloseReceipt: "not-exposed-by-sdk",
      rows: {},
    };
    const { scope } = fakeScope({
      spawn: (args, _label, options) => {
        expect(args).toEqual(["/never/executed", "observe"]);
        seen = options.env;
        const child = exitedChild(0, JSON.stringify(ok));
        (child as unknown as { stdin: unknown }).stdin = {
          write: (text: string) => {
            fed += text;
          },
          end: () => {},
        };
        return child;
      },
    });
    const value = await fixture(
      scope,
      fixtureManifest,
      "observe",
      {
        FVOCI_LIBSQL_URL: "u",
        FVOCI_LIBSQL_AUTH_TOKEN: "t",
        FVOCI_BIND: "x",
        ENCRYPTION_KEYS: "PRIVATE_KEYRING",
        ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
        SSL_CERT_FILE: "/owned/test-ca.pem",
      },
      { workspaceId: "w", documentIds: ["d"] },
    );
    expect(value).toEqual(ok);
    expect(fed).toBe('{"documentIds":["d"],"workspaceId":"w"}');
    expect(Object.keys(seen).sort()).toEqual(
      [
        "E2E_DATABASE_BACKEND",
        "FVOCI_E2E_TURSO_UI_SELECTED",
        "FVOCI_LIBSQL_AUTH_TOKEN",
        "FVOCI_LIBSQL_URL",
        "PATH",
        "SSL_CERT_FILE",
      ].sort(),
    );
    expect(seen.SSL_CERT_FILE).toBe("/owned/test-ca.pem");
  });

  test("a successful native helper with an unconfirmed drain is refused", async () => {
    const { scope } = fakeScope({
      spawn: () =>
        exitedChild(
          0,
          JSON.stringify({
            lifecycleDrain: "unconfirmed",
            leases: 0,
            serverCloseReceipt: "not-exposed-by-sdk",
          }),
        ),
    });
    expect(await failureOf(fixture(scope, fixtureManifest, "owner", {}))).toContain(
      "UI_NATIVE_DRAIN_FAILED",
    );
    expect(await failureOf(fixture(null, fixtureManifest, "owner", {}))).toContain(
      "UI_OWNED_PROCESS_SCOPE_REQUIRED",
    );
  });
});

interface BrowserInputs {
  workspace: string;
  cli: string;
  pkg: string;
  physical: string;
  manifest: Pick<Manifest, "bun" | "physicalInputs">;
  run: string;
}
function browserInputs(): BrowserInputs {
  const workspace = mkdtempSync(join(directory, "fvoci-turso-cli-pure-"));
  const cli = join(workspace, "node_modules/playwright/cli.js");
  mkdirSync(join(workspace, "node_modules/playwright"), { recursive: true });
  writeFileSync(cli, "/* pure fixture; never executed */");
  const pkg = join(workspace, "node_modules/playwright/package.json");
  writeFileSync(pkg, JSON.stringify({ version: "1.64.0", bin: { playwright: "cli.js" } }));
  const physical = join(workspace, "physical.private.json");
  write(physical, { files: { external: { [cli]: sha(cli), [pkg]: sha(pkg) } } });
  const manifest = {
    bun: { path: "/never-executed/admitted-bun", sha256: "", version: "" },
    physicalInputs: { path: physical, sha256: sha(physical) },
  };
  const run = join(workspace, "run");
  mkdirSync(run, 0o700);
  writeFileSync(join(run, "playwright.private.json"), '{"fixture":true}');
  for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, {
    PATH: "/pure/path",
    FVOCI_LIBSQL_URL: "PRIVATE_ENDPOINT",
    FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN",
    GITHUB_TOKEN: "PRIVATE_GITHUB_TOKEN",
    DATABASE_URL: "PRIVATE_DATABASE",
    PASSWORD_PEPPER_KEYS: "PRIVATE_PEPPER",
    ENCRYPTION_KEYS: "PRIVATE_KEYRING",
  });
  return { workspace, cli, pkg, physical, manifest, run };
}
const rewrite = (inputs: BrowserInputs, change: (external: Record<string, string>) => void) => {
  const value = JSON.parse(readFileSync(inputs.physical, "utf8")) as {
    files: { external: Record<string, string> };
  };
  change(value.files.external);
  writeFileSync(inputs.physical, JSON.stringify(value));
  inputs.manifest.physicalInputs.sha256 = sha(inputs.physical);
};

describe("browser", () => {
  const diagnosticSources = (source: string): BrowserDiagnosticSources => ({
    [ON]: { path: "/admitted/apps/web/e2e-pending/" + ON, source },
  });

  test("canonical reporter error locations survive without a stack and remain source qualified", () => {
    const source = 'test("registered", async () => {\n  expect(response.status()).toBe(200);\n});';
    const diagnostic = (location: unknown, errorLocation?: unknown) =>
      browserFailureDiagnostic(
        {
          suites: [
            {
              specs: [
                {
                  title: "registered",
                  tests: [
                    {
                      results: [
                        {
                          status: "failed",
                          error: {
                            message:
                              "Error: expect(received).toBe(expected)\nReceived: 503\nPRIVATE_BODY",
                            location,
                            snippet: "PRIVATE_SNIPPET",
                            cause: { message: "PRIVATE_CAUSE" },
                          },
                          errorLocation,
                          stdout: [{ text: "PRIVATE_STDOUT" }],
                          attachments: [{ body: "PRIVATE_ATTACHMENT" }],
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        },
        ON,
        "on",
        diagnosticSources(source),
      );
    const location = { file: ON, line: 2, column: 3 };
    for (const errorLocation of [undefined, { file: ON, line: 1 }]) {
      expect(diagnostic(location, errorLocation).browserFailure).toMatchObject({
        failures: [{ title: "registered", step: "http-status", httpStatus: 503, line: 2 }],
      });
    }
    for (const refused of [
      null,
      undefined,
      [],
      "PRIVATE_LOCATION",
      { file: "PRIVATE_HELPER.ts", line: 2 },
      { file: "PRIVATE_URL", line: 2 },
      ...[0, -1, 1.5, 4, Number.NaN, Number.POSITIVE_INFINITY, "2"].map((line) => ({
        file: ON,
        line,
      })),
    ]) {
      const result = diagnostic(refused, location);
      expect(result.browserFailure).toMatchObject({
        failures: [{ step: "expect", httpStatus: null, line: null }],
      });
      expect(JSON.stringify(result)).not.toContain("PRIVATE_");
    }
    expect(JSON.stringify(diagnostic(location))).not.toContain("PRIVATE_");
  });

  test("helper locations use only fixed admitted sources and their own bounds", () => {
    const sources = {
      ...diagnosticSources('test("registered", async () => {});'),
      "collab-helpers.ts": {
        path: "/admitted/apps/web/e2e-pending/collab-helpers.ts",
        source:
          'test("PRIVATE_HELPER_TITLE", async () => {});\nexpect(body.secret).toBe(200);\nexpect(response.status()).toBe(200);',
      },
      "selected-final-native.ts": {
        path: "/admitted/apps/web/e2e-pending/selected-final-native.ts",
        source: "// fixed native helper\nexpect(ack.version).toBe(200);",
      },
      "PRIVATE_FOREIGN.ts": {
        path: "/PRIVATE_FOREIGN.ts",
        source: "expect(response.status()).toBe(200);",
      },
    };
    const diagnostic = (error: Record_, errorLocation?: unknown, title = "registered") =>
      browserFailureDiagnostic(
        {
          suites: [
            {
              specs: [
                {
                  title,
                  tests: [
                    {
                      results: [
                        {
                          status: "failed",
                          error: {
                            message:
                              "Error: expect(received).toBe(expected)\nReceived: 503\nPRIVATE_BODY",
                            stack:
                              "PRIVATE_STACK at /admitted/apps/web/e2e-pending/collab-helpers.ts:3:1",
                            ...error,
                          },
                          errorLocation,
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        },
        ON,
        "on",
        sources,
      );
    const first = (value: Record_) => (value.browserFailure as Record_).failures as Record_[];
    for (const name of ["collab-helpers.ts", "selected-final-native.ts"] as const)
      for (const file of [name, "apps/web/e2e-pending/" + name, sources[name].path])
        expect(first(diagnostic({ location: { file, line: 2 } }))[0]).toMatchObject({
          title: "registered",
          file: "apps/web/e2e-pending/" + name,
          line: 2,
          httpStatus: null,
        });
    const fallback = { file: sources["collab-helpers.ts"].path, line: 3 };
    expect(first(diagnostic({}, fallback))[0]).toMatchObject({
      file: "apps/web/e2e-pending/collab-helpers.ts",
      line: 3,
      httpStatus: 503,
    });
    for (const location of [
      null,
      undefined,
      [],
      "PRIVATE_LOCATION",
      { file: "/foreign/e2e-pending/collab-helpers.ts", line: 3 },
      { file: "/PRIVATE_FOREIGN.ts", line: 1 },
      ...[0, -1, 1.5, 4, "3", Number.NaN, Number.POSITIVE_INFINITY].map((line) => ({
        file: fallback.file,
        line,
      })),
    ]) {
      const result = diagnostic({ location }, fallback);
      expect(first(result)[0]).toMatchObject({ file: null, line: null, httpStatus: null });
      expect(JSON.stringify(result)).not.toContain("PRIVATE_");
    }
    expect(
      first(diagnostic({ location: fallback }, undefined, "PRIVATE_HELPER_TITLE"))[0]?.title,
    ).toBe("unregistered");
  });

  test("diagnostics bound hostile reports and never treat body numbers as HTTP status", () => {
    const source = 'test("registered", async () => {\n  expect(body.secret).toBe(200);\n});';
    const result = {
      status: "failed",
      error: { message: "Error: expect(received).toBe(expected)\nReceived: 401\nPRIVATE_BODY" },
      errorLocation: { file: ON, line: 2 },
    };
    const report = {
      suites: [{ specs: [{ title: "registered", tests: [{ results: Array(100).fill(result) }] }] }],
    };
    const diagnostic = browserFailureDiagnostic(report, ON, "restart", diagnosticSources(source));
    expect(diagnostic.browserFailure).toMatchObject({
      stage: "restart",
      failures: Array(16).fill({
        title: "registered",
        category: "assertion",
        step: "expect",
        httpStatus: null,
        line: 2,
      }),
    });
    expect(JSON.stringify(diagnostic)).not.toContain("PRIVATE_");
    expect(
      browserFailureDiagnostic(null, ON, "off", diagnosticSources(source)).browserFailure,
    ).toEqual({
      stage: "off",
      report: "unavailable",
      failures: [],
      globalErrors: 0,
      globalErrorCategories: [],
    });
    const noTests = browserFailureDiagnostic(
      {
        suites: [],
        errors: [{ message: "Error: No tests found. PRIVATE_COLLECTION_DETAIL" }],
      },
      ON,
      "on",
      diagnosticSources(source),
    );
    expect(noTests.browserFailure).toMatchObject({ globalErrorCategories: ["no-tests"] });
    expect(JSON.stringify(noTests)).not.toContain("PRIVATE_");
  });

  test("a failed browser publishes only registered titles, fixed classes, stages and HTTP status", async () => {
    const inputs = browserInputs();
    const title = "selected normal main: synthetic browser failure";
    const source = `test("${title}", async () => {\n  expect(response.status()).toBe(200);\n});\n`;
    mkdirSync(join(inputs.workspace, "apps/web/e2e-pending"), { recursive: true });
    writeFileSync(join(inputs.workspace, "apps/web/e2e-pending", ON), source);
    const physical = JSON.parse(readFileSync(inputs.physical, "utf8")) as {
      files: { tracked?: Record<string, string> };
    };
    physical.files.tracked = {
      ["apps/web/e2e-pending/" + ON]: sha(join(inputs.workspace, "apps/web/e2e-pending", ON)),
    };
    writeFileSync(inputs.physical, JSON.stringify(physical));
    inputs.manifest.physicalInputs.sha256 = sha(inputs.physical);
    writeFileSync(
      join(inputs.run, "playwright.private.json"),
      JSON.stringify({
        suites: [
          {
            specs: [
              {
                title,
                tests: [
                  {
                    results: [
                      {
                        status: "failed",
                        error: {
                          message:
                            "Error: expect(received).toBe(expected)\nExpected: 200\nReceived: 503\nPRIVATE_URL_TOKEN_COOKIE_BODY_KEYRING",
                        },
                        errorLocation: {
                          file: join(inputs.workspace, "apps/web/e2e-pending", ON),
                          line: 2,
                          column: 3,
                        },
                        stdout: [{ text: "PRIVATE_STDOUT" }],
                        attachments: [{ body: "PRIVATE_BODY" }],
                      },
                    ],
                  },
                ],
              },
              {
                title: "PRIVATE_TEST_TITLE",
                tests: [
                  { results: [{ status: "timedOut", error: { message: "PRIVATE_TIMEOUT" } }] },
                ],
              },
            ],
          },
        ],
        errors: [{ message: "PRIVATE_GLOBAL_ERROR" }],
      }),
    );
    const output = new Captured();
    const { scope, calls } = fakeScope({ spawn: () => exitedChild(1), output });
    expect(
      await failureOf(
        browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
      ),
    ).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(calls.finish).toHaveLength(1);
    const diagnostic = JSON.parse(output.stderr[0] as string) as Record_;
    expect(diagnostic).toMatchObject({
      originalFailure: "UI_ACTUAL_BROWSER_FAILED",
      browserFailure: {
        stage: "on",
        report: "available",
        failures: [
          { title, category: "assertion", step: "http-status", httpStatus: 503, line: 2 },
          {
            title: "unregistered",
            category: "timeout",
            step: "test-body",
            httpStatus: null,
            line: null,
          },
        ],
        globalErrors: 1,
      },
    });
    expect(output.all).not.toContain("PRIVATE_");
  });

  test("failed browser helper diagnostics require unchanged source-admitted files and retain closure", async () => {
    for (const changed of [false, true]) {
      const inputs = browserInputs();
      const title = "selected normal main: fixed helper failure";
      const relative = "apps/web/e2e-pending/";
      const spec = join(inputs.workspace, relative, ON);
      const helper = join(inputs.workspace, relative, "collab-helpers.ts");
      mkdirSync(join(inputs.workspace, relative), { recursive: true });
      writeFileSync(spec, `test("${title}", async () => {});`);
      writeFileSync(helper, "// admitted helper\nexpect(response.status()).toBe(200);");
      const physical = JSON.parse(readFileSync(inputs.physical, "utf8")) as {
        files: { tracked?: Record<string, string> };
      };
      physical.files.tracked = {
        [relative + ON]: sha(spec),
        [relative + "collab-helpers.ts"]: sha(helper),
      };
      writeFileSync(inputs.physical, JSON.stringify(physical));
      inputs.manifest.physicalInputs.sha256 = sha(inputs.physical);
      if (changed)
        writeFileSync(helper, "// changed private input\nexpect(response.status()).toBe(200);");
      writeFileSync(
        join(inputs.run, "playwright.private.json"),
        JSON.stringify({
          suites: [
            {
              specs: [
                {
                  title,
                  tests: [
                    {
                      results: [
                        {
                          status: "failed",
                          error: {
                            location: { file: helper, line: 2 },
                            message:
                              "Error: expect(received).toBe(expected)\nReceived: 503\nPRIVATE_BODY",
                            stack: "PRIVATE_STACK",
                          },
                          errorLocation: { file: spec, line: 1 },
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        }),
      );
      const output = new Captured();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(1), output });
      expect(
        await failureOf(
          browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
        ),
      ).toBe("UI_ACTUAL_BROWSER_FAILED");
      expect(calls.finish).toHaveLength(1);
      expect(JSON.parse(output.stderr[0] as string)).toMatchObject({
        originalFailure: "UI_ACTUAL_BROWSER_FAILED",
        browserFailure: {
          failures: [
            {
              title,
              file: changed ? null : relative + "collab-helpers.ts",
              line: changed ? null : 2,
              httpStatus: changed ? null : 503,
            },
          ],
        },
      });
      expect(output.all).not.toContain("PRIVATE_");
      expect(output.all).not.toContain(inputs.workspace);
    }
  });

  test("the direct CLI keeps arguments, environment, wait and ownership", async () => {
    for (const [spec, grep] of [
      [ON, "selected normal main:"],
      [OFF, undefined],
      [ON, "selected normal main restart:"],
    ] as const) {
      const inputs = browserInputs();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      const result = await browser(
        scope,
        inputs.manifest,
        inputs.run,
        { FVOCI_E2E_SELECTED_FLOW: "on" },
        spec,
        grep,
        inputs.workspace,
      );
      expect(result).toEqual({ fixture: true });
      const [args, label, options] = must(calls.spawn[0]);
      const expected = [
        "/never-executed/admitted-bun",
        "--no-env-file",
        "--config=" + join(inputs.workspace, "bunfig.toml"),
        "--no-install",
        inputs.cli,
        "test",
        "--config",
        "e2e-pending/collab-playwright.config.ts",
        "--reporter=line,json",
      ];
      if (grep) expected.push("--grep", grep);
      expect(args).toEqual([...expected, "e2e-pending/" + spec]);
      expect(label).toBe("browser");
      expect(options.cwd).toBe(join(inputs.workspace, "apps/web"));
      expect(options.stderr).toBe(options.stdout);
      expect(Object.keys(options.env).sort()).toEqual(
        [
          "CI",
          "FVOCI_E2E_RESULT_DIR",
          "FVOCI_E2E_SELECTED_FLOW",
          "PATH",
          "PLAYWRIGHT_JSON_OUTPUT_FILE",
        ].sort(),
      );
      expect(options.env.CI).toBe("true");
      expect(JSON.stringify(args) + JSON.stringify(options.env)).not.toContain("PRIVATE_");
      expect(calls.finish).toEqual([
        [calls.spawn.length ? must(scope.allocations[0]).process : (null as never), undefined],
      ]);
      expect(statSync(join(inputs.run, "browser.private.log")).mode & 0o777).toBe(0o600);
      expect(statSync(join(inputs.run, "playwright.private.json")).mode & 0o777).toBe(0o600);
    }
  });

  test("the dedicated auth spec runs only with its own config and keeps the actual flow", async () => {
    for (const flow of ["on", "off"] as const) {
      const inputs = browserInputs();
      const output = new Captured();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(1), output });
      expect(
        await failureOf(
          browser(
            scope,
            inputs.manifest,
            inputs.run,
            { FVOCI_E2E_SELECTED_FLOW: flow },
            AUTH_SPEC,
            undefined,
            inputs.workspace,
            "auth-session-playwright.config.ts",
          ),
        ),
      ).toBe("UI_ACTUAL_BROWSER_FAILED");
      const [args] = must(calls.spawn[0]);
      expect(args.slice(-4)).toEqual([
        "--config",
        "e2e-pending/auth-session-playwright.config.ts",
        "--reporter=line,json",
        "e2e-pending/" + AUTH_SPEC,
      ]);
      const diagnostic = JSON.parse(
        must(output.stderr.find((line) => line.includes("browserFailure"))),
      ) as { browserFailure: { stage: string } };
      expect(diagnostic.browserFailure.stage).toBe(flow);
    }
    for (const [spec, config, flow] of [
      [AUTH_SPEC, "collab-playwright.config.ts", "on"],
      [ON, "auth-session-playwright.config.ts", "on"],
      [OFF, "auth-session-playwright.config.ts", "off"],
      [AUTH_SPEC, "auth-session-playwright.config.ts", "restart"],
      [AUTH_SPEC, "auth-session-playwright.config.ts", undefined],
    ] as const) {
      const inputs = browserInputs();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      expect(
        await failureOf(
          browser(
            scope,
            inputs.manifest,
            inputs.run,
            flow === undefined ? {} : { FVOCI_E2E_SELECTED_FLOW: flow },
            spec,
            undefined,
            inputs.workspace,
            config,
          ),
        ),
      ).toBe("UI_BROWSER_CONFIG_REFUSED");
      expect(calls.spawn).toHaveLength(0);
    }
  });

  test("a missing, symlinked, nonregular, drifted or unadmitted CLI is refused before the child", async () => {
    for (const fault of [
      "missing",
      "symlink",
      "directory",
      "drift",
      "unadmitted",
      "package-drift",
      "package-version",
      "package-bin",
      "receipt-drift",
    ]) {
      const inputs = browserInputs();
      if (fault === "missing") unlinkSync(inputs.cli);
      else if (fault === "symlink") {
        const target = join(inputs.workspace, "node_modules/playwright/actual.js");
        renameSync(inputs.cli, target);
        symlinkSync(target, inputs.cli);
      } else if (fault === "directory") {
        unlinkSync(inputs.cli);
        mkdirSync(inputs.cli);
      } else if (fault === "drift") writeFileSync(inputs.cli, "changed fixture bytes");
      else if (fault === "package-drift") writeFileSync(inputs.pkg, "{}");
      else if (fault === "package-version" || fault === "package-bin") {
        writeFileSync(
          inputs.pkg,
          JSON.stringify({
            version: fault === "package-version" ? "1.62.0" : "1.64.0",
            bin: { playwright: fault === "package-bin" ? "other.js" : "cli.js" },
          }),
        );
        rewrite(inputs, (external) => (external[inputs.pkg] = sha(inputs.pkg)));
      } else if (fault === "unadmitted")
        rewrite(inputs, (external) => Reflect.deleteProperty(external, inputs.cli));
      else writeFileSync(inputs.physical, "{}");
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      const code =
        fault === "receipt-drift" ? "UI_PHYSICAL_RECEIPT_CHANGED" : "UI_PLAYWRIGHT_CLI_REFUSED";
      expect(
        await failureOf(
          browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
        ),
      ).toBe(code);
      expect(calls.spawn).toHaveLength(0);
      expect(existsSync(join(inputs.run, "browser.private.log"))).toBe(false);
    }
  });

  test("an explicit secret environment is refused before the child", async () => {
    for (const key of [
      "FVOCI_LIBSQL_URL",
      "FVOCI_LIBSQL_AUTH_TOKEN",
      "DATABASE_URL",
      "DATABASE_APP_URL",
      "PASSWORD_PEPPER_KEYS",
      "ENCRYPTION_KEYS",
    ]) {
      const inputs = browserInputs();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      expect(
        await failureOf(
          browser(
            scope,
            inputs.manifest,
            inputs.run,
            { [key]: "PRIVATE_VALUE" },
            ON,
            undefined,
            inputs.workspace,
          ),
        ),
      ).toBe("UI_BROWSER_SECRET_ENV_REFUSED");
      expect(calls.spawn).toHaveLength(0);
      expect(existsSync(join(inputs.run, "browser.private.log"))).toBe(false);
    }
  });

  test("a child failure keeps the original failure and finishes the owned process", async () => {
    const inputs = browserInputs();
    const output = new Captured();
    const { scope, calls } = fakeScope({ spawn: () => exitedChild(1), output });
    expect(
      await failureOf(
        browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
      ),
    ).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(calls.finish).toHaveLength(1);
    const failing = fakeScope({
      spawn: () => exitedChild(1),
      output,
      finish: () => {
        throw new UiError("UI_PROCESS_CLOSURE_FAILED");
      },
    });
    const again = browserInputs();
    expect(
      await failureOf(
        browser(failing.scope, again.manifest, again.run, {}, ON, undefined, again.workspace),
      ),
    ).toBe("UI_ACTUAL_BROWSER_FAILED");
    expect(
      JSON.parse(output.stderr.find((line) => line.includes("browserProcessClosure")) as string),
    ).toEqual({
      originalFailure: "UI_ACTUAL_BROWSER_FAILED",
      browserProcessClosure: "UI_PROCESS_CLOSURE_FAILED",
    });
  });
});

describe("server stop", () => {
  test("the canonical restart projection keeps detailed identities private", async () => {
    const child = exitedChild(0, "", 10);
    const { scope } = fakeScope({ spawn: () => child });
    scope.spawn(["server"], "server", { env: {} });
    const row = { pid: 10, parentPid: 1, startTicks: "100", state: "S" };
    scope.capture(row, "server", 0);
    const result = await stop(scope, { child }, "http://127.0.0.1:12345", directory, () =>
      Promise.resolve(true),
    );
    expect(result).toEqual({ serverExit: 0, portClosed: true, recordedIdentitiesRetired: true });
    const packet = must(readdirSync(directory).find((n) => n.startsWith("server-identities-")));
    expect(
      (JSON.parse(readFileSync(join(directory, packet), "utf8")) as Record_).identities,
    ).toEqual([row]);
    expect(result).not.toHaveProperty("identities");
    const open = fakeScope({ spawn: () => child });
    open.scope.spawn(["server"], "server", { env: {} });
    open.scope.capture(row, "server", 0);
    expect(
      await failureOf(
        stop(open.scope, { child }, "http://127.0.0.1:12345", directory, () =>
          Promise.resolve(false),
        ),
      ),
    ).toContain("UI_SERVER_CLOSURE_FAILED");
  });
});

// A real Bun child under a real process scope: the env checked before the
// spawn is the env the Playwright CLI sees. Only key presence is recorded.
describe.skipIf(process.platform !== "linux")("browser child environment", () => {
  test("no .env, bunfig or Bun test worker id reaches the Playwright child", async () => {
    const inputs = browserInputs();
    process.env.PATH = saved.PATH ?? "";
    writeFileSync(
      inputs.cli,
      "const keys = ['FVOCI_LIBSQL_AUTH_TOKEN', 'FVOCI_LIBSQL_URL', 'JEST_WORKER_ID', 'FROM_BUNFIG_PRELOAD'];\n" +
        "require('node:fs').writeFileSync(process.env.PLAYWRIGHT_JSON_OUTPUT_FILE, JSON.stringify(Object.fromEntries(keys.map((k) => [k, k in process.env]))));\n",
    );
    rewrite(inputs, (external) => (external[inputs.cli] = sha(inputs.cli)));
    mkdirSync(join(inputs.workspace, "apps/web"), { recursive: true });
    writeFileSync(
      join(inputs.workspace, "apps/web/.env"),
      "FVOCI_LIBSQL_AUTH_TOKEN=synthetic\nFVOCI_LIBSQL_URL=libsql://synthetic.invalid\nJEST_WORKER_ID=7\n",
    );
    // The isolated config is the workspace bunfig; a bunfig in apps/web is not read.
    writeFileSync(join(inputs.workspace, "bunfig.toml"), "");
    writeFileSync(
      join(inputs.workspace, "apps/web/preload.ts"),
      "process.env.FROM_BUNFIG_PRELOAD = '1';\n",
    );
    writeFileSync(join(inputs.workspace, "apps/web/bunfig.toml"), 'preload = ["./preload.ts"]\n');
    const manifest = {
      ...inputs.manifest,
      bun: { path: process.execPath, sha256: "", version: "" },
    };
    const output = new Captured();
    const seen = await withProcesses(
      (scope) =>
        browser(
          scope,
          manifest,
          inputs.run,
          { JEST_WORKER_ID: "3" },
          ON,
          undefined,
          inputs.workspace,
        ),
      new UiProcesses(linuxKernel, { output, write, root: () => inputs.run }),
    );
    expect(seen).toEqual({
      FVOCI_LIBSQL_AUTH_TOKEN: false,
      FVOCI_LIBSQL_URL: false,
      JEST_WORKER_ID: false,
      FROM_BUNFIG_PRELOAD: false,
    });
  });
});

test.each(["spawn", "clock"] as const)(
  "FD ownership covers native startup %s failure",
  async (fault) => {
    let fd: number | undefined;
    const original = new UiError("UI_START_INITIALIZATION_FAULT");
    const { scope, calls } = fakeScope({
      spawn: (_args, _label, options) => {
        fd = options.stdout as number;
        if (fault === "spawn") throw original;
        return exitedChild(0);
      },
      finish: () => {
        throw new UiError("UI_CLEANUP_FAULT");
      },
    });
    try {
      const caught = await start(
        scope,
        { binaries: { "fvoci-migrate": { path: "/never-executed" } } } as unknown as Manifest,
        {},
        directory,
        false,
        () => {
          throw original;
        },
      ).then(
        () => null,
        (error: unknown) => error,
      );
      expect(caught).toBe(original);
      expect(() => fstatSync(must(fd))).toThrow(expect.objectContaining({ code: "EBADF" }));
      expect(calls.finish).toHaveLength(fault === "clock" ? 1 : 0);
    } finally {
      if (fd !== undefined) {
        try {
          closeSync(fd);
        } catch {
          /* already closed */
        }
      }
    }
  },
);
