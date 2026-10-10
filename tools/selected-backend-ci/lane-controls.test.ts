// Runner and lane-driver fail-closed controls. Each case is a behaviour the
// removed scripts/selected-backend-ci/test_*.py files guarded and that no
// drivers/*.test.ts or runner.test.ts case already covers.
import { spawnSync } from "bun";
import { afterEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { identity, localAllocation, registeredModules } from "./admission.ts";
import { qualifyArtifacts } from "./build.ts";
import { engineFeaturesMatch, validateOffReport } from "./drivers/binding.ts";
import { failureDigest } from "./drivers/common.ts";
import { checkpointPrefix, preparationSteps } from "./drivers/sqlite.ts";
import { call, digest, gid, parseJson, read, root, sha, uid, write } from "./io.ts";
import {
  knownOnBrowserTest,
  laneDriver,
  laneRetirement,
  ownershipReturn,
  publicFailureFields,
  run,
} from "./runtime.ts";
import type { RunBoundary } from "./runtime.ts";
import type {
  Aggregate,
  Artifact,
  Browser,
  DriverReceipt,
  Flow,
  Inputs,
  Lane,
  LocalGrant,
} from "./types.ts";

const linux = process.platform === "linux";
const temporary: string[] = [];
function directory(): string {
  const path = mkdtempSync(join(tmpdir(), "selected-lane-controls-"));
  chmodSync(path, 0o700);
  temporary.push(path);
  return path;
}
afterEach(() => {
  for (const path of temporary.splice(0)) rmSync(path, { recursive: true, force: true });
});
async function withEnvironment<T>(
  values: Record<string, string | undefined>,
  body: () => T | Promise<T>,
): Promise<T> {
  const old = { ...process.env };
  for (const key of Object.keys(process.env))
    if (key.startsWith("GITHUB_") || key.startsWith("FVOCI_") || key === "CI")
      Reflect.deleteProperty(process.env, key);
  for (const [key, value] of Object.entries(values))
    if (value === undefined) Reflect.deleteProperty(process.env, key);
    else process.env[key] = value;
  try {
    return await body();
  } finally {
    for (const key of Object.keys(process.env))
      if (!(key in old)) Reflect.deleteProperty(process.env, key);
    Object.assign(process.env, old);
  }
}
// Everything a body writes to stdout, and what it returned or threw.
async function stdoutOf<T>(
  body: () => T | Promise<T>,
): Promise<{ value?: T; error?: unknown; stdout: string }> {
  const original = process.stdout.write.bind(process.stdout);
  let stdout = "";
  process.stdout.write = ((chunk: string | Uint8Array) => {
    stdout +=
      typeof chunk === "string"
        ? chunk
        : new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(chunk);
    return true;
  }) as typeof process.stdout.write;
  try {
    return { value: await body(), stdout };
  } catch (error) {
    return { error, stdout };
  } finally {
    process.stdout.write = original;
  }
}

describe("engine features", () => {
  test("exactly default and worker, each once, in any order", () => {
    expect(engineFeaturesMatch(["default", "worker"])).toBe(true);
    expect(engineFeaturesMatch(["worker", "default"])).toBe(true);
    for (const features of [
      ["worker"],
      ["default"],
      ["default", "test-hang", "worker"],
      ["default", "worker", "worker"],
      [],
    ])
      expect(engineFeaturesMatch(features)).toBe(false);
  });
  test("record-after qualifies exactly the engine feature lists the binding admits", () => {
    const output = directory();
    const before: Inputs = {
      head: "a".repeat(40),
      tree: "b".repeat(40),
      status: "",
      tracked: {},
      external: {},
      untracked: {},
    };
    for (const engine of [
      ["default", "worker"],
      ["worker", "default"],
      ["worker"],
      ["default"],
      ["default", "test-hang", "worker"],
      ["default", "worker", "worker"],
      [],
    ]) {
      const artifacts: Artifact[] = [
        "fvoci-server",
        "fvoci-migrate",
        "fvoci-e2e-fixture",
        "fvoci_server",
        "selected_install_lifetime",
        "collab-engine",
      ].map((name) => {
        const path = join(output, name);
        writeFileSync(path, name, { mode: 0o755 });
        return {
          reason: "compiler-artifact",
          executable: path,
          target: { name },
          profile: { test: ["fvoci_server", "selected_install_lifetime"].includes(name) },
          features: name === "collab-engine" ? engine : ["api-schema", "db-tests"],
          fresh: false,
          filenames: [path],
        };
      });
      let qualified = true;
      try {
        qualifyArtifacts(artifacts, before);
      } catch {
        qualified = false;
      }
      expect([engine, qualified]).toEqual([engine, engineFeaturesMatch(engine)]);
    }
  });
});

// loadCurrent compares the tracked digest of each pinned spec with a literal.
// The literal must be the committed spec, or every selected lane refuses late.
describe("pinned specs", () => {
  const binding = readFileSync(join(import.meta.dir, "drivers/binding.ts"), "utf8");
  for (const [spec, pattern] of [
    [
      "apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts",
      /before\.tracked\[wikiSpec\] === "([0-9a-f]{64})"/,
    ],
    [
      "apps/web/e2e-pending/workspace-off-selected-backend.spec.ts",
      /const offSpecSha256 = "([0-9a-f]{64})"/,
    ],
    [
      "apps/web/e2e-pending/workspace-wiki-selected-auxiliary.ts",
      /"apps\/web\/e2e-pending\/workspace-wiki-selected-auxiliary\.ts"\] ===\s*"([0-9a-f]{64})"/,
    ],
  ] as const)
    test("the " + spec + " pin is the committed spec", () => {
      const pin = pattern.exec(binding)?.[1];
      const committed = spawnSync(["git", "show", "HEAD:" + spec], { cwd: root });
      expect(committed.exitCode).toBe(0);
      expect(pin).toBe(digest(committed.stdout));
      expect(pin).toBe(sha(join(root, spec)));
    });
});

