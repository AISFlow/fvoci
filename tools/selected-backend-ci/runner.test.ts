import { spawn, spawnSync } from "bun";
import { afterEach, describe, expect, test } from "bun:test";
import { strict as assert } from "node:assert";
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
import { main, modes, parseCLI } from "../../scripts/run-selected-backend-e2e.ts";
import {
  admittedBrowser,
  allocationExpiry,
  browserInventory,
  configListInputs,
  expectedFiles,
  identity,
  localAllocation,
} from "./admission.ts";
import { buildEnv, elfDependencies, qualifyArtifacts, stage } from "./build.ts";
import { qualifyListing } from "./config-list.ts";
import type { Listing } from "./config-list.ts";
import {
  call,
  digest,
  gid,
  jsonInteger,
  observedExit,
  read,
  root,
  sha,
  sourceInputText,
  spawnSelectedCommand,
  templates,
  tool,
  uid,
  write,
} from "./io.ts";
import {
  driverCommand,
  knownOnBrowserTest,
  laneRetirement,
  ownershipReturn,
  prepareBrowser,
  publicFailureFields,
  run,
  selectedRuns,
} from "./runtime.ts";
import type { RunBoundary } from "./runtime.ts";
import type {
  Aggregate,
  Artifact,
  Browser,
  Bundle,
  DriverReceipt,
  Flow,
  Inputs,
  Lane,
  LocalGrant,
  Reference,
} from "./types.ts";

const temporary: string[] = [];
function directory(): string {
  const path = mkdtempSync(join(tmpdir(), "selected-backend-ts-"));
  chmodSync(path, 0o700);
  temporary.push(path);
  return path;
}
afterEach(() => {
  for (const path of temporary.splice(0)) rmSync(path, { recursive: true });
});
const source = call(["git", "rev-parse", "HEAD"]),
  tree = call(["git", "rev-parse", "HEAD^{tree}"]);
const owner = "github:fixture/repo:123:1:collaboration-flow";
const ci = {
  CI: "true",
  GITHUB_ACTIONS: "true",
  GITHUB_SHA: source,
  GITHUB_REPOSITORY: "fixture/repo",
  GITHUB_RUN_ID: "123",
  GITHUB_RUN_ATTEMPT: "1",
  GITHUB_JOB: "collaboration-flow",
};
async function withEnvironment<T>(
  values: Record<string, string | undefined>,
  body: () => T | Promise<T>,
): Promise<T> {
  const old = { ...process.env };
  for (const key of Object.keys(process.env))
    if (key.startsWith("GITHUB_") || key.startsWith("FVOCI_") || key === "CI")
      Reflect.deleteProperty(process.env, key);
  for (const [key, value] of Object.entries(values)) {
    if (value === undefined) Reflect.deleteProperty(process.env, key);
    else process.env[key] = value;
  }
  try {
    return await body();
  } finally {
    for (const key of Object.keys(process.env))
      if (!(key in old)) Reflect.deleteProperty(process.env, key);
    Object.assign(process.env, old);
  }
}
function receipt(lane: Lane, flow: Flow, exit = 0): DriverReceipt {
  return {
    source,
    tree,
    root_owner: owner,
    final_exit_code: exit,
    owned_container_absent: true,
    ...(lane === "install"
      ? { actual_tests: 4, actual_owned_process_receipts: 15 }
      : {
          selected_flow: flow,
          cleanup_errors: [],
          owned_loopback_port_closed: true,
          recorded_process_identities_retired: true,
          current_schema_server_restart: { restartBrowserExit: 0 },
          actual_browser_tests: flow === "off" ? 8 : 1,
          retries: 0,
        }),
  };
}
function retirementFiles(path: string, lane: Lane, flow: Flow, exit = 0): void {
  mkdirSync(path, { mode: 0o700 });
  write(join(path, "receipt.json"), receipt(lane, flow, exit));
  if (lane === "postgres")
    write(join(path, "parent-receipt.json"), {
      source,
      tree,
      root_owner: owner,
      selected_flow: flow,
      all_owned_fixtures_closed: true,
    });
}
function cohort() {
  const output = directory();
  const before: Inputs = {
    head: source,
    tree,
    status: "",
    tracked: { "한글😀.ts": "1".repeat(64) },
    external: {},
    untracked: {},
  };
  const browser: Browser = {
    bun: { path: process.execPath, sha256: sha(process.execPath) },
    chromium: { path: process.execPath, sha256: sha(process.execPath) },
    chromium_directory_files: {},
  };
  for (const name of ["before.json", "after.json"]) write(join(output, name), before);
  write(join(output, "bundle.json"), { source, tree, binaries: {}, compiler_artifacts: [] });
  write(join(output, "compile-receipt.json"), { source, tree });
  write(join(output, "web-receipt.json"), {
    source,
    tree,
    dist_files: { "index.html": "2".repeat(64) },
  });
  write(join(output, "abi-receipt.json"), { currentSource: source, host_runtime_files: {} });
  const boundary: RunBoundary = {
    identity: () => owner,
    browser: () => browser,
    access: () => {
      return;
    },
    execute(driver, environment, log) {
      const allocation = read(environment.FVOCI_ROOT_CURRENT_ALLOCATION as string) as {
        runRoot: string;
        lane: Lane;
        flow: Flow;
      };
      expect(driver).toBe(join(templates, "current-" + allocation.lane + "-driver.py"));
      retirementFiles(allocation.runRoot, allocation.lane, allocation.flow);
      writeFileSync(log, "fixture only\n");
      return 0;
    },
  };
  return { output, before, browser, boundary };
}

