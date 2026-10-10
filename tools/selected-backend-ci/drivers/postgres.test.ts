import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { digest, sourceInputText } from "../io.ts";
import type { Inputs } from "../types.ts";
import {
  failureCheckpoint,
  browserPacketKeys,
  knownOnBrowserTest,
  type Command,
} from "./common.ts";
import {
  browserArgs,
  canonicalUuid,
  finalize,
  parent,
  postgresDriverCommand,
  qualifyPlaywright,
  recordBrowserFailure,
  wrapperEnvironment,
  type InsideState,
  type Seam,
} from "./postgres.ts";

const SOURCE = "a".repeat(40),
  TREE = "b".repeat(40),
  OWNER = "pure-owned-control";
const FIRST = "FIRST_PRIVATE_ORIGINAL",
  SECOND = "SECOND_PRIVATE_CLEANUP";
const hash = (data: string | Buffer) => createHash("sha256").update(data).digest("hex");
let root: string;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "fvoci-postgres-driver-"));
});
afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});
const fail = (label = SECOND) => {
  throw Object.assign(new Error(label), { name: "OSError" });
};

describe("Playwright direct CLI qualification", () => {
  const setup = () => {
    const cli = join(root, "node_modules/playwright/cli.js"),
      manifest = join(root, "node_modules/playwright/package.json"),
      bun = join(root, "bun");
    mkdirSync(join(root, "node_modules/playwright"), { recursive: true });
    writeFileSync(cli, "synthetic official CLI");
    writeFileSync(manifest, JSON.stringify({ version: "1.63.0", bin: { playwright: "cli.js" } }));
    writeFileSync(bun, "synthetic qualified Bun");
    const before = { external: { [cli]: hash(readFileSync(cli)) } } as unknown as Inputs;
    return { cli, manifest, bun, before };
  };
  test("the pinned bin regular file in the admitted closure qualifies", () => {
    const { cli, bun, before } = setup();
    expect(qualifyPlaywright(root, before, bun)).toBe(cli);
  });
  test("hash, missing, bin, version, symlink and Bun faults refuse", () => {
    for (const fault of ["hash", "missing", "bin", "version", "symlink", "bun"]) {
      rmSync(join(root, "node_modules"), { recursive: true, force: true });
      const { cli, manifest, bun, before } = setup();
      if (fault === "hash") before.external[cli] = "0".repeat(64);
      if (fault === "missing") before.external = {};
      if (fault === "bin")
        writeFileSync(manifest, JSON.stringify({ version: "1.63.0", bin: { playwright: "o.js" } }));
      if (fault === "version")
        writeFileSync(
          manifest,
          JSON.stringify({ version: "0.0.0", bin: { playwright: "cli.js" } }),
        );
      if (fault === "symlink") {
        renameSync(cli, join(root, "foreign-cli"));
        symlinkSync(join(root, "foreign-cli"), cli);
      }
      if (fault === "bun") unlinkSync(bun);
      expect([
        fault,
        (() => {
          try {
            qualifyPlaywright(root, before, bun);
            return "accepted";
          } catch {
            return "refused";
          }
        })(),
      ]).toEqual([fault, "refused"]);
    }
  });
  test("both selected flows use the same direct CLI argv and no install", () => {
    for (const spec of [
      "workspace-wiki-selected-backend.spec.ts",
      "workspace-off-selected-backend.spec.ts",
    ])
      expect(
        browserArgs("/qualified/bun", "/qualified/node_modules/playwright/cli.js", spec),
      ).toEqual([
        "/qualified/bun",
        "--no-install",
        "/qualified/node_modules/playwright/cli.js",
        "test",
        "--config",
        "e2e-pending/collab-playwright.config.ts",
        "--reporter=line,json",
        spec,
      ]);
  });
});