describe("OFF report", () => {
  const titles = [
    ...readFileSync(
      join(root, "apps/web/e2e-pending/workspace-off-selected-backend.spec.ts"),
      "utf8",
    ).matchAll(/ {2}test\("([^"\n]+)"/g),
  ].map((match) => match[1] as string);
  interface Result {
    status: string;
    retry: number;
    errors: unknown[];
  }
  interface Case {
    title: string;
    file: string;
    ok: boolean;
    tests: { expectedStatus: string; results: Result[] }[];
  }
  interface Report {
    config: { workers: number; metadata: Record<string, string> };
    errors: unknown[];
    stats: Record<string, number>;
    suites: { suites: { specs: Case[] }[] }[];
  }
  const report = (backend: string): Report => ({
    config: { workers: 1, metadata: { selectedBackend: backend, selectedFlow: "off" } },
    errors: [],
    stats: { expected: 8, unexpected: 0, flaky: 0, skipped: 0 },
    suites: [
      {
        suites: [
          {
            specs: titles.map((title) => ({
              title,
              file: "workspace-off-selected-backend.spec.ts",
              ok: true,
              tests: [
                { expectedStatus: "passed", results: [{ status: "passed", retry: 0, errors: [] }] },
              ],
            })),
          },
        ],
      },
    ],
  });
  const validate = (value: Report, backend: string) =>
    validateOffReport(parseJson(JSON.stringify(value)), backend);
  test("the eight registered cases pass for both backends", () => {
    expect(titles).toHaveLength(8);
    for (const backend of ["sqlite", "postgres"])
      expect(validate(report(backend), backend)).toEqual(titles);
  });
  const cases = (r: Report) => r.suites[0]?.suites[0]?.specs as Case[];
  const first = (r: Report) => cases(r)[0]?.tests[0]?.results[0] as Result;
  const defects: Record<string, (r: Report) => void> = {
    duplicate: (r) => {
      cases(r)[7] = structuredClone(cases(r)[0] as Case);
    },
    "extra attempt": (r) => {
      cases(r)[0]?.tests[0]?.results.push(structuredClone(first(r)));
    },
    skipped: (r) => {
      first(r).status = "skipped";
    },
    failed: (r) => {
      first(r).status = "failed";
    },
    unexpected: (r) => {
      r.stats.unexpected = 1;
    },
    "foreign backend": (r) => {
      r.config.metadata.selectedBackend = "foreign";
    },
    "foreign spec": (r) => {
      (cases(r)[0] as Case).file = "workspace-wiki-selected-backend.spec.ts";
    },
  };
  for (const backend of ["sqlite", "postgres"])
    for (const [name, defect] of Object.entries(defects))
      test(`${backend}: ${name} is refused`, () => {
        const changed = report(backend);
        defect(changed);
        expect(() => validate(changed, backend)).toThrow();
      });
});

const SOURCE = "a".repeat(40),
  TREE = "b".repeat(40),
  OWNER = "pure-owned-control";
const SECRET = "http://secret.example/a cookie=PRIVATE_LANE_SECRET";
const PRIVATE = { type: "AssertionError", message: SECRET };
const failedReceipt = (): DriverReceipt => ({
  source: SOURCE,
  tree: TREE,
  root_owner: OWNER,
  final_exit_code: 7,
  selected_flow: "on",
  owned_container_absent: true,
  owned_loopback_port_closed: true,
  recorded_process_identities_retired: true,
  cleanup_errors: [],
  failed_phase: "server-startup",
  original_driver_failure: PRIVATE,
});
const parentReceipt = (): DriverReceipt => ({
  source: SOURCE,
  tree: TREE,
  root_owner: OWNER,
  selected_flow: "on",
  all_owned_fixtures_closed: true,
});