describe.serial("selected runner contract and fail-closed controls", () => {
  test("original five lane tuple and all seven CLI modes remain exact", () => {
    const original = readFileSync(join(root, "scripts/run-selected-backend-e2e.py"), "utf8");
    expect(original).toContain(
      "return (('install','on'),('postgres','on'),('sqlite','on'),('postgres','off'),('sqlite','off'))",
    );
    expect(original).toContain(
      "subprocess.run([sys.executable,str(driver)],env=env,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT)",
    );
    expect(selectedRuns).toEqual([
      ["install", "on"],
      ["postgres", "on"],
      ["sqlite", "on"],
      ["postgres", "off"],
      ["sqlite", "off"],
    ]);
    expect(modes).toEqual([
      "record-before",
      "stage",
      "record-after",
      "run",
      "permissions",
      "owner-return",
      "config-list",
    ]);
    for (const [lane] of selectedRuns)
      expect(driverCommand(join(templates, "current-" + lane + "-driver.py"))).toEqual([
        tool("python3"),
        join(templates, "current-" + lane + "-driver.py"),
      ]);
    const caller = readFileSync(join(root, "scripts/run-web-e2e.sh"), "utf8");
    for (const mode of modes.filter((value) => value !== "stage"))
      expect(caller).toContain('scripts/run-selected-backend-e2e.py" ' + mode);
    expect(caller.match(/scripts\/run-selected-backend-e2e\.py" stage/g)).toHaveLength(4);
    expect(caller).not.toContain("run-selected-backend-e2e.ts");
  });

  for (const mode of modes)
    test("parse existing caller: " + mode, () => {
      const suffix =
        mode === "permissions"
          ? ["--sqlite-parent", "/fixture/sqlite", "--docker-gid", "1000"]
          : mode === "stage"
            ? [
                "--stage-name",
                "main",
                "--",
                "cargo",
                "build",
                "--locked",
                "--message-format=json-render-diagnostics",
              ]
            : [];
      const parsed = parseCLI([mode, "--output", "/fixture/output", ...suffix]);
      expect(parsed?.mode).toBe(mode);
      expect(parsed?.output).toBe("/fixture/output");
      expect(parsed?.command).toEqual(mode === "stage" ? suffix.slice(3) : []);
    });
  test("compiler argv keeps spaces, empty args, equals, switch order and trailing LF", () => {
    const args = ["cargo", "--features", "api-schema,db-tests", "", "a b", "--flag=x", "line\n"];
    expect(
      parseCLI(["stage", "--output=/fixture/output", "--stage-name=main", "--", ...args])?.command,
    ).toEqual(args);
  });
  for (const args of [
    [],
    ["unknown", "--output", "/fixture"],
    ["run"],
    ["run", "--output"],
    ["permissions", "--output", "/fixture", "--docker-gid", "1.5"],
  ])
    test("parser refuses malformed invocation " + JSON.stringify(args), () => {
      expect(() => parseCLI(args)).toThrow();
    });
  for (const args of [
    ["--help"],
    [],
    ["unknown"],
    ["run"],
    ["run", "--output"],
    ["permissions", "--output", "/fixture", "--docker-gid", "nan"],
  ])
    test("actual Bun CLI versus original Python exit: " + JSON.stringify(args), () => {
      const ts = spawnSync(
        [process.execPath, join(root, "scripts/run-selected-backend-e2e.ts"), ...args],
        { stdout: "pipe", stderr: "pipe" },
      );
      const py = spawnSync(
        [tool("python3"), join(root, "scripts/run-selected-backend-e2e.py"), ...args],
        { stdout: "pipe", stderr: "pipe", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      expect(ts.exitCode).toBe(py.exitCode);
      expect(ts.exitCode).toBe(args.includes("--help") ? 0 : 2);
    });
  for (const mode of modes)
    test("actual CLI refuses unallocated " + mode + " without starting resources", () => {
      const output = directory();
      const result = spawnSync(
        [
          process.execPath,
          join(root, "scripts/run-selected-backend-e2e.ts"),
          mode,
          "--output",
          output,
        ],
        { stdout: "pipe", stderr: "pipe", env: { PATH: process.env.PATH } },
      );
      expect(result.exitCode).toBe(1);
      expect(readdirSync(output)).toEqual([]);
    });
  test("private exclusive JSON writes never replace an occupied file or symlink", () => {
    const output = directory(),
      path = join(output, "receipt.json");
    write(path, { first: true });
    expect(statSync(path).mode & 0o777).toBe(0o600);
    expect(() => {
      write(path, { second: true });
    }).toThrow();
    const link = join(output, "link.json");
    symlinkSync(path, link);
    expect(() => {
      write(link, {});
    }).toThrow();
    expect(read(path)).toEqual({ first: true });
  });
  test("restart source-input byte form preserves Python ASCII JSON contract", () => {
    const text = sourceInputText({
      head: "a",
      tree: "b",
      status: "",
      tracked: { "한글😀.ts": "x" },
      external: {},
      untracked: {},
    });
    expect(text).toBe(
      '{\n  "head": "a",\n  "tree": "b",\n  "status": "",\n  "tracked": {\n    "\\ud55c\\uae00\\ud83d\\ude00.ts": "x"\n  },\n  "external": {},\n  "untracked": {}\n}\n',
    );
  });
  test("signal exit retains Python subprocess's negative sign", () => {
    expect(observedExit({ exitCode: 143, signalCode: "SIGTERM" })).toBe(-15);
    expect(observedExit({ exitCode: 7 })).toBe(7);
    expect(() => observedExit({ exitCode: 1, signalCode: "INVALID" })).toThrow();
  });

  test("all five mocked driver outcomes produce exact binding/grant/aggregate contracts", async () => {
    const { output, before, boundary } = cohort(),
      calls: { lane: Lane; flow: Flow; environment: Record<string, string | undefined> }[] = [];
    const execute = boundary.execute;
    boundary.execute = (driver, environment, log, signal) => {
      const grant = read(environment.FVOCI_ROOT_CURRENT_ALLOCATION as string) as {
        runRoot: string;
        lane: Lane;
        flow: Flow;
        bindingSha256: string;
        driverSha256: string;
        bindingModuleSha256: string;
      };
      const manifestPath = environment.FVOCI_ROOT_CURRENT_BINDING as string;
      const manifest = read(manifestPath) as {
        flow: Flow;
        closedInstallReceipt: Reference | null;
        restartAllocation?: Reference;
        sourceInputsBefore: Reference;
        sourceInputsAfter: Reference;
      };
      expect(grant.bindingSha256).toBe(sha(manifestPath));
      expect(grant.driverSha256).toBe(sha(driver));
      expect(grant.bindingModuleSha256).toBe(sha(join(templates, "current_binding.py")));
      expect(manifest.sourceInputsBefore.sha256).toBe(sha(join(output, "before.json")));
      expect(manifest.sourceInputsAfter.sha256).toBe(sha(join(output, "after.json")));
      expect(grant.runRoot).toMatch(new RegExp("/root-current-" + grant.lane + "-[0-9a-f]{12}$"));
      expect(environment.FVOCI_E2E_SELECTED_AUXILIARY).toBe(
        grant.lane === "postgres" && grant.flow === "on" ? "normal-api" : undefined,
      );
      if (grant.lane !== "install") {
        expect(manifest.closedInstallReceipt).not.toBeNull();
        expect(manifest.closedInstallReceipt?.sha256).toBe(
          sha(manifest.closedInstallReceipt?.path as string),
        );
        expect(
          (read(manifest.closedInstallReceipt?.path as string) as DriverReceipt).actual_tests,
        ).toBe(4);
      } else expect(manifest.closedInstallReceipt).toBeNull();
      if (grant.lane !== "install" && grant.flow === "on") {
        expect(manifest.restartAllocation?.path).toBe(environment.FVOCI_ROOT_RESTART_GRANT);
        const restart = read(manifest.restartAllocation?.path as string) as {
          binding: {
            sourceInputsSha256: string;
            parentDriverSha256: string;
            restartHelperSha256: string;
            backend: Lane;
            runRoot: string;
          };
        };
        expect(restart.binding.sourceInputsSha256).toBe(digest(sourceInputText(before)));
        expect(restart.binding.parentDriverSha256).toBe(sha(driver));
        expect(restart.binding.restartHelperSha256).toBe(
          sha(join(templates, "restart_checkpoint.py")),
        );
        expect(restart.binding.backend).toBe(grant.lane);
        expect(restart.binding.runRoot).toBe(grant.runRoot);
      } else {
        expect(manifest.restartAllocation).toBeUndefined();
        expect(environment.FVOCI_ROOT_RESTART_GRANT).toBeUndefined();
      }
      calls.push({ lane: grant.lane, flow: grant.flow, environment: { ...environment } });
      return execute(driver, environment, log, signal);
    };
    await withEnvironment(ci, async () => {
      expect(await run(output, boundary)).toBe(0);
    });
    expect(calls.map((r) => [r.lane, r.flow])).toEqual(
      selectedRuns.map(([lane, flow]) => [lane, flow]),
    );
    const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate & {
      whole060Complete: boolean;
      offTestsPerBackend: number;
    };
    expect(aggregate.exit).toBe(0);
    expect(aggregate.allRequestedRunsExecuted).toBe(true);
    expect(aggregate.whole060Complete).toBe(false);
    expect(aggregate.offTestsPerBackend).toBe(8);
    expect(new Set(aggregate.runs.map((r) => r.runRoot)).size).toBe(5);
    expect(aggregate.runs.every((r) => r.retirement?.qualified)).toBe(true);
    for (const path of readdirSync(output).filter((p) => p.endsWith(".json")))
      expect(statSync(join(output, path)).mode & 0o777).toBe(0o600);
  });
  for (let failed = 0; failed < 5; failed++)
    for (const failure of ["exit", "retirement"] as const)
      test(`serial stop at lane ${String(failed + 1)}: ${failure}`, async () => {
        const { output, boundary } = cohort();
        let count = 0;
        boundary.execute = (_driver, environment) => {
          const grant = read(environment.FVOCI_ROOT_CURRENT_ALLOCATION as string) as {
            runRoot: string;
            lane: Lane;
            flow: Flow;
          };
          const current = count++;
          const code = current === failed && failure === "exit" ? 7 : 0;
          retirementFiles(grant.runRoot, grant.lane, grant.flow, code);
          if (current === failed && failure === "retirement")
            rmSync(join(grant.runRoot, "receipt.json"));
          return code;
        };
        await withEnvironment(ci, async () => {
          expect(await run(output, boundary)).toBe(failure === "exit" ? 7 : 1);
        });
        const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate;
        expect(count).toBe(failed + 1);
        expect(aggregate.runs).toHaveLength(failed + 1);
        expect(aggregate.allRequestedRunsExecuted).toBe(failed === 4);
        expect(aggregate.runs[failed]?.exit).toBe(failure === "exit" ? 7 : 0);
        expect(aggregate.runs[failed]?.retirement?.qualified).toBe(failure === "exit");
      });
  test("launcher failure preserves private original and refuses partial completion", async () => {
    const { output, boundary } = cohort();
    boundary.execute = () => {
      throw new Error("private fixture outcome");
    };
    await withEnvironment(ci, async () => {
      expect(await run(output, boundary)).toBe(1);
    });
    const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate;
    expect(aggregate.runs).toHaveLength(0);
    expect(aggregate.allRequestedRunsExecuted).toBe(false);
    expect(statSync(join(output, "selected-launcher-failure.private.json")).mode & 0o777).toBe(
      0o600,
    );
    expect(aggregate.launcherFailure).toMatchObject({
      code: "SELECTED_LAUNCHER_FAILED",
      receiptWrite: "confirmed",
    });
  });
  test("SIGINT retains exit130 and records interrupted partial outcome", async () => {
    const { output, boundary } = cohort();
    boundary.execute = (_driver, _environment, _log, signal) => {
      process.emit("SIGINT");
      expect(signal.aborted).toBe(true);
      throw new Error("interrupted fixture");
    };
    await withEnvironment(ci, async () => {
      expect(await run(output, boundary)).toBe(130);
    });
    expect(read(join(output, "selected-ci-receipt.json")) as Aggregate).toMatchObject({
      exit: 130,
      allRequestedRunsExecuted: false,
      runs: [],
    });
  });
  test("aggregate write failure cannot convert success to zero", async () => {
    const { output, boundary } = cohort();
    write(join(output, "selected-ci-receipt.json"), { occupied: true });
    await withEnvironment(ci, async () => {
      expect(await run(output, boundary)).toBe(1);
    });
    expect(read(join(output, "selected-ci-receipt.json"))).toEqual({ occupied: true });
  });

  for (const [lane, flow] of selectedRuns)
    test(`retirement rejects each missing/invalid proof: ${lane}/${flow}`, () => {
      const parent = directory(),
        path = join(parent, "run");
      retirementFiles(path, lane, flow);
      expect(laneRetirement(path, lane, flow, source, tree, owner, 0).qualified).toBe(true);
      const base = receipt(lane, flow);
      const mandatory =
        lane === "install"
          ? [
              "source",
              "tree",
              "root_owner",
              "final_exit_code",
              "owned_container_absent",
              "actual_tests",
              "actual_owned_process_receipts",
            ]
          : [
              "source",
              "tree",
              "root_owner",
              "final_exit_code",
              "owned_container_absent",
              "selected_flow",
              "cleanup_errors",
              "owned_loopback_port_closed",
              "recorded_process_identities_retired",
              "actual_browser_tests",
              "retries",
              ...(flow === "on" ? ["current_schema_server_restart"] : []),
            ];
      for (const key of mandatory)
        for (const value of [undefined, "invalid", false, null]) {
          const changed = { ...base };
          if (value === undefined) Reflect.deleteProperty(changed, key);
          else changed[key] = value;
          writeFileSync(join(path, "receipt.json"), JSON.stringify(changed));
          expect(laneRetirement(path, lane, flow, source, tree, owner, 0).qualified).toBe(false);
        }
      if (lane === "postgres") {
        writeFileSync(join(path, "receipt.json"), JSON.stringify(base));
        rmSync(join(path, "parent-receipt.json"));
        expect(laneRetirement(path, lane, flow, source, tree, owner, 0).qualified).toBe(false);
      }
    });
  test("failure summaries reveal only known phase/code/status/file:line", () => {
    const facts: DriverReceipt = {
      failed_phase: "browser",
      failure_code: "SELECTED_BODY_NONZERO",
      original_driver_failure: { type: "ReturnedNonzero", message: "private" },
      browser_report_state: "matched",
      known_browser_test: knownOnBrowserTest,
      known_browser_status: "failed",
      known_browser_checkpoint: "e2e-pending/workspace-wiki-selected-backend.spec.ts:413",
    };
    expect(publicFailureFields(facts)).toMatchObject({
      failed_phase: "browser",
      known_browser_checkpoint: facts.known_browser_checkpoint,
      original_driver_failure_type: "ReturnedNonzero",
    });
    expect(JSON.stringify(publicFailureFields(facts))).not.toContain("private");
    for (const checkpoint of [
      "/private/file:1",
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:0",
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:10001",
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:12\nprivate",
    ])
      expect(
        publicFailureFields({ ...facts, known_browser_checkpoint: checkpoint })
          .known_browser_checkpoint,
      ).toBeNull();
    expect(
      publicFailureFields({ ...facts, browser_report_state: "spec-mismatch" }).known_browser_test,
    ).toBeNull();
  });

  test("owner-return accepts no-start and complete retained process proofs", async () => {
    await withEnvironment(ci, async () => {
      const first = directory();
      write(join(first, "before.json"), { head: source, tree });
      ownershipReturn(first);
      expect(read(join(first, "runtime-close-stage.json"))).toMatchObject({
        no_runtime_started: true,
        ownership_return_qualified: true,
      });
      const { output, boundary } = cohort();
      expect(await run(output, boundary)).toBe(0);
      const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate;
      const install = aggregate.runs[0];
      expect(install).toBeDefined();
      const retained = join(install?.runRoot as string, "retained-run");
      mkdirSync(retained);
      for (let index = 0; index < 15; index++)
        write(join(retained, String(index) + "-process.json"), { status: 0 });
      ownershipReturn(output);
      expect(read(join(output, "runtime-close-stage.json"))).toMatchObject({
        ownership_return_qualified: true,
        installation_process_receipts: 15,
      });
    });
  });
  for (const mutation of [
    "missing-process",
    "status-null",
    "duplicate-root",
    "missing-lane",
    "live-port",
    "bad-owner",
  ] as const)
    test("owner-return refuses " + mutation, async () => {
      await withEnvironment(ci, async () => {
        const { output, boundary } = cohort();
        expect(await run(output, boundary)).toBe(0);
        const path = join(output, "selected-ci-receipt.json"),
          aggregate = read(path) as Aggregate,
          install = aggregate.runs[0];
        expect(install).toBeDefined();
        const retained = join(install?.runRoot as string, "retained-run");
        mkdirSync(retained);
        for (let index = 0; index < (mutation === "missing-process" ? 14 : 15); index++)
          write(join(retained, String(index) + "-process.json"), {
            status: mutation === "status-null" ? null : 0,
          });
        if (mutation === "duplicate-root")
          aggregate.runs[1] = aggregate.runs[0] as NonNullable<typeof install>;
        if (mutation === "missing-lane") aggregate.runs.pop();
        if (mutation === "bad-owner") aggregate.owner = "foreign";
        if (mutation === "live-port") {
          const lane = aggregate.runs[1];
          const receiptPath = join(lane?.runRoot as string, "receipt.json");
          const facts = read(receiptPath) as DriverReceipt;
          facts.owned_loopback_port_closed = false;
          writeFileSync(receiptPath, JSON.stringify(facts));
        }
        writeFileSync(path, JSON.stringify(aggregate));
        expect(() => {
          ownershipReturn(output);
        }).toThrow();
        expect(existsSync(join(output, "runtime-close-stage.json"))).toBe(false);
      });
    });

  for (const change of [
    "CI",
    "GITHUB_JOB",
    "GITHUB_SHA",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_ATTEMPT",
    "FVOCI_WEB_BUILD_PHASE",
  ])
    test("CI identity rejects forged " + change, async () => {
      await withEnvironment({ ...ci, [change]: "invalid" }, () => {
        expect(() => identity("run")).toThrow();
      });
    });
  test("producer admits only its four preparation modes", async () => {
    await withEnvironment(
      { ...ci, GITHUB_JOB: "collaboration-build", FVOCI_WEB_BUILD_PHASE: "prepare" },
      () => {
        for (const mode of ["record-before", "stage", "record-after", "handoff"])
          expect(identity(mode)).toEndWith(":collaboration-build");
        for (const mode of ["run", "permissions", "owner-return", "config-list"])
          expect(() => identity(mode)).toThrow();
      },
    );
  });
  for (const mutation of [
    "none",
    "expired",
    "foreign-source",
    "foreign-tree",
    "bad-registration",
    "extra-mode",
    "same-terminal",
    "bad-task",
    "bad-owner",
    "bad-mode",
    "public-file",
    "bad-digest",
    "ci-leak",
  ] as const)
    test("local allocation " + mutation, async () => {
      const output = directory(),
        path = join(output, "local.json");
      const registrationHashes = Object.fromEntries(
        [
          "run-selected-backend-e2e.py",
          "selected-backend-ci/current_binding.py",
          "selected-backend-ci/restart_checkpoint.py",
          "selected-backend-ci/current-install-driver.py",
          "selected-backend-ci/current-postgres-driver.py",
          "selected-backend-ci/current-sqlite-driver.py",
        ].map((name) => [name, sha(join(root, "scripts", name))]),
      );
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
        uid: 1000,
        gid: 1000,
        source,
        tree,
        allowedModes: ["record-before", "stage", "record-after", "run"],
        expiresUtc: new Date(Date.now() + 3600000).toISOString(),
        outputRoot: output,
        registrationHashes,
        stageCommands: {},
      };
      if (mutation === "expired") grant.expiresUtc = "2000-01-01T00:00:00Z";
      if (mutation === "foreign-source") grant.source = "0".repeat(40);
      if (mutation === "foreign-tree") grant.tree = "0".repeat(40);
      if (mutation === "bad-registration")
        grant.registrationHashes["run-selected-backend-e2e.py"] = "0".repeat(64);
      if (mutation === "extra-mode") grant.allowedModes.push("permissions");
      if (mutation === "same-terminal") grant.workerTerminal = grant.rootTerminal;
      if (mutation === "bad-task") grant.taskId = "other";
      if (mutation === "bad-owner") grant.owner = "foreign";
      write(path, grant);
      if (mutation === "public-file") chmodSync(path, 0o644);
      const values = {
        FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
        FVOCI_SELECTED_LOCAL_ALLOCATION: path,
        FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256:
          mutation === "bad-digest" ? "0".repeat(64) : sha(path),
        FVOCI_CI_OWNER: "orca:fixture",
        FVOCI_ROOT_RUN_OWNER: "orca:fixture",
        FVOCI_LOCAL_RUN_ID: "run_ab12",
        FVOCI_LOCAL_DISPATCH_ID: "ctx_cd34",
        FVOCI_LOCAL_TASK_ID: "task_ef56",
        ORCA_TERMINAL_HANDLE: "fixture-worker",
        FVOCI_LOCAL_ROOT_TERMINAL: "fixture-root",
        FVOCI_CI_SELECTED_RUNS: join(output, "runtime"),
        ...(mutation === "ci-leak" ? { CI: "true" } : {}),
      };
      await withEnvironment(values, () => {
        if (mutation === "none") expect(localAllocation("run")).toEqual(grant);
        else
          expect(() => localAllocation(mutation === "bad-mode" ? "owner-return" : "run")).toThrow();
      });
    });

  test("browser staging preserves source bytes/modes and rejects symlink/owner/byte/mode changes", async () => {
    const cache = directory(),
      component = join(cache, "chromium-123"),
      binaryDir = join(component, "chrome-linux64");
    mkdirSync(binaryDir, { recursive: true, mode: 0o700 });
    const chromium = join(binaryDir, "chrome");
    writeFileSync(chromium, "fixture", { mode: 0o700 });
    const original = browserInventory(component, [uid(), gid()], true),
      output = directory();
    await withEnvironment({ ...ci, PLAYWRIGHT_BROWSERS_PATH: join(output, "browser") }, () => {
      const copied = prepareBrowser(output, chromium);
      expect(browserInventory(component, [uid(), gid()], true)).toEqual(original);
      expect(admittedBrowser(output, [uid(), gid()])).toBe(copied);
      chmodSync(copied, 0o755);
      expect(() => admittedBrowser(output, [uid(), gid()])).toThrow();
      chmodSync(copied, 0o700);
      writeFileSync(copied, "changed");
      expect(() => admittedBrowser(output, [uid(), gid()])).toThrow();
      writeFileSync(copied, "fixture");
      expect(() => admittedBrowser(output, [uid() + 1, gid()])).toThrow();
      symlinkSync(chromium, join(binaryDir, "linked"));
      expect(() => browserInventory(component)).toThrow();
    });
  });
  test("config-list cannot bypass consumed phase or allocate any lane", async () => {
    await withEnvironment(ci, () => {
      expect(() => configListInputs(directory())).toThrow();
    });
    await withEnvironment({ ...ci, FVOCI_WEB_BUILD_PHASE: "consume" }, () => {
      const output = directory();
      mkdirSync(join(output, "runtime"));
      expect(() => configListInputs(output)).toThrow();
    });
  });
  const listing: Listing = {
    config: { workers: 1, metadata: { selectedBackend: "postgres", selectedFlow: "on" } },
    errors: [],
    stats: { expected: 0, unexpected: 0, flaky: 0, skipped: 1 },
    suites: [{ specs: [{ title: knownOnBrowserTest, tests: [{ results: [] }] }] }],
  };
  test("config list accepts discovery with zero actual bodies", () => {
    qualifyListing(listing);
  });
  for (const field of [
    "workers",
    "backend",
    "errors",
    "expected",
    "unexpected",
    "flaky",
    "skipped",
    "title",
    "results",
    "suite-count",
  ] as const)
    test("config list refuses " + field, () => {
      const changed = structuredClone(listing);
      if (field === "workers") changed.config.workers = 2;
      else if (field === "backend")
        changed.config.metadata = { selectedBackend: "sqlite", selectedFlow: "on" };
      else if (field === "errors") changed.errors = [{}];
      else if (["expected", "unexpected", "flaky", "skipped"].includes(field))
        changed.stats[field as keyof Listing["stats"]] = field === "skipped" ? 0 : 1;
      else if (field === "title")
        (changed.suites[0]?.specs[0] as Listing["suites"][number]["specs"][number]).title = "other";
      else if (field === "results")
        (changed.suites[0]?.specs[0]?.tests[0] as { results: unknown[] }).results = [{}];
      else changed.suites = [];
      expect(() => {
        qualifyListing(changed);
      }).toThrow();
    });

  test("six emitted artifacts, exact features and immutable destinations", () => {
    const output = directory(),
      artifacts: Artifact[] = [
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
          features: name === "collab-engine" ? ["default", "worker"] : ["api-schema", "db-tests"],
          fresh: false,
          filenames: [path],
        };
      });
    const before: Inputs = {
      head: source,
      tree,
      status: "",
      tracked: {},
      external: {},
      untracked: {},
    };
    const binaries = qualifyArtifacts(artifacts, before);
    expect(Object.keys(binaries)).toHaveLength(6);
    const core: Artifact = {
      reason: "compiler-artifact",
      executable: null,
      target: { name: "fvoci_server" },
      profile: { test: false },
      features: ["api-schema", "db-tests"],
      fresh: false,
      filenames: [join(output, "core.rlib"), join(output, "core.rmeta")],
    };
    const bundle: Bundle = {
      source,
      tree,
      full_inputs_unchanged: true,
      binaries,
      compiler_artifacts: [...artifacts, core],
    };
    expect(expectedFiles(bundle, output)).toHaveLength(24);
    for (const index of [0, 1, 2, 3, 4, 5]) {
      const changed = structuredClone(artifacts);
      (changed[index] as Artifact).features.push("unqualified");
      expect(() => qualifyArtifacts(changed, before)).toThrow();
      expect(() =>
        qualifyArtifacts(
          artifacts.filter((_, i) => i !== index),
          before,
        ),
      ).toThrow();
    }
    const duplicate = structuredClone(artifacts);
    const item = duplicate[0] as Artifact;
    item.executable = artifacts[1]?.executable ?? null;
    expect(() => qualifyArtifacts([...artifacts, item], before)).toThrow();
    expect(() => expectedFiles({ ...bundle, compiler_artifacts: artifacts }, output)).toThrow();
  });
  test("ldd permits known static outcomes and fails closed on unexpected exits", () => {
    expect(elfDependencies(1, "not a dynamic executable", "")).toEqual([]);
    expect(elfDependencies(1, "", "statically linked")).toEqual([]);
    for (const code of [1, 2, 127, -15])
      expect(() => elfDependencies(code, "unexpected", "")).toThrow();
  });
  test("compiler environment contains hashes only and rejects wrappers/accounts", async () => {
    await withEnvironment(
      { CARGO_BUILD_JOBS: "2", RUSTC_WRAPPER: "", RUSTC_WORKSPACE_WRAPPER: "" },
      () => {
        expect(buildEnv().CARGO_BUILD_JOBS).toBe(digest("2"));
        expect(Object.values(buildEnv()).every((v) => /^[0-9a-f]{64}$/.test(v))).toBe(true);
      },
    );
    for (const values of [
      { RUSTC_WRAPPER: "wrapper" },
      { RUSTC_WORKSPACE_WRAPPER: "wrapper" },
      { CARGO_TOKEN: "account" },
    ])
      await withEnvironment(values, () => {
        expect(() => buildEnv()).toThrow();
      });
  });
  for (const exit of [0, 7])
    test(
      "actual TS stage invokes harmless Bun command and preserves exit " + String(exit),
      async () => {
        const output = directory();
        write(join(output, "before.json"), { head: source, tree });
        await withEnvironment(ci, async () => {
          const command = [
            process.execPath,
            "--eval",
            `console.log('compiler-fixture'); process.exit(${String(exit)});`,
          ];
          expect(await stage(output, "main", command)).toBe(exit);
          expect(read(join(output, "main-stage.json"))).toMatchObject({
            source,
            tree,
            command,
            exit_code: exit,
          });
          expect(readFileSync(join(output, "main-compiler.jsonl"), "utf8")).toBe(
            "compiler-fixture\n",
          );
        });
      },
    );
  for (const exit of [0, 7])
    test("real stage CLI comparison against unchanged Python: " + String(exit), () => {
      const command = [
        process.execPath,
        "--eval",
        `console.log('compiler-fixture'); process.exit(${String(exit)});`,
      ];
      const receipts: unknown[] = [];
      for (const [executable, runner] of [
        [process.execPath, "run-selected-backend-e2e.ts"],
        [tool("python3"), "run-selected-backend-e2e.py"],
      ]) {
        const output = directory();
        write(join(output, "before.json"), { head: source, tree });
        const result = spawnSync(
          [
            executable as string,
            join(root, "scripts", runner as string),
            "stage",
            "--output",
            output,
            "--stage-name",
            "main",
            "--",
            ...command,
          ],
          {
            cwd: root,
            env: { ...process.env, ...ci, PYTHONDONTWRITEBYTECODE: "1" },
            stdout: "pipe",
            stderr: "pipe",
          },
        );
        expect(result.exitCode).toBe(exit);
        const receipt = read(join(output, "main-stage.json")) as {
          seconds: number;
          [key: string]: unknown;
        };
        expect(receipt.seconds).toBeGreaterThanOrEqual(0);
        const { seconds: elapsed, ...contract } = receipt;
        expect(Number.isFinite(elapsed)).toBe(true);
        receipts.push(contract);
        expect(readFileSync(join(output, "main-compiler.jsonl"), "utf8")).toBe(
          "compiler-fixture\n",
        );
      }
      expect(receipts[0]).toEqual(receipts[1]);
    });
  test("main rejects broad permission arguments and nonphysical config output", async () => {
    const output = directory();
    await assert.rejects(main(["run", "--output", output, "--docker-gid", "1000"]), Error);
    await assert.rejects(main(["config-list", "--output", "relative"]), Error);
  });
});

describe.serial("task4 counterexamples and real child cancellation", () => {
  for (const expiry of [
    "2099-12-31T00:00:00",
    "2099-12-31",
    "2099",
    "Dec 31 2099",
    "Thu, 31 Dec 2099 00:00:00 GMT",
    "2099-02-30T00:00:00Z",
    "2099-02-29T00:00:00Z",
    "2099-12-31T24:00:00Z",
    "0000-12-31T00:00:00Z",
    "2099-12-31T00:00:00+24:00",
  ])
    test("B1 rejects non-timezone ISO expiry: " + expiry, () => {
      expect(() => allocationExpiry(expiry)).toThrow();
    });
  for (const expiry of [
    "2099-12-31T00:00:00Z",
    "2099-12-31T01:00:00+01:00",
    "2099-12-30T19:00:00-05:00",
  ])
    test("B1 accepts equivalent timezone ISO expiry: " + expiry, () => {
      expect(allocationExpiry(expiry)).toBe(4102358400000);
    });
  test("B1 accepts a valid leap day and Python microsecond precision", () => {
    expect(allocationExpiry("2096-02-29T00:00:00.123456Z")).toBe(
      Date.UTC(2096, 1, 29, 0, 0, 0, 123),
    );
  });

  for (const [lane, flow] of selectedRuns)
    for (const token of ["0.0", "0e0", "0E0"])
      test(`B2 retirement rejects original float token: ${lane}/${flow}/${token}`, () => {
        const path = join(directory(), "run");
        retirementFiles(path, lane, flow);
        expect(laneRetirement(path, lane, flow, source, tree, owner, 0).qualified).toBe(true);
        writeFileSync(
          join(path, "receipt.json"),
          JSON.stringify(receipt(lane, flow)).replace(
            '"final_exit_code":0',
            '"final_exit_code":' + token,
          ),
        );
        expect(laneRetirement(path, lane, flow, source, tree, owner, 0).qualified).toBe(false);
      });
  for (const field of ["final_exit_code", "actual_owned_process_receipts"])
    for (const suffix of [".0", "e0", "E0"])
      test(`B2 owner-return rejects original float proof: ${field}/${suffix}`, async () => {
        await withEnvironment(ci, async () => {
          const { output, boundary } = cohort();
          expect(await run(output, boundary)).toBe(0);
          const aggregate = read(join(output, "selected-ci-receipt.json")) as Aggregate;
          const install = aggregate.runs[0];
          assert.ok(install);
          const retained = join(install.runRoot, "retained-run");
          mkdirSync(retained);
          for (let index = 0; index < 15; index++)
            write(join(retained, String(index) + "-process.json"), { status: 0 });
          const closed = join(output, "runtime-close-stage.json");
          ownershipReturn(output);
          expect(existsSync(closed)).toBe(true);
          rmSync(closed);
          const receiptPath = join(install.runRoot, "receipt.json");
          const integer = field === "final_exit_code" ? "0" : "15";
          writeFileSync(
            receiptPath,
            readFileSync(receiptPath, "utf8").replace(
              '"' + field + '": ' + integer,
              '"' + field + '": ' + integer + suffix,
            ),
          );
          expect(() => {
            ownershipReturn(output);
          }).toThrow();
          expect(existsSync(closed)).toBe(false);
        });
      });
  test("B2 nested integer diagnostics retain token identity without changing JSON values", () => {
    const path = join(directory(), "tokens.json");
    writeFileSync(
      path,
      '{"exit":0.0,"runs":[{"exit":0e0}],"final_exit_code":0E0,"integer":-0,"large":9007199254740993,"escaped\\u004bey":15}',
    );
    const value = read(path) as Aggregate & {
      final_exit_code: number;
      integer: number;
      large: number;
      escapedKey: number;
    };
    assert.ok(value.runs[0]);
    expect(value.exit).toBe(0);
    expect(value.runs[0].exit).toBe(0);
    expect(value.final_exit_code).toBe(0);
    expect(jsonInteger(value, "exit")).toBe(false);
    expect(jsonInteger(value.runs[0], "exit")).toBe(false);
    expect(jsonInteger(value, "final_exit_code")).toBe(false);
    expect(jsonInteger(value, "integer")).toBe(true);
    expect(jsonInteger(value, "large")).toBe(false);
    expect(jsonInteger(value, "escapedKey")).toBe(true);
  });

  for (const token of ["15", "15.0", "15e0", "15E0"])
    test("B2 preparation diagnostic preserves Python integer type: " + token, () => {
      const path = join(directory(), "diagnostic.json");
      writeFileSync(
        path,
        JSON.stringify({
          failed_phase: "container-prepare",
          failure_code: "SELECTED_DRIVER_EXCEPTION",
          original_driver_failure: { type: "ReturnedNonzero" },
          known_driver_checkpoint: "scripts/selected-backend-ci/current-sqlite-driver.py:120",
          preparation_command_exit: 15,
        }).replace('"preparation_command_exit":15', '"preparation_command_exit":' + token),
      );
      expect(publicFailureFields(read(path) as DriverReceipt).preparation_command_exit).toBe(
        token === "15" ? 15 : null,
      );
    });

  const childFixture = join(import.meta.dir, "interrupt-child.fixture.ts");
  test("B3 actual spawn abort kills and reaps a direct child ignoring SIGINT", async () => {
    const controller = new AbortController();
    const child = spawnSelectedCommand(
      [process.execPath, childFixture],
      process.env,
      "pipe",
      "pipe",
      controller.signal,
    );
    try {
      assert.ok(child.stdout instanceof ReadableStream);
      const reader = child.stdout.getReader();
      try {
        const ready = await reader.read();
        expect(ready.done).toBe(false);
        expect(Number(new TextDecoder().decode(ready.value).trim())).toBe(child.pid);
      } finally {
        reader.releaseLock();
      }
      controller.abort(new Error("actual child cancellation fixture"));
      await child.exited;
      expect(child.signalCode).toBe("SIGKILL");
      expect(() => process.kill(child.pid, 0)).toThrow();
    } finally {
      if (child.exitCode === null) child.kill("SIGKILL");
      await child.exited;
    }
  });
  test("B3 real child abort is awaited before run records exit130", async () => {
    const { output, boundary } = cohort();
    boundary.execute = async (_driver, environment, _log, signal) => {
      const child = spawnSelectedCommand(
        [process.execPath, childFixture],
        environment,
        "pipe",
        "pipe",
        signal,
      );
      try {
        assert.ok(child.stdout instanceof ReadableStream);
        const reader = child.stdout.getReader();
        try {
          const ready = await reader.read();
          expect(ready.done).toBe(false);
          expect(Number(new TextDecoder().decode(ready.value).trim())).toBe(child.pid);
        } finally {
          reader.releaseLock();
        }
        process.emit("SIGINT");
        await child.exited;
        expect(child.signalCode).toBe("SIGKILL");
        expect(() => process.kill(child.pid, 0)).toThrow();
        signal.throwIfAborted();
        throw new Error("SIGINT fixture must abort the run");
      } finally {
        if (child.exitCode === null) child.kill("SIGKILL");
        await child.exited;
      }
    };
    await withEnvironment(ci, async () => {
      expect(await run(output, boundary)).toBe(130);
    });
    expect(read(join(output, "selected-ci-receipt.json"))).toMatchObject({
      exit: 130,
      allRequestedRunsExecuted: false,
      runs: [],
    });
  });
  test("B3 actual stage CLI kills a SIGINT-resistant child and writes exit130", async () => {
    const output = directory();
    write(join(output, "before.json"), { head: source, tree });
    const child = spawn(
      [
        process.execPath,
        join(root, "scripts/run-selected-backend-e2e.ts"),
        "stage",
        "--output",
        output,
        "--stage-name",
        "main",
        "--",
        process.execPath,
        childFixture,
        "interrupt-parent",
      ],
      { cwd: root, env: { ...process.env, ...ci }, stdout: "pipe", stderr: "pipe" },
    );
    try {
      expect(await child.exited).toBe(130);
      expect(read(join(output, "main-stage.json"))).toMatchObject({ exit_code: 130 });
      const compilerPid = Number(readFileSync(join(output, "main-compiler.jsonl"), "utf8").trim());
      expect(Number.isSafeInteger(compilerPid) && compilerPid > 0).toBe(true);
      expect(() => process.kill(compilerPid, 0)).toThrow();
    } finally {
      if (child.exitCode === null) child.kill("SIGKILL");
      await child.exited;
    }
  });
});