describe("wrapper environment and identifiers", () => {
  test("the owned wrappers get only the allowlisted names", () => {
    const saved = { ...process.env };
    try {
      Object.assign(process.env, {
        TEST_DATABASE_URL: "postgres://private",
        PYTHONPATH: "/private",
        GITHUB_SHA: SOURCE,
        FVOCI_CI_BUN: "/qualified/bun",
        PLAYWRIGHT_BROWSERS_PATH: "",
      });
      const on = wrapperEnvironment("on", OWNER),
        off = wrapperEnvironment("off", OWNER);
      expect(on.FVOCI_E2E_SELECTED_AUXILIARY).toBe("normal-api");
      expect("FVOCI_E2E_SELECTED_AUXILIARY" in off).toBe(false);
      expect(on.FVOCI_ROOT_RUN_OWNER).toBe(OWNER);
      expect(on.FVOCI_TEST_PG_MAJOR).toBe("18");
      expect(on.GITHUB_SHA).toBe(SOURCE);
      for (const name of [
        "TEST_DATABASE_URL",
        "PYTHONPATH",
        "PYTHONDONTWRITEBYTECODE",
        "PLAYWRIGHT_BROWSERS_PATH",
        "HOME",
      ])
        expect([name, name in on]).toEqual([name, false]);
    } finally {
      for (const key of Object.keys(process.env))
        if (!(key in saved)) Reflect.deleteProperty(process.env, key);
      Object.assign(process.env, saved);
    }
  });
  test("re-entry keeps Bun without .env autoload", () => {
    expect(postgresDriverCommand()).toEqual([
      process.execPath,
      "--no-env-file",
      join(import.meta.dir, "postgres.ts"),
    ]);
  });
  test("only canonical lowercase UUID text reaches owned SQL", () => {
    const id = "0f1e2d3c-4b5a-4968-8776-655443322110";
    expect(canonicalUuid(id)).toBe(id);
    for (const value of [
      id.toUpperCase(),
      "{" + id + "}",
      id.replaceAll("-", ""),
      "x' OR '1'='1",
      7,
    ])
      expect(() => canonicalUuid(value)).toThrow();
  });
});