describe("lane retirement of a failed lane", () => {
  const retire = (
    receipt: DriverReceipt | string,
    parent: DriverReceipt = parentReceipt(),
    exit = 7,
  ) => {
    const path = directory();
    writeFileSync(
      join(path, "receipt.json"),
      typeof receipt === "string" ? receipt : JSON.stringify(receipt),
    );
    writeFileSync(join(path, "parent-receipt.json"), JSON.stringify(parent));
    return laneRetirement(path, "postgres", "on", SOURCE, TREE, OWNER, exit);
  };
  test("a closed lane keeps its product failure: phase and digest, never the message", () => {
    const facts = retire(failedReceipt());
    expect(facts.qualified).toBe(true);
    expect(facts.failedPhase).toBe("server-startup");
    expect(facts.originalFailureSha256).toBe(failureDigest(PRIVATE));
    expect(facts.originalFailureSha256).toBe(
      digest('{"message":' + JSON.stringify(SECRET) + ',"type":"AssertionError"}'),
    );
    expect(JSON.stringify(facts)).not.toContain("PRIVATE_LANE_SECRET");
  });
  for (const value of ["missing", null, false, 1, "true"] as const)
    test("an unobserved loopback port is not a closed one: " + String(value), () => {
      const receipt = failedReceipt();
      if (value === "missing") Reflect.deleteProperty(receipt, "owned_loopback_port_closed");
      else (receipt as Record<string, unknown>).owned_loopback_port_closed = value;
      const facts = retire(receipt);
      expect(facts.qualified).toBe(false);
      expect(facts.refusalCodes).toEqual(["SELECTED_DRIVER_RETIREMENT_UNCONFIRMED"]);
      expect(facts.originalFailureSha256).toBe(failureDigest(PRIVATE));
    });
  test("each false parent fixture fact and an exit mismatch refuse", () => {
    for (const key of [
      "source",
      "tree",
      "root_owner",
      "selected_flow",
      "all_owned_fixtures_closed",
    ]) {
      const parent = parentReceipt();
      parent[key] = false;
      expect([key, retire(failedReceipt(), parent).qualified]).toEqual([key, false]);
    }
    expect(retire(failedReceipt(), parentReceipt(), 0).qualified).toBe(false);
  });
  test("an unreadable or non-object receipt is unconfirmed, with its digest", () => {
    for (const raw of ["not-json", "[]", "null", "{}"]) {
      const facts = retire(raw);
      expect(facts.qualified).toBe(false);
      expect(facts.receiptSha256).toBe(digest(raw));
    }
    const path = directory();
    expect(laneRetirement(path, "postgres", "on", SOURCE, TREE, OWNER, 7)).toMatchObject({
      qualified: false,
      receiptPresent: false,
      receiptSha256: null,
    });
  });
});

describe("owner-return diagnostics", () => {
  // One recorded postgres/on run whose receipt is `receipt`; owner-return
  // must refuse (the other four lanes never ran) and keep its diagnostic.
  async function refusedReturn(receipt: Record<string, unknown>, parent = false) {
    const output = directory(),
      runtime = join(output, "runtime"),
      runRoot = join(runtime, "root-current-postgres-0123456789ab");
    mkdirSync(runRoot, { recursive: true });
    write(join(runRoot, "receipt.json"), receipt);
    if (parent) write(join(runRoot, "parent-receipt.json"), parentReceipt());
    write(join(output, "before.json"), { head: SOURCE, tree: TREE });
    write(join(output, "postgres-on-allocation.json"), {});
    write(join(output, "selected-ci-receipt.json"), {
      owner: OWNER,
      source: SOURCE,
      tree: TREE,
      exit: receipt.final_exit_code,
      allRequestedRunsExecuted: false,
      runs: [
        {
          lane: "postgres",
          flow: "on",
          exit: receipt.final_exit_code,
          actualSource: SOURCE,
          runRoot,
        },
      ],
    });
    const result = await withEnvironment(ci, () =>
      stdoutOf(() => {
        ownershipReturn(output, [uid(), gid()], undefined, () => OWNER);
      }),
    );
    expect(result.error).toBeInstanceOf(Error);
    expect(existsSync(join(output, "runtime-close-stage.json"))).toBe(false);
    expect(result.stdout).not.toContain("PRIVATE_LANE_SECRET");
    const diagnostic = JSON.parse(result.stdout) as {
      ownership_return_qualified: boolean;
      selected_exit: number;
      proof_error_type: string;
      lanes: Record<string, unknown>[];
    };
    expect(diagnostic.ownership_return_qualified).toBe(false);
    expect(diagnostic.lanes).toHaveLength(1);
    return { diagnostic, lane: diagnostic.lanes[0] as Record<string, unknown> };
  }
  test("a partial failure has a diagnostic but returns no ownership", async () => {
    const { diagnostic, lane } = await refusedReturn({
      ...failedReceipt(),
      owned_loopback_port_closed: null,
    });
    expect(diagnostic.selected_exit).toBe(7);
    expect(lane.invalid_required_fields).toEqual(["owned_loopback_port_closed"]);
    expect(lane.original_driver_failure_sha256).toBe(failureDigest(PRIVATE));
    expect(lane.failed_phase).toBe("server-startup");
    expect(lane.original_driver_failure_type).toBe("AssertionError");
    expect(lane.original_driver_failure_code).toBeNull();
  });
  for (const [phase, kind, code, published] of [
    ["server-ready", "AssertionError", "SELECTED_DRIVER_EXCEPTION", true],
    ["browser", "ReturnedNonzero", "SELECTED_BODY_NONZERO", true],
    ["install-body", "RuntimeError", "SELECTED_DRIVER_EXCEPTION", true],
    ["owned-fixture-wrapper", "OSError", "SELECTED_DRIVER_FAILED", false],
    [SECRET, SECRET, SECRET, false],
  ] as const)
    test("only whitelisted phase, type and code are published: " + phase.slice(0, 24), async () => {
      const failure = { type: kind, message: SECRET, phase, observedExit: 7, code };
      const { lane } = await refusedReturn({
        ...failedReceipt(),
        owned_loopback_port_closed: null,
        failed_phase: phase,
        failure_code: code,
        original_driver_failure: failure,
      });
      expect([lane.failed_phase, lane.original_driver_failure_type]).toEqual(
        published ? [phase, kind] : [null, null],
      );
      expect(lane.original_driver_failure_code).toBe(published ? code : null);
      expect(lane.original_driver_failure_sha256).toBe(failureDigest(failure));
      expect("original_driver_failure" in lane).toBe(false);
    });
  test("a closed failed lane publishes its facts and still requires every lane", async () => {
    const { diagnostic, lane } = await refusedReturn(
      {
        ...failedReceipt(),
        failed_phase: "server-ready",
        failure_code: "SELECTED_DRIVER_EXCEPTION",
      },
      true,
    );
    expect(diagnostic.proof_error_type).toBe("AssertionError");
    expect(lane).toMatchObject({
      lane: "postgres",
      flow: "on",
      failed_phase: "server-ready",
      original_driver_failure_type: "AssertionError",
      original_driver_failure_code: "SELECTED_DRIVER_EXCEPTION",
      closure_facts: {
        owned_container_absent: true,
        owned_loopback_port_closed: true,
        recorded_process_identities_retired: true,
      },
      cleanup_error_count: 0,
      browser_report_state: null,
      known_browser_test: null,
      known_browser_status: null,
      known_browser_checkpoint: null,
    });
  });
  test("a browser failure publishes its matched checkpoint; a missing report only its state", async () => {
    const browser: DriverReceipt = {
      ...failedReceipt(),
      failed_phase: "browser",
      failure_code: "SELECTED_BODY_NONZERO",
      original_driver_failure: { type: "ReturnedNonzero", phase: "browser", observedExit: 1 },
      browser_report_state: "matched",
      known_browser_test: knownOnBrowserTest,
      known_browser_status: "failed",
      known_browser_checkpoint: "e2e-pending/workspace-wiki-selected-backend.spec.ts:413",
    };
    expect((await refusedReturn(browser, true)).lane).toMatchObject({
      browser_report_state: "matched",
      known_browser_test: knownOnBrowserTest,
      known_browser_status: "failed",
      known_browser_checkpoint: "e2e-pending/workspace-wiki-selected-backend.spec.ts:413",
    });
    const missing = {
      ...browser,
      browser_report_state: "report-missing",
      known_browser_test: null,
      known_browser_status: null,
      known_browser_checkpoint: null,
    };
    expect((await refusedReturn(missing, true)).lane).toMatchObject({
      browser_report_state: "report-missing",
      known_browser_status: null,
      known_browser_checkpoint: null,
    });
  });
});