describe("nonzero browser exit", () => {
  test("records the first error before cleanup, privately and without secrets", () => {
    const secret = "SECRET_COOKIE=synthetic-not-a-cause";
    const specFile = "/opt/fvoci/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts";
    writeFileSync(join(root, "browser.log"), "1 failed\n" + secret + "\n");
    writeFileSync(
      join(root, "playwright-result.private.json"),
      JSON.stringify({
        config: { workers: 1 },
        suites: [
          {
            specs: [
              {
                title: knownOnBrowserTest,
                file: specFile,
                tests: [
                  {
                    results: [
                      {
                        status: "failed",
                        error: { message: secret, stack: secret },
                        errorLocation: { file: specFile, line: 42, column: 1 },
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
    const receipt: Record<string, unknown> = { phase: "browser" };
    recordBrowserFailure(receipt, root, 7);
    const packet = JSON.parse(
      readFileSync(join(root, "original-failure.private.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(packet.browser_report_state).toBe("matched");
    expect(packet.known_browser_status).toBe("failed");
    expect(packet.known_browser_checkpoint).toBe(
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:42",
    );
    expect(packet.failure_code).toBe("SELECTED_BODY_NONZERO");
    expect(packet.observed_failed_exit).toBe(7);
    expect(JSON.stringify(packet)).not.toContain("SECRET_COOKIE");
    for (const name of ["browser.log", "playwright-result.private.json"])
      expect(statSync(join(root, name)).mode & 0o777).toBe(0o600);
    expect(existsSync(join(root, "receipt.json"))).toBe(false);
  });
  test("a missing or unreadable report keeps its state", () => {
    writeFileSync(join(root, "browser.log"), "log");
    const missing: Record<string, unknown> = { phase: "browser" };
    recordBrowserFailure(missing, root, 1);
    expect(missing.browser_report_state).toBe("report-missing");
    rmSync(join(root, "original-failure.private.json"));
    writeFileSync(join(root, "playwright-result.private.json"), "{not json");
    const unreadable: Record<string, unknown> = { phase: "browser" };
    recordBrowserFailure(unreadable, root, 1);
    expect(unreadable.browser_report_state).toBe("report-unreadable");
    expect(unreadable.diagnostic_errors).toEqual(["browser-report-read-failed"]);
  });
});

type Fault =
  | "rows"
  | "remove"
  | "inspect"
  | "wait"
  | "log-close"
  | "log-hash"
  | "port"
  | "inputs"
  | "receipt"
  | null;
// The child's whole finalization with its process, Docker, socket and DB I/O
// replaced; the receipts, packet and logs are real files.
async function exercise(fault: Fault, ordinary = false) {
  const run = join(root, "run");
  mkdirSync(run);
  for (const name of ["browser.log", "normal-server.log"])
    writeFileSync(join(run, name), "private synthetic body log");
  const receipt: Record<string, unknown> = {
    source: SOURCE,
    tree: TREE,
    root_owner: OWNER,
    selected_flow: "on",
    phase: "browser",
    browser_exit: 7,
    owned_container_absent: null,
    owned_loopback_port_closed: null,
    recorded_process_identities_retired: null,
  };
  if (ordinary) recordBrowserFailure(receipt, run, 7);
  else
    failureCheckpoint(receipt, run, receipt.browser_exit, {
      error: Object.assign(new Error(FIRST), { name: "RuntimeError" }),
      extraKeys: browserPacketKeys,
    });
  const attempted: string[][] = [];
  const command: Command = (args) => {
    attempted.push(args);
    expect(existsSync(join(run, "original-failure.private.json"))).toBe(true);
    if (fault === "remove" && args[1] === "rm") fail();
    if (fault === "inspect" && args[1] === "inspect") fail();
    return Promise.resolve({
      returncode: args[1] === "inspect" ? 1 : 0,
      stdout: "",
      stderr: "No such container: owned",
    });
  };
  if (fault === "receipt") writeFileSync(join(run, "receipt.json"), "occupied");
  if (fault === "log-hash") unlinkSync(join(run, "normal-server.log"));
  // An fd number nothing holds: closing it fails like a broken log handle.
  const serverLog =
    fault === "log-close" ? 999_999 : openSync(join(run, "normal-server-fd.log"), "w");
  const seam: Seam = {
    command,
    ownedRows: () =>
      fault === "rows"
        ? Promise.reject(new Error(SECOND))
        : Promise.resolve([{ pid: 22 } as never]),
    identityGone: (row) => row.pid === 22,
    pgSql: () => Promise.resolve({}),
    spawnServer: () => fail(),
    probeSetup: () => fail(),
    portClosed: () =>
      fault === "port" ? Promise.reject(new Error(SECOND)) : Promise.resolve(true),
    postInputs: () =>
      fault === "inputs" ? Promise.reject(new Error(SECOND)) : Promise.resolve({} as Inputs),
    emit: (line) => emitted.push(line),
    actor: () => [1000, 1000],
  };
  const emitted: string[] = [];
  const state: InsideState = {
    receipt,
    run,
    storage: join(run, "storage"),
    name: "owned",
    code: 7,
    created: true,
    serverRow: { pid: 22, namespace_pid: 22 } as never,
    serverProcess: {
      get exited() {
        return fault === "wait" ? Promise.reject(new Error(SECOND)) : Promise.resolve(0);
      },
      exitCode: fault === "wait" ? null : 0,
      signalCode: null,
      kill: () => {
        if (fault === "wait") fail();
      },
    },
    serverLog,
    base: "http://127.0.0.1:12345",
    bun: join(run, "bun"),
    sourceBefore: {} as Inputs,
    browserInputs: null,
  };
  const code = await finalize(state, seam);
  expect(code).toBe(7);
  const packet = JSON.parse(
    readFileSync(join(run, "original-failure.private.json"), "utf8"),
  ) as Record<string, unknown> & { original_driver_failure?: Record<string, unknown> };
  expect(emitted.length).toBe(1);
  const summary = JSON.parse(emitted[0] as string) as Record<string, unknown>;
  expect(summary.final_exit_code).toBe(7);
  expect(summary.failed_phase).toBe("browser");
  expect(summary.original_driver_failure_sha256).toMatch(/^[0-9a-f]{64}$/);
  expect(packet.observed_failed_exit).toBe(7);
  expect(emitted[0]).not.toContain("PRIVATE");
  if (ordinary) {
    expect(packet.original_driver_failure?.type).toBe("ReturnedNonzero");
    expect(packet.failure_code).toBe("SELECTED_BODY_NONZERO");
    expect(packet.original_body_log_sha256).toBe(hash("private synthetic body log"));
  } else expect(packet.original_driver_failure).toEqual({ type: "RuntimeError", message: FIRST });
  if (fault) expect((summary.cleanup_failure_codes as unknown[]).length).toBeGreaterThan(0);
  else expect(summary.cleanup_failure_codes).toEqual([]);
  if (fault === "rows") expect(receipt.recorded_process_identities_retired).toBeNull();
  if (fault === "port") expect(receipt.owned_loopback_port_closed).toBeNull();
  if (fault === "inspect") expect(receipt.owned_container_absent).toBeNull();
  expect(attempted.some((args) => args[1] === "inspect")).toBe(true);
  if (fault !== "receipt") {
    const written = JSON.parse(readFileSync(join(run, "receipt.json"), "utf8")) as Record<
      string,
      unknown
    >;
    expect(written.final_exit_code).toBe(7);
  } else expect(summary.cleanup_failure_codes).toContain("final-receipt-write-failed");
  rmSync(run, { recursive: true });
  return { summary, receipt };
}

describe("child finalization", () => {
  test("the exception path survives each secondary cleanup fault", async () => {
    const { summary } = await exercise(null);
    expect(summary.cleanup_failure_codes).toEqual([]);
    for (const fault of [
      "rows",
      "remove",
      "inspect",
      "wait",
      "log-close",
      "log-hash",
      "port",
      "inputs",
      "receipt",
    ] as const)
      await exercise(fault);
  });
  test("an ordinary nonzero body keeps its phase, exit and original log digest", async () => {
    for (const fault of [null, "remove", "inputs"] as const) await exercise(fault, true);
  });
  test("an unobserved port and the initial phase record without a positive fact", async () => {
    const run = join(root, "run");
    mkdirSync(run);
    const receipt: Record<string, unknown> = {
      phase: "container-prepare",
      owned_loopback_port_closed: null,
      loopback_port_observation: "not-observed",
      recorded_process_identities_retired: null,
    };
    const private_ = { type: "AssertionError", message: "SYNTHETIC_PRIVATE_SECRET" };
    failureCheckpoint(receipt, run, null, {
      error: Object.assign(new Error(private_.message), { name: private_.type }),
    });
    const emitted: string[] = [];
    const code = await finalize(
      {
        receipt,
        run,
        storage: join(run, "storage"),
        name: "never-created",
        code: 7,
        created: false,
        serverRow: null,
        serverProcess: null,
        serverLog: null,
        base: null,
        bun: "",
        sourceBefore: {} as Inputs,
        browserInputs: null,
      },
      {
        command: () => fail(),
        ownedRows: () => fail(),
        identityGone: () => fail(),
        pgSql: () => Promise.resolve({}),
        spawnServer: () => fail(),
        probeSetup: () => fail(),
        portClosed: () => fail(),
        postInputs: () => Promise.resolve({} as Inputs),
        emit: (line) => emitted.push(line),
        actor: () => [1000, 1000],
      },
    );
    expect(code).toBe(7);
    expect(receipt.owned_loopback_port_closed).toBeNull();
    expect(receipt.failed_phase).toBe("container-prepare");
    expect(receipt.original_driver_failure).toEqual(private_);
    expect(receipt.cleanup_errors).toEqual([
      "owned loopback port never observed; retirement remains unqualified",
    ]);
    expect(emitted.join("")).not.toContain("SYNTHETIC_PRIVATE_SECRET");
  });
  test("an empty process observation cannot claim retired identities", async () => {
    for (const [rows, gone, expected] of [
      [[], true, false],
      [[{ pid: 1 }], false, false],
      [[{ pid: 1 }], true, true],
    ] as const) {
      const run = join(root, "run");
      mkdirSync(run);
      const receipt: Record<string, unknown> = { phase: "browser" };
      await finalize(
        {
          receipt,
          run,
          storage: join(run, "storage"),
          name: "owned",
          code: 0,
          created: true,
          serverRow: null,
          serverProcess: null,
          serverLog: null,
          base: null,
          bun: "",
          sourceBefore: {} as Inputs,
          browserInputs: null,
        },
        {
          command: (args) =>
            Promise.resolve({
              returncode: args[1] === "inspect" ? 1 : 0,
              stdout: "",
              stderr: "No such container",
            }),
          ownedRows: () => Promise.resolve(rows as never),
          identityGone: () => gone,
          pgSql: () => Promise.resolve({}),
          spawnServer: () => fail(),
          probeSetup: () => fail(),
          portClosed: () => fail(),
          postInputs: () => Promise.resolve({} as Inputs),
          emit: () => undefined,
          actor: () => [1000, 1000],
        },
      );
      expect(receipt.recorded_process_identities_retired).toBe(expected);
      rmSync(run, { recursive: true });
    }
  });
});

describe("parent finalization", () => {
  const sourceBefore = {
    head: SOURCE,
    tree: TREE,
    status: "",
    tracked: { "apps/web/é.ts": "1".repeat(64), "z.ts": "2".repeat(64) },
    external: { "/opt/x": "3".repeat(64) },
    untracked: {},
  };
  for (const fault of ["fixture", "post", "both", "write", "none"] as const)
    test(`wrapper exit and first packet survive ${fault}`, async () => {
      const run = join(root, "root-current-postgres-0123456789ab");
      const emitted: string[] = [];
      const writes: string[] = [];
      const code = await parent(
        run,
        { head: SOURCE, tree: TREE, owner: OWNER, flow: "on", sourceBefore },
        {
          command: (args, options = {}) => {
            expect(args.slice(0, 2)).toEqual([
              "bash",
              join(import.meta.dir, "../../../scripts/start-test-postgres.sh"),
            ]);
            expect(args.slice(2, 5)).toEqual(postgresDriverCommand());
            expect(args.slice(5)).toEqual(["--pg-ready", run]);
            expect(options.env?.FVOCI_ROOT_RUN_OWNER).toBe(OWNER);
            // The wrapper owns the PostgreSQL container; an interrupt never SIGKILLs it.
            expect(options.waitOnInterrupt).toBe(true);
            writeFileSync(options.log as string, "synthetic wrapper log");
            writeFileSync(join(run, "receipt.json"), JSON.stringify({ final_exit_code: 7 }));
            if (fault === "write") writeFileSync(join(run, "parent-receipt.json"), "occupied");
            return Promise.resolve({ returncode: 7, stdout: "", stderr: "" });
          },
          verifyFixturesClosed: () => {
            writes.push("fixtures");
            return fault === "fixture" || fault === "both"
              ? Promise.reject(new Error("SECOND_PARENT_PRIVATE"))
              : Promise.resolve(
                  Array.from({ length: 2 }, (_, i) => ({
                    kind: i ? "meili" : "pg",
                    name: "owned",
                    containerAbsent: true,
                    recordedPIDIdentitiesRetired: true,
                    portClosed: true,
                    ownedVolumesAbsent: { owned: true },
                  })),
                );
          },
          postInputs: () =>
            fault === "post" || fault === "both"
              ? Promise.reject(new Error("SECOND_PARENT_PRIVATE"))
              : Promise.resolve(structuredClone(sourceBefore)),
          emit: (line) => emitted.push(line),
        },
      );
      expect(code).toBe(7);
      expect(writes).toEqual(["fixtures"]);
      expect(existsSync(join(run, "parent-original-failure.private.json"))).toBe(true);
      // The same nine-key packet the child writes, browser fields null.
      expect(
        Object.keys(
          JSON.parse(
            readFileSync(join(run, "parent-original-failure.private.json"), "utf8"),
          ) as object,
        ),
      ).toEqual([
        "failed_phase",
        "observed_failed_exit",
        "failure_code",
        "original_driver_failure",
        "original_body_log_sha256",
        "known_browser_test",
        "known_browser_status",
        "known_browser_checkpoint",
        "browser_report_state",
      ]);
      expect(emitted.join("")).not.toContain("SECOND_PARENT_PRIVATE");
      const summary = JSON.parse(emitted[0] as string) as Record<string, unknown>;
      expect(summary.final_exit_code).toBe(7);
      expect(summary.failure_code).toBe("SELECTED_PG_PARENT_FAILED");
      if (fault === "write") {
        expect(summary.cleanup_failure_codes).toContain("parent-final-receipt-write-failed");
        expect(summary.parent_receipt_sha256).toBe(hash("occupied"));
      } else {
        const receipt = JSON.parse(
          readFileSync(join(run, "parent-receipt.json"), "utf8"),
        ) as Record<string, unknown>;
        expect(receipt.wrapper_exit).toBe(7);
        expect(receipt.failed_phase).toBe("owned-fixture-wrapper");
        expect(receipt.all_owned_fixtures_closed).toBe(fault === "post" || fault === "none");
        expect(receipt.exact_source_artifact_inputs_unchanged).toBe(
          fault === "fixture" || fault === "none",
        );
      }
      // The restart binding recomputes this digest from the same inputs.
      expect(hash(readFileSync(join(run, "source-inputs-before.json")))).toBe(
        digest(sourceInputText(sourceBefore)),
      );
      expect(statSync(run).mode & 0o777).toBe(0o700);
    });
  test("a zero wrapper with closed fixtures and a passing child returns zero", async () => {
    const run = join(root, "root-current-postgres-0123456789ab");
    const emitted: string[] = [];
    const code = await parent(
      run,
      { head: SOURCE, tree: TREE, owner: OWNER, flow: "off", sourceBefore },
      {
        command: () => {
          writeFileSync(join(run, "receipt.json"), '{"final_exit_code": 0}');
          return Promise.resolve({ returncode: 0, stdout: "", stderr: "" });
        },
        verifyFixturesClosed: () =>
          Promise.resolve(
            ["pg", "meili"].map((kind) => ({
              kind,
              name: "owned",
              containerAbsent: true,
              recordedPIDIdentitiesRetired: true,
              portClosed: true,
              ownedVolumesAbsent: {},
            })),
          ),
        postInputs: () => Promise.resolve(structuredClone(sourceBefore)),
        emit: (line) => emitted.push(line),
      },
    );
    expect(code).toBe(0);
    expect(JSON.parse(emitted[0] as string)).toMatchObject({
      final_exit_code: 0,
      failure_code: null,
      cleanup_failure_codes: [],
    });
    expect(existsSync(join(run, "parent-original-failure.private.json"))).toBe(false);
  });
  test("a float exit token, one fixture or an open port is not a pass", async () => {
    for (const variant of ["float", "one", "port"] as const) {
      const run = join(root, "root-current-postgres-" + variant.padEnd(12, "0").slice(0, 12));
      const code = await parent(
        run,
        { head: SOURCE, tree: TREE, owner: OWNER, flow: "on", sourceBefore },
        {
          command: () => {
            writeFileSync(
              join(run, "receipt.json"),
              `{"final_exit_code": ${variant === "float" ? "0.0" : "0"}}`,
            );
            return Promise.resolve({ returncode: 0, stdout: "", stderr: "" });
          },
          verifyFixturesClosed: () =>
            Promise.resolve(
              (variant === "one" ? ["pg"] : ["pg", "meili"]).map((kind) => ({
                kind,
                name: "owned",
                containerAbsent: true,
                recordedPIDIdentitiesRetired: true,
                portClosed: variant !== "port",
                ownedVolumesAbsent: {},
              })),
            ),
          postInputs: () => Promise.resolve(structuredClone(sourceBefore)),
          emit: () => undefined,
        },
      );
      expect([variant, code]).toEqual([variant, 1]);
    }
  });
});