describe("public preparation checkpoint", () => {
  const step = checkpointPrefix + "network-driver-host";
  // Receipt values as the runner reads them: parsed JSON with integer tokens.
  const facts = (change: Record<string, unknown> = {}) =>
    parseJson(
      JSON.stringify({
        failed_phase: "container-prepare",
        original_driver_failure: { type: "AssertionError", message: "PRIVATE" },
        failure_code: "SELECTED_DRIVER_EXCEPTION",
        known_driver_checkpoint: step,
        preparation_command_exit: 0,
        ...change,
      }),
    ) as DriverReceipt;
  test("every registered sqlite step is published with its command exit", () => {
    for (const name of preparationSteps)
      expect(
        publicFailureFields(
          facts({
            known_driver_checkpoint: checkpointPrefix + name,
            preparation_command_exit: 125,
          }),
        ),
      ).toMatchObject({
        failed_phase: "container-prepare",
        known_driver_checkpoint: checkpointPrefix + name,
        preparation_command_exit: 125,
      });
    expect(JSON.stringify(publicFailureFields(facts()))).not.toContain("PRIVATE");
  });
  test("any other checkpoint is withheld together with its exit", () => {
    for (const checkpoint of [
      null,
      244,
      "https://private.example/a",
      "scripts/selected-backend-ci/current-sqlite-driver.py:244",
      "tools/selected-backend-ci/drivers/postgres.ts#container-create",
      "/foreign/tools/selected-backend-ci/drivers/sqlite.ts#container-create",
      checkpointPrefix,
      checkpointPrefix + "unregistered-step",
      checkpointPrefix + "container-create:12",
      step + "\nPRIVATE",
    ])
      expect(publicFailureFields(facts({ known_driver_checkpoint: checkpoint }))).toMatchObject({
        known_driver_checkpoint: null,
        preparation_command_exit: null,
      });
  });
  test("a non-integer or out-of-range exit is withheld", () => {
    for (const exit of [null, true, "0", 256, -256, 1.5, { secret: "PRIVATE" }])
      expect(
        publicFailureFields(facts({ preparation_command_exit: exit })).preparation_command_exit,
      ).toBeNull();
    expect(publicFailureFields(facts({ preparation_command_exit: -255 }))).toMatchObject({
      preparation_command_exit: -255,
    });
  });
  test("only a container-prepare failure with a known type and code shows a checkpoint", () => {
    for (const change of [
      { failed_phase: "browser" },
      { failed_phase: "server-ready" },
      { failed_phase: "restart" },
      { failed_phase: "PRIVATE" },
      { failure_code: "PRIVATE" },
      { original_driver_failure: { type: "OSError", message: "PRIVATE" } },
    ])
      expect(publicFailureFields(facts(change))).toMatchObject({
        known_driver_checkpoint: null,
        preparation_command_exit: null,
      });
  });
});

const ci = {
  CI: "true",
  GITHUB_ACTIONS: "true",
  GITHUB_SHA: SOURCE,
  GITHUB_REPOSITORY: "fixture/repo",
  GITHUB_RUN_ID: "123",
  GITHUB_RUN_ATTEMPT: "1",
  GITHUB_JOB: "collaboration-flow",
};
function cohort() {
  const output = directory();
  const before: Inputs = {
    head: SOURCE,
    tree: TREE,
    status: "",
    tracked: {},
    external: {},
    untracked: {},
  };
  const browser: Browser = {
    bun: { path: process.execPath, sha256: sha(process.execPath) },
    chromium: { path: process.execPath, sha256: sha(process.execPath) },
    chromium_directory_files: {},
  };
  for (const name of ["before.json", "after.json"]) write(join(output, name), before);
  write(join(output, "bundle.json"), { source: SOURCE, tree: TREE, binaries: {} });
  write(join(output, "compile-receipt.json"), { source: SOURCE, tree: TREE });
  write(join(output, "web-receipt.json"), { source: SOURCE, tree: TREE, dist_files: {} });
  write(join(output, "abi-receipt.json"), { currentSource: SOURCE, host_runtime_files: {} });
  const boundary: RunBoundary = {
    identity: () => OWNER,
    browser: () => browser,
    access: () => undefined,
    execute: (_driver, environment) => {
      const grant = read(environment.FVOCI_ROOT_CURRENT_ALLOCATION as string) as {
        runRoot: string;
        lane: Lane;
        flow: Flow;
      };
      mkdirSync(grant.runRoot);
      write(join(grant.runRoot, "receipt.json"), {
        source: SOURCE,
        tree: TREE,
        root_owner: OWNER,
        final_exit_code: 0,
        owned_container_absent: true,
        ...(grant.lane === "install"
          ? { actual_tests: 4, actual_owned_process_receipts: 15 }
          : {
              selected_flow: grant.flow,
              cleanup_errors: [],
              owned_loopback_port_closed: true,
              recorded_process_identities_retired: true,
              current_schema_server_restart: { restartBrowserExit: 0 },
              actual_browser_tests: grant.flow === "off" ? 8 : 1,
              retries: 0,
            }),
      });
      if (grant.lane === "postgres")
        write(join(grant.runRoot, "parent-receipt.json"), {
          source: SOURCE,
          tree: TREE,
          root_owner: OWNER,
          selected_flow: grant.flow,
          all_owned_fixtures_closed: true,
        });
      return 0;
    },
  };
  return { output, boundary };
}

describe("selected launcher", () => {
  test("each lane gets a CI-job grant for its .ts driver and no Python environment", async () => {
    const { output, boundary } = cohort();
    const seen: string[] = [];
    const execute = boundary.execute;
    let browsers = 0;
    const browser = boundary.browser;
    boundary.browser = (path) => {
      browsers++;
      return browser(path);
    };
    boundary.execute = (driver, environment, log, signal) => {
      const grant = read(environment.FVOCI_ROOT_CURRENT_ALLOCATION as string) as {
        lane: Lane;
        flow: Flow;
        exclusiveCIJob: boolean;
        currentCIJobConfirmed: boolean;
      };
      const manifest = read(environment.FVOCI_ROOT_CURRENT_BINDING as string) as { flow: Flow };
      expect(driver).toBe(laneDriver(grant.lane));
      expect(driver.endsWith("tools/selected-backend-ci/drivers/" + grant.lane + ".ts")).toBe(true);
      expect(grant.flow).toBe(manifest.flow);
      expect(grant.exclusiveCIJob && grant.currentCIJobConfirmed).toBe(true);
      expect(Object.keys(environment).filter((key) => key.startsWith("PYTHON"))).toEqual([]);
      seen.push(grant.lane + "/" + grant.flow);
      return execute(driver, environment, log, signal);
    };
    // The runner adds no Python name; the job's own environment is not under test.
    await withEnvironment({ ...ci, PYTHONDONTWRITEBYTECODE: undefined }, async () => {
      expect(await run(output, boundary, [uid(), gid()])).toBe(0);
    });
    expect(seen).toEqual(["install/on", "postgres/on", "sqlite/on", "postgres/off", "sqlite/off"]);
    expect(browsers).toBe(1);
  });
  test("a refused browser admission starts no lane and writes no grant", async () => {
    const { output, boundary } = cohort();
    let executed = 0;
    boundary.browser = () => {
      throw new Error("admitted browser mismatch");
    };
    boundary.execute = () => {
      executed++;
      return 0;
    };
    await withEnvironment(ci, async () => {
      await expect(run(output, boundary, [uid(), gid()])).rejects.toThrow(
        "admitted browser mismatch",
      );
    });
    expect(executed).toBe(0);
    expect(readdirSync(output).filter((name) => /-(allocation|binding)\.json$/.test(name))).toEqual(
      [],
    );
  });
  test("an unwritable private original keeps the first exit and the aggregate", async () => {
    const { output, boundary } = cohort();
    boundary.execute = () => {
      throw new Error("PRIVATE_FIRST_OUTCOME");
    };
    write(join(output, "selected-launcher-failure.private.json"), { occupied: true });
    const result = await withEnvironment(ci, () =>
      stdoutOf(() => run(output, boundary, [uid(), gid()])),
    );
    expect(result.value).toBe(1);
    const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate;
    expect(aggregate).toMatchObject({ exit: 1, allRequestedRunsExecuted: false, runs: [] });
    expect(aggregate.launcherFailure).toMatchObject({
      code: "SELECTED_LAUNCHER_FAILED",
      sha256: null,
      receiptWrite: "failed",
    });
    expect(read(join(output, "selected-launcher-failure.private.json"))).toEqual({
      occupied: true,
    });
  });
  for (const original of ["written", "unwritable"] as const)
    test(`an unwritable aggregate prints the first exit; the original is ${original}`, async () => {
      const { output, boundary } = cohort();
      boundary.execute = () => {
        throw new Error("PRIVATE_FIRST_OUTCOME");
      };
      write(join(output, "selected-ci-receipt.json"), { occupied: true });
      if (original === "unwritable")
        write(join(output, "selected-launcher-failure.private.json"), { occupied: true });
      const result = await withEnvironment(ci, () =>
        stdoutOf(() => run(output, boundary, [uid(), gid()])),
      );
      expect(result.value).toBe(1);
      expect(result.stdout).not.toContain("PRIVATE_FIRST_OUTCOME");
      const printed = JSON.parse(result.stdout.trim().split("\n").at(-1) as string) as {
        exit: number;
        aggregateReceiptWrite: string;
        launcherFailure: { receiptWrite: string; originalOutcomeSha256: string };
      };
      expect(printed).toMatchObject({ exit: 1, aggregateReceiptWrite: "failed" });
      expect(printed.launcherFailure.receiptWrite).toBe(
        original === "written" ? "confirmed" : "failed",
      );
      expect(printed.launcherFailure.originalOutcomeSha256).toMatch(/^[0-9a-f]{64}$/);
    });
});

describe("local lease", () => {
  const source = call(["git", "rev-parse", "HEAD"]),
    tree = call(["git", "rev-parse", "HEAD^{tree}"]);
  function lease(change: (grant: LocalGrant) => void = () => undefined) {
    const output = directory(),
      path = join(output, "local.json");
    const grant: LocalGrant = {
      schema: 1,
      status: "GRANTED",
      executionMode: "orca-local",
      exclusiveLocalBatch: true,
      currentDispatchConfirmed: true,
      owner: "orca:fixture",
      runId: "run_ab12",
      dispatchId: "ctx_cd34",
      taskId: "task_ef56",
      workerTerminal: "fixture-worker",
      rootTerminal: "fixture-root",
      worktree: root,
      uid: uid(),
      gid: gid(),
      source,
      tree,
      allowedModes: ["record-before", "stage", "record-after", "run"],
      expiresUtc: new Date(Date.now() + 3600000).toISOString(),
      outputRoot: output,
      registrationHashes: Object.fromEntries(
        registeredModules.map((name) => [name, sha(join(root, name))]),
      ),
      stageCommands: {},
    };
    change(grant);
    write(path, grant);
    const env: Record<string, string | undefined> = {
      FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
      FVOCI_SELECTED_LOCAL_ALLOCATION: path,
      FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256: sha(path),
      FVOCI_CI_OWNER: "orca:fixture",
      FVOCI_ROOT_RUN_OWNER: "orca:fixture",
      FVOCI_LOCAL_RUN_ID: "run_ab12",
      FVOCI_LOCAL_DISPATCH_ID: "ctx_cd34",
      FVOCI_LOCAL_TASK_ID: "task_ef56",
      ORCA_TERMINAL_HANDLE: "fixture-worker",
      FVOCI_LOCAL_ROOT_TERMINAL: "fixture-root",
      FVOCI_CI_SELECTED_RUNS: join(output, "runtime"),
    };
    return { grant, path, output, env };
  }
  const admitted = (env: Record<string, string | undefined>) =>
    withEnvironment(env, () => localAllocation("run", [uid(), gid()]));
  test("the valid lease is admitted and binds the runner and lane driver modules", async () => {
    const { grant, env } = lease();
    expect(await admitted(env)).toEqual(grant);
    expect(Object.keys(grant.registrationHashes)).toEqual([
      "scripts/run-selected-backend-e2e.ts",
      "tools/selected-backend-ci/drivers/common.ts",
      "tools/selected-backend-ci/drivers/binding.ts",
      "tools/selected-backend-ci/drivers/restart.ts",
      "tools/selected-backend-ci/drivers/install.ts",
      "tools/selected-backend-ci/drivers/postgres.ts",
      "tools/selected-backend-ci/drivers/sqlite.ts",
    ]);
  });
  for (const name of registeredModules)
    for (const fault of ["changed", "missing"] as const)
      test(`a lease with a ${fault} ${name} hash is refused`, async () => {
        const { env } = lease((grant) => {
          if (fault === "changed") grant.registrationHashes[name] = "0".repeat(64);
          else Reflect.deleteProperty(grant.registrationHashes, name);
        });
        await expect(admitted(env)).rejects.toThrow();
      });
  for (const [field, value] of [
    ["worktree", "/foreign"],
    ["dispatchId", "ctx_ef01"],
    ["status", "NOTRUN"],
    ["exclusiveLocalBatch", false],
    ["currentDispatchConfirmed", false],
    ["workerTerminal", "foreign-worker"],
    ["executionMode", "github-ci"],
    ["schema", 2],
  ] as const)
    test(`a lease with a foreign ${field} is refused`, async () => {
      const { env } = lease((grant) => {
        (grant as unknown as Record<string, unknown>)[field] = value;
      });
      await expect(admitted(env)).rejects.toThrow();
    });
  for (const mode of [0o640, 0o644, 0o700])
    test("a lease file with mode " + mode.toString(8) + " is refused", async () => {
      const { path, env } = lease();
      chmodSync(path, mode);
      await expect(admitted(env)).rejects.toThrow();
    });
  test.skipIf(!linux)("a lease file owned by another uid or root is refused", async () => {
    for (const owner of ["1001:1001", "0:0"]) {
      const { path, env } = lease();
      expect(spawnSync(["sudo", "-n", "chown", owner, path]).exitCode).toBe(0);
      try {
        expect(statSync(path).uid).not.toBe(uid());
        await expect(admitted(env)).rejects.toThrow();
      } finally {
        spawnSync(["sudo", "-n", "chown", `${String(uid())}:${String(gid())}`, path]);
      }
    }
  });
  test("a symlinked or absent lease path is refused", async () => {
    const { path, output, env } = lease();
    const link = join(output, "link.json");
    symlinkSync(path, link);
    await expect(admitted({ ...env, FVOCI_SELECTED_LOCAL_ALLOCATION: link })).rejects.toThrow();
    await expect(
      admitted({ ...env, FVOCI_SELECTED_LOCAL_ALLOCATION: undefined }),
    ).rejects.toThrow();
  });
  for (const [key, value] of [
    ["CI", "true"],
    ["GITHUB_ACTIONS", "true"],
    ["GITHUB_SHA", "a".repeat(40)],
    ["GITHUB_RUN_ID", "123"],
    ["FVOCI_SELECTED_EXECUTION_MODE", "github-ci"],
  ] as const)
    test(`a CI job cannot fall back to a valid lease: ${key}`, async () => {
      const { env } = lease();
      await expect(admitted({ ...env, [key]: value })).rejects.toThrow();
    });
  test("without an execution mode the runner never consumes a local lease", async () => {
    const { env, output } = lease();
    await withEnvironment({ ...env, FVOCI_SELECTED_EXECUTION_MODE: undefined }, () => {
      expect(() => identity("run", output)).toThrow("allocated GitHub CI job only");
    });
  });
});

// The consume-phase config-list leaf and its launcher gate in
// scripts/run-web-e2e.sh, with sudo replaced by a recorder.
describe("config-list launcher gate", () => {
  const script = readFileSync(join(root, "scripts/run-web-e2e.sh"), "utf8");
  const start = script.indexOf("  config_list_exit=not-run\n"),
    end = script.indexOf('\nfi\nif [[ "$pending_status"', start);
  function gate(leaf: number, ownerExit: number, occupied?: "stdout" | "stderr") {
    expect(start).toBeGreaterThan(0);
    expect(end).toBeGreaterThan(start);
    const base = directory(),
      safe = join(base, "safe");
    mkdirSync(safe, { mode: 0o700 });
    if (occupied) writeFileSync(join(safe, `config-list.${occupied}.log`), "OLD_CAPTURE");
    const fragment = join(base, "fragment.sh");
    writeFileSync(
      fragment,
      `set -euo pipefail
sudo() {
  for arg in "$@"; do
    case "$arg" in
      config-list) printf 'list\\n' >> "$MARKS"; printf 'CREDENTIAL_FREE_LIST\\n'; printf 'ORIGINAL_LOAD_ERROR\\n' >&2; return "$LEAF" ;;
      run) printf 'run\\n' >> "$MARKS"; return 0 ;;
      owner-return) printf 'owner\\n' >> "$MARKS"; printf '{"ownership_return_qualified":false}\\n'; return "$OWNER_EXIT" ;;
    esac
  done
  return 0
}
` +
        script.slice(start, end) +
        '\nexit "$selected_status"\n',
    );
    const marks = join(base, "marks");
    const result = spawnSync(["bash", fragment], {
      env: {
        PATH: join(process.execPath, "..") + ":/usr/bin:/bin",
        SELECTED_PHASE: "consume",
        ROOT: root,
        safe_diagnostics: safe,
        FVOCI_SELECTED_CI_OUTPUT: join(base, "output"),
        FVOCI_SELECTED_CI_SQLITE_PARENT: join(base, "sqlite"),
        runtime_groups: "1000",
        runner_uid: "1000",
        runner_gid: "1000",
        selected_status: "0",
        pending_status: "0",
        LEAF: String(leaf),
        OWNER_EXIT: String(ownerExit),
        MARKS: marks,
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    return {
      exitCode: result.exitCode,
      safe,
      marks: existsSync(marks) ? readFileSync(marks, "utf8").split("\n").filter(Boolean) : [],
    };
  }
  test("a failed list skips the launcher, keeps its error and still gates ownership", () => {
    const { exitCode, safe, marks } = gate(7, 0);
    expect(exitCode).toBe(7);
    expect(marks).toEqual(["list", "owner"]);
    expect(read(join(safe, "launcher-stage.json"))).toMatchObject({
      actual_launcher_exit: null,
      config_list_exit: 7,
      selected_final_exit: 7,
    });
    expect(readFileSync(join(safe, "config-list.stderr.log"), "utf8")).toBe(
      "ORIGINAL_LOAD_ERROR\n",
    );
    for (const name of ["config-list.stdout.log", "config-list.stderr.log"])
      expect(statSync(join(safe, name)).mode & 0o777).toBe(0o600);
  });
  test("a listed config still runs the launcher; an owner failure keeps the captures", () => {
    const { exitCode, safe, marks } = gate(0, 1);
    expect(exitCode).toBe(1);
    expect(marks).toEqual(["list", "run", "owner"]);
    expect(read(join(safe, "launcher-stage.json"))).toMatchObject({
      actual_launcher_exit: 0,
      config_list_exit: 0,
    });
    for (const name of ["config-list.stdout.log", "config-list.stderr.log"])
      expect(statSync(join(safe, name)).isFile()).toBe(true);
  });
  for (const occupied of ["stdout", "stderr"] as const)
    test(`an occupied ${occupied} capture refuses without clobber or launcher`, () => {
      const { exitCode, safe, marks } = gate(0, 0, occupied);
      expect(exitCode).not.toBe(0);
      expect(marks).toEqual(["owner"]);
      expect(readFileSync(join(safe, `config-list.${occupied}.log`), "utf8")).toBe("OLD_CAPTURE");
    });
});
