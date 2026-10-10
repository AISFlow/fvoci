import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import { randomBytes } from "node:crypto";
import { Buffer } from "node:buffer";
import {
  constants,
  chmodSync,
  closeSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative } from "node:path";
import process from "node:process";
import {
  admittedBrowser,
  assertRuntimeJob,
  browserInventory,
  identity,
  runtimeAccess,
} from "./admission.ts";
import { reference } from "./build.ts";
import { bindingModule } from "./drivers/binding.ts";
import { bunDriver, failureDigest } from "./drivers/common.ts";
import { restartHelper } from "./drivers/restart.ts";
import { publicCheckpoint } from "./drivers/sqlite.ts";
import {
  accessible,
  ancestors,
  assertHandoffActor,
  below,
  call,
  digest,
  env,
  files,
  gid,
  inventory,
  jsonInteger,
  observedExit,
  read,
  root,
  sha,
  sourceInputText,
  spawnSelectedCommand,
  tool,
  uid,
  write,
} from "./io.ts";
import type {
  Abi,
  Aggregate,
  Browser,
  Bundle,
  DriverReceipt,
  Flow,
  Inputs,
  Lane,
  Reference,
  Retirement,
  RunResult,
  Web,
} from "./types.ts";
import type { Environment } from "./io.ts";

export const selectedRuns = [
  ["install", "on"],
  ["postgres", "on"],
  ["sqlite", "on"],
  ["postgres", "off"],
  ["sqlite", "off"],
] as const;
// The caller's `--lane` argument is the only lane source; the runtime actor's
// sudo environment does not carry the shell's FVOCI_COLLAB_LANE.
export function requestedRuns(value?: string): readonly (readonly [Lane, Flow])[] {
  if (value === undefined) return selectedRuns;
  const match = /^(install|postgres|sqlite)\/(on|off)$/.exec(value);
  assert.ok(match, "unknown collaboration lane");
  const lane = match[1] as Lane;
  const flow = match[2] as Flow;
  assert.ok(
    selectedRuns.some(([itemLane, itemFlow]) => itemLane === lane && itemFlow === flow),
    "unknown collaboration lane",
  );
  return [[lane, flow]];
}
export const knownOnBrowserTest =
  "selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback";
const publicPhases = [
  "container-prepare",
  "server-startup",
  "server-ready",
  "browser",
  "restart",
  "install-body",
];
const failureTypes = ["ReturnedNonzero", "AssertionError", "RuntimeError"];
const failureCodes = ["SELECTED_DRIVER_EXCEPTION", "SELECTED_BODY_NONZERO"];
const reportStates = [
  "matched",
  "report-missing",
  "report-unreadable",
  "workers-not-one",
  "spec-mismatch",
  "status-not-known",
];
export function publicFailureFields(facts: DriverReceipt): Record<string, unknown> {
  const phase = facts.failed_phase,
    original = facts.original_driver_failure;
  const kind =
    original && typeof original === "object" && "type" in original ? original.type : undefined;
  const code = facts.failure_code,
    state =
      typeof facts.browser_report_state === "string" &&
      reportStates.includes(facts.browser_report_state)
        ? facts.browser_report_state
        : null;
  const show = state !== null || phase === "browser";
  const location = facts.known_browser_checkpoint;
  const checkpoint =
    show &&
    typeof location === "string" &&
    /^e2e-pending\/workspace-wiki-selected-(?:backend\.spec|auxiliary)\.ts:[1-9][0-9]{0,4}$/.test(
      location,
    ) &&
    Number(location.split(":").at(-1)) <= 10000
      ? location
      : null;
  const matched =
    state === "matched" || (state === null && phase === "browser" && checkpoint !== null);
  const driver = facts.known_driver_checkpoint,
    preparationExit = facts.preparation_command_exit;
  const preparation =
    phase === "container-prepare" &&
    typeof kind === "string" &&
    failureTypes.includes(kind) &&
    typeof code === "string" &&
    failureCodes.includes(code);
  const driverCheckpoint = preparation ? publicCheckpoint(driver) : null;
  return {
    failed_phase: typeof phase === "string" && publicPhases.includes(phase) ? phase : null,
    known_driver_checkpoint: driverCheckpoint,
    preparation_command_exit:
      driverCheckpoint !== null &&
      typeof preparationExit === "number" &&
      jsonInteger(facts, "preparation_command_exit") &&
      preparationExit >= -255 &&
      preparationExit <= 255
        ? preparationExit
        : null,
    original_driver_failure_type:
      typeof kind === "string" && failureTypes.includes(kind) ? kind : null,
    original_driver_failure_code:
      typeof code === "string" && failureCodes.includes(code) ? code : null,
    browser_report_state: state,
    known_browser_test:
      show && (matched || state === null) && facts.known_browser_test === knownOnBrowserTest
        ? knownOnBrowserTest
        : null,
    known_browser_status:
      show &&
      (matched || state === null) &&
      typeof facts.known_browser_status === "string" &&
      ["failed", "timedOut", "interrupted"].includes(facts.known_browser_status)
        ? facts.known_browser_status
        : null,
    known_browser_checkpoint: matched || state === null ? checkpoint : null,
  };
}
export function laneRetirement(
  runRoot: string,
  lane: Lane,
  flow: Flow,
  source: string,
  tree: string,
  owner: string,
  driverExit: number,
): Retirement {
  const facts: Retirement = {
    qualified: false,
    receiptPresent: false,
    refusalCodes: [],
    receiptSha256: null,
    originalFailureSha256: null,
    failedPhase: null,
  };
  try {
    const path = join(runRoot, "receipt.json");
    facts.receiptSha256 = sha(path);
    const value = read(path);
    assert.ok(value && typeof value === "object" && !Array.isArray(value));
    const receipt = value as DriverReceipt;
    facts.receiptPresent = true;
    const original = receipt.original_driver_failure ?? receipt.driver_error;
    if (original !== undefined && original !== null)
      facts.originalFailureSha256 = failureDigest(original);
    if (receipt.failed_phase && publicPhases.includes(receipt.failed_phase))
      facts.failedPhase = receipt.failed_phase;
    assert.ok(receipt.source === source && receipt.tree === tree && receipt.root_owner === owner);
    assert.ok(jsonInteger(receipt, "final_exit_code") && receipt.final_exit_code === driverExit);
    assert.equal(receipt.owned_container_absent, true);
    if (lane === "install")
      assert.ok(receipt.actual_tests === 4 && receipt.actual_owned_process_receipts === 15);
    else {
      assert.ok(receipt.selected_flow === flow && deepEquals(receipt.cleanup_errors, []));
      assert.equal(receipt.owned_loopback_port_closed, true);
      assert.equal(receipt.recorded_process_identities_retired, true);
      if (lane === "postgres") {
        const parent = read(join(runRoot, "parent-receipt.json")) as DriverReceipt;
        assert.ok(
          parent.source === source &&
            parent.tree === tree &&
            parent.root_owner === owner &&
            parent.selected_flow === flow,
        );
        assert.equal(parent.all_owned_fixtures_closed, true);
      }
      if (driverExit === 0) {
        if (flow === "on")
          assert.equal(receipt.current_schema_server_restart?.restartBrowserExit, 0);
        assert.ok(
          receipt.actual_browser_tests === (flow === "off" ? 8 : 1) && receipt.retries === 0,
        );
      }
    }
    facts.qualified = true;
  } catch {
    facts.refusalCodes.push("SELECTED_DRIVER_RETIREMENT_UNCONFIRMED");
  }
  return facts;
}

export function prepareBrowser(output: string, chromium: string): string {
  // The preparation owner is the actual host user, never root; its numeric
  // value is not an ownership check (macOS and CI hosts differ).
  const runner = uid();
  assert.ok(runner !== 0 && gid() !== 0, "browser preparation refuses root");
  const component = dirname(dirname(chromium)),
    cache = dirname(component);
  assert.match(basename(component), /^chromium-[0-9]+$/);
  assert.ok(
    !lstatSync(cache).isSymbolicLink() &&
      statSync(cache).uid === runner &&
      statSync(cache).gid === gid(),
  );
  const components = readdirSync(cache)
    .filter((name) => /^(chromium|chromium_headless_shell|ffmpeg)-[0-9]+$/.test(name))
    .sort();
  assert.ok(components.includes(basename(component)));
  const owner = [runner, gid()] as const;
  const before = Object.fromEntries(
    components.map((name) => [name, browserInventory(join(cache, name), owner)]),
  );
  const metadata = Object.fromEntries(
    components.map((name) => [name, browserInventory(join(cache, name), owner, true)]),
  );
  const destination = join(output, "browser");
  mkdirSync(destination, { mode: 0o700 });
  for (const name of components)
    cpSync(join(cache, name), join(destination, name), {
      recursive: true,
      preserveTimestamps: true,
      errorOnExist: true,
      force: false,
    });
  function restrict(directory: string): void {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name),
        facts = lstatSync(path);
      assert.ok(facts.uid === runner && (facts.isFile() || facts.isDirectory()));
      chmodSync(path, facts.isDirectory() || (facts.mode & 0o111) !== 0 ? 0o700 : 0o600);
      if (facts.isDirectory()) restrict(path);
    }
  }
  restrict(destination);
  assert.ok(
    deepEquals(
      Object.fromEntries(
        components.map((name) => [name, browserInventory(join(cache, name), owner, true)]),
      ),
      metadata,
    ),
  );
  assert.ok(
    deepEquals(
      Object.fromEntries(
        components.map((name) => [name, browserInventory(join(destination, name))]),
      ),
      before,
    ),
  );
  const copied = join(destination, relative(cache, chromium));
  write(join(output, "runtime-browser-stage.json"), {
    source: env("GITHUB_SHA"),
    cache: destination,
    chromium: copied,
    files: before,
    metadata: Object.fromEntries(
      components.map((name) => [name, browserInventory(join(destination, name), owner, true)]),
    ),
  });
  return copied;
}
export interface PermissionBoundary {
  identity: typeof identity;
  // Private runtime copy of the admitted browser; returns the copied executable.
  browser: (output: string, bun: string) => string;
}
const permissionBoundary: PermissionBoundary = {
  identity,
  browser: (output, bun) =>
    prepareBrowser(
      output,
      call([bun, "--eval", "console.log(require('@playwright/test').chromium.executablePath())"]),
    ),
};
export function runtimePermissions(
  output: string,
  sqliteParent: string,
  dockerGid: number,
  boundary: PermissionBoundary = permissionBoundary,
): void {
  // The docker socket group joins the actor's groups; group 0 is never granted.
  assert.ok(dockerGid > 0, "root group is never granted as the docker group");
  assert.equal(process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci", "github-ci");
  const owner = boundary.identity("run", output);
  assertRuntimeJob();
  const runnerUid = uid(),
    runnerGid = gid(),
    temp = realpathSync(env("RUNNER_TEMP"));
  assert.ok(
    output === join(temp, "fvoci-selected-current") && sqliteParent === join(temp, "fvoci-sqlite"),
  );
  for (const prefix of [output, sqliteParent]) {
    assert.ok(
      !lstatSync(prefix).isSymbolicLink() &&
        statSync(prefix).isDirectory() &&
        statSync(prefix).uid === runnerUid,
    );
    function check(directory: string): void {
      for (const name of readdirSync(directory)) {
        const path = join(directory, name),
          facts = lstatSync(path);
        assert.ok(!facts.isSymbolicLink() && facts.uid === runnerUid);
        if (facts.isDirectory()) check(path);
      }
    }
    check(prefix);
  }
  assert.ok(below(realpathSync(env("SQLITE3_LIB_DIR")), sqliteParent));
  const before = read(join(output, "before.json")) as Inputs;
  assert.equal(before.head, env("GITHUB_SHA"));
  assert.ok(deepEquals(read(join(output, "after.json")), before));
  const bun = realpathSync(tool("bun")),
    chromium = boundary.browser(output, bun);
  const paths: Record<string, number> = Object.fromEntries(
    Object.keys(before.tracked).map((p) => [join(root, p), constants.R_OK]),
  );
  for (const path of Object.keys(before.external)) paths[path] = constants.R_OK;
  for (const path of Object.keys((read(join(output, "bundle.json")) as Bundle).binaries))
    paths[path] = constants.R_OK | constants.X_OK;
  for (const name of Object.keys(browserInventory(dirname(chromium))))
    paths[join(dirname(chromium), name)] = constants.R_OK;
  paths[bun] = paths[chromium] = constants.R_OK | constants.X_OK;
  const accessiblePaths = Object.fromEntries(
    Object.entries(paths).filter(
      ([p]) => ![output, sqliteParent].some((prefix) => below(p, prefix)),
    ),
  );
  accessiblePaths[dirname(output)] = accessiblePaths[dirname(sqliteParent)] = constants.X_OK;
  const allowedGroups = new Set([dockerGid]),
    needed: Record<string, unknown> = {};
  for (const [path, mask] of Object.entries(accessiblePaths)) {
    for (const [entry, required] of [
      [path, mask],
      ...ancestors(path).map((p) => [p, constants.X_OK] as const),
    ] as [string, number][]) {
      const facts = statSync(entry),
        permitted =
          facts.uid === 1000
            ? facts.mode >> 6
            : facts.gid === 1000 || facts.gid === dockerGid
              ? facts.mode >> 3
              : facts.mode;
      if (
        (permitted & required) !== required &&
        facts.gid === runnerGid &&
        ((facts.mode >> 3) & required) === required
      ) {
        // A group-grant boundary, not an ownership check: never hand a system or
        // privileged group (root, adm, sudo, ... below GID 1000) to the actor.
        assert.ok(runnerGid >= 1000, "privileged preparation group is never granted");
        allowedGroups.add(runnerGid);
        needed[entry] = {
          path_sha256: digest(entry),
          uid: facts.uid,
          gid: facts.gid,
          mode: facts.mode & 0o777,
        };
      }
    }
  }
  const groupList = [...allowedGroups].sort((a, b) => a - b);
  const result = spawnSync(
    [
      "sudo",
      "setpriv",
      "--reuid=1000",
      "--regid=1000",
      "--groups=" + groupList.join(","),
      bun,
      join(import.meta.dir, "runtime-access-check.ts"),
    ],
    {
      stdin: Buffer.from(JSON.stringify({ groups: groupList, files: accessiblePaths })),
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  let receipt: unknown;
  try {
    receipt = JSON.parse(result.stdout.toString());
  } catch {
    receipt = { invalid_receipt: true };
  }
  const exit = observedExit(result);
  write(join(output, "runtime-access-stage.json"), {
    source: before.head,
    tree: before.tree,
    owner,
    runner_uid: runnerUid,
    runner_gid: runnerGid,
    runtime_uid: 1000,
    runtime_gid: 1000,
    groups: groupList,
    required_group_paths: Object.values(needed).slice(0, 16),
    required_group_path_count: Object.keys(needed).length,
    preflight_exit: exit,
    preflight: receipt,
  });
  assert.ok(
    exit === 0 &&
      receipt &&
      typeof receipt === "object" &&
      "missing" in receipt &&
      receipt.missing === 0,
  );
  process.stdout.write(groupList.join(",") + "\n");
}

export interface RunBoundary {
  identity: typeof identity;
  browser: (output: string) => Browser;
  access: typeof runtimeAccess;
  execute: (
    driver: string,
    environment: Environment,
    log: string,
    signal: AbortSignal,
  ) => number | Promise<number>;
}
// The lane drivers run on this Bun without .env autoload; each refuses to start
// without `--no-env-file`.
export const driverCommand = bunDriver;
export const laneDriver = (lane: Lane) => join(root, "tools/selected-backend-ci/drivers", lane + ".ts");
const runBoundary: RunBoundary = {
  identity,
  access: runtimeAccess,
  browser(output) {
    const bun = realpathSync(tool("bun")),
      chromium = call([
        bun,
        "--eval",
        "console.log(require('@playwright/test').chromium.executablePath())",
      ]);
    if ((process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci") === "github-ci")
      assert.equal(chromium, admittedBrowser(output));
    return {
      bun: { path: bun, sha256: sha(bun) },
      chromium: { path: chromium, sha256: sha(chromium) },
      chromium_directory_files: inventory(dirname(chromium)),
    };
  },
  async execute(driver, environment, log, signal) {
    const fd = openSync(log, "wx");
    try {
      const child = spawnSelectedCommand(driverCommand(driver), environment, fd, fd, signal);
      const exitCode = await child.exited;
      if (signal.aborted) throw signal.reason;
      return observedExit({ exitCode, signalCode: child.signalCode ?? undefined });
    } finally {
      closeSync(fd);
    }
  },
};
export async function run(
  output: string,
  boundary: RunBoundary = runBoundary,
  expected: readonly [number, number] = [1000, 1000],
  lane?: string,
): Promise<number> {
  assert.notEqual(process.env.GITHUB_JOB, "collaboration-build");
  const runs = requestedRuns(lane);
  assertHandoffActor(output, expected);
  const owner = boundary.identity("run", output),
    before = read(join(output, "before.json")) as Inputs;
  assert.ok(deepEquals(read(join(output, "after.json")), before));
  const runtime = join(output, "runtime");
  mkdirSync(runtime, { mode: 0o700 });
  const browser = boundary.browser(output);
  boundary.access(Object.keys(before.external), browser);
  const bundle = read(join(output, "bundle.json")) as Bundle,
    web = read(join(output, "web-receipt.json")) as Web,
    abi = read(join(output, "abi-receipt.json")) as Abi;
  for (const path of Object.keys(bundle.binaries))
    assert.ok(accessible(path, constants.R_OK | constants.X_OK));
  const sourceWritten = Object.fromEntries(
    ["head", "tree", "status", "tracked", "external", "untracked"].map((key) => [
      key,
      before[key as keyof Inputs],
    ]),
  );
  const environment: Environment = {
    ...process.env,
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: join(output, "bun-transpiler-cache"),
    FVOCI_CI_OWNER: owner,
    FVOCI_CI_BUN: browser.bun.path,
    FVOCI_CI_SELECTED_RUNS: runtime,
    FVOCI_ROOT_RUN_OWNER: owner,
  };
  const local = process.env.FVOCI_SELECTED_EXECUTION_MODE === "orca-local";
  const authority = local
    ? {
        executionMode: "orca-local",
        localAuthorizationSha256: env("FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256"),
        runId: env("FVOCI_LOCAL_RUN_ID"),
        runAttempt: env("FVOCI_LOCAL_DISPATCH_ID"),
      }
    : {
        exclusiveCIJob: true,
        currentCIJobConfirmed: true,
        runId: env("GITHUB_RUN_ID"),
        runAttempt: env("GITHUB_RUN_ATTEMPT"),
      };
  // A later single lane reads the install receipt its caller placed in output.
  const closedInstall = join(output, "closed-install-receipt.json");
  let closed: Reference | null = null;
  if (runs.length === 1 && runs[0]?.[0] !== "install") {
    assert.ok(lstatSync(closedInstall).isFile(), "closed install receipt required");
    closed = reference(closedInstall);
  }
  let code = 0;
  const results: RunResult[] = [];
  let launcherFailure: {
    code: string;
    originalOutcomeSha256: string;
    sha256: string | null;
    receiptWrite: string;
  } | null = null;
  const controller = new AbortController();
  const interrupt = () => {
    const error = new Error("selected launcher interrupted");
    error.name = "InterruptError";
    controller.abort(error);
  };
  process.once("SIGINT", interrupt);
  try {
    for (const [lane, flow] of runs) {
      const runRoot = join(runtime, "root-current-" + lane + "-" + randomBytes(6).toString("hex")),
        driver = laneDriver(lane);
      const manifest: Record<string, unknown> = {
        schema: 1,
        ready: true,
        flow,
        source: before.head,
        tree: before.tree,
        compiledSource: before.head,
        sourceInputsBefore: reference(join(output, "before.json")),
        sourceInputsAfter: reference(join(output, "after.json")),
        bundle: reference(join(output, "bundle.json")),
        compileReceipt: reference(join(output, "compile-receipt.json")),
        webReceipt: reference(join(output, "web-receipt.json")),
        abiReceipt: reference(join(output, "abi-receipt.json")),
        nativeQualification: null,
        browserInputs: browser,
        closedInstallReceipt: closed,
      };
      if (lane !== "install") assert.ok(closed, "mandatory actual current installation4 failed");
      if (lane !== "install" && flow === "on") {
        const binding = {
          runId: authority.runId,
          runAttempt: authority.runAttempt,
          source: before.head,
          tree: before.tree,
          compiledSource: before.head,
          backend: lane,
          runRoot,
          parentDriverSha256: sha(driver),
          restartHelperSha256: sha(restartHelper),
          sourceInputsSha256: digest(sourceInputText(sourceWritten)),
          artifactHashes: Object.fromEntries(
            Object.entries(bundle.binaries).map(([p, r]) => [p, r.sha256]),
          ),
          assetHashes: web.dist_files,
          browserInputs: browser,
          abiHashes: abi.host_runtime_files,
        };
        const restart = join(output, lane + "-" + flow + "-restart-allocation.json");
        write(restart, {
          schema: 1,
          status: "GRANTED",
          owner,
          ...authority,
          source: before.head,
          tree: before.tree,
          compiledSource: before.head,
          backend: lane,
          binding,
        });
        manifest.restartAllocation = reference(restart);
        environment.FVOCI_ROOT_RESTART_GRANT = restart;
      } else delete environment.FVOCI_ROOT_RESTART_GRANT;
      const manifestPath = join(output, lane + "-" + flow + "-binding.json");
      write(manifestPath, manifest);
      const allocation = join(output, lane + "-" + flow + "-allocation.json");
      write(allocation, {
        schema: 1,
        status: "GRANTED",
        owner,
        ...authority,
        flow,
        source: before.head,
        tree: before.tree,
        compiledSource: before.head,
        lane,
        backend: lane === "install" ? null : lane,
        runRoot,
        driverSha256: sha(driver),
        bindingSha256: sha(manifestPath),
        bindingModuleSha256: sha(bindingModule),
      });
      environment.FVOCI_ROOT_CURRENT_BINDING = manifestPath;
      environment.FVOCI_ROOT_CURRENT_ALLOCATION = allocation;
      environment.FVOCI_E2E_SELECTED_FLOW = flow;
      if (lane === "postgres" && flow === "on")
        environment.FVOCI_E2E_SELECTED_AUXILIARY = "normal-api";
      else delete environment.FVOCI_E2E_SELECTED_AUXILIARY;
      const at = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
      process.stdout.write(`selected-driver lane=${lane} flow=${flow} started at=${at()}\n`);
      const start = performance.now(),
        exit = await boundary.execute(
          driver,
          environment,
          join(output, lane + "-" + flow + "-driver.log"),
          controller.signal,
        );
      process.stdout.write(
        `selected-driver lane=${lane} flow=${flow} finished at=${at()} elapsed_seconds=${String(Math.round((performance.now() - start) / 1000))} exit=${String(exit)}\n`,
      );
      const result: RunResult = { lane, flow, exit, actualSource: before.head, runRoot };
      results.push(result);
      code ||= exit;
      result.retirement = laneRetirement(
        runRoot,
        lane,
        flow,
        before.head,
        before.tree,
        owner,
        exit,
      );
      if (!result.retirement.qualified) code ||= 1;
      if (exit !== 0 || !result.retirement.qualified) break;
      if (lane === "install") {
        const receipt = join(runRoot, "receipt.json");
        closed = reference(receipt);
        // Exclusive private copy: never replace an occupied destination.
        if (runs.length === 1)
          writeFileSync(closedInstall, readFileSync(receipt), { flag: "wx", mode: 0o600 });
      }
    }
  } catch (error) {
    code ||= controller.signal.aborted ? 130 : 1;
    const packet = {
      type: error instanceof Error ? error.name : "Error",
      message: error instanceof Error ? error.message.slice(0, 4096) : "non-Error thrown",
    };
    launcherFailure = {
      code: "SELECTED_LAUNCHER_FAILED",
      originalOutcomeSha256: failureDigest(packet),
      sha256: null,
      receiptWrite: "not-attempted",
    };
    try {
      const original = join(output, "selected-launcher-failure.private.json");
      write(original, packet);
      launcherFailure.sha256 = sha(original);
      launcherFailure.receiptWrite = "confirmed";
    } catch {
      launcherFailure.receiptWrite = "failed";
    }
  } finally {
    process.removeListener("SIGINT", interrupt);
    const complete = deepEquals(
      results.map((r) => [r.lane, r.flow]),
      runs,
    );
    if (!complete) code ||= 1;
    try {
      write(join(output, "selected-ci-receipt.json"), {
        source: before.head,
        tree: before.tree,
        owner,
        runs: results,
        exit: code,
        allRequestedRunsExecuted: complete,
        launcherFailure,
        normalBothAndRestartRequired: true,
        offBothRequired: true,
        offTestsPerBackend: 8,
        sqliteAuxiliary: "BLOCKED: normal writers unported",
        whole060Complete: false,
      });
    } catch {
      code ||= 1;
      process.stdout.write(
        JSON.stringify({
          source: before.head,
          tree: before.tree,
          exit: code,
          launcherFailure,
          aggregateReceiptWrite: "failed",
        }) + "\n",
      );
    }
  }
  return code;
}

export function ownershipReturn(
  output: string,
  expected: readonly [number, number] = [1000, 1000],
  lane?: string,
  admit: typeof identity = identity,
): void {
  const diagnostic: Record<string, unknown> = {
    schema: 1,
    phase: "identity",
    source: null,
    tree: null,
    ownership_return_qualified: false,
    lanes: [],
  };
  const lanes: Record<string, unknown>[] = [];
  diagnostic.lanes = lanes;
  try {
    assert.equal(process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci", "github-ci");
    assertHandoffActor(output, expected);
    const owner = admit("run", output);
    diagnostic.phase = "current-source";
    assertRuntimeJob();
    const requested = requestedRuns(lane);
    const before = read(join(output, "before.json")) as Inputs;
    assert.equal(before.head, env("GITHUB_SHA"));
    diagnostic.source = before.head;
    diagnostic.tree = /^[0-9a-f]{40}$/.test(before.tree) ? before.tree : null;
    const runtime = join(output, "runtime"),
      allocated = readdirSync(output).filter((p) => /-(allocation|binding)\.json$/.test(p));
    let proof: Record<string, unknown>;
    if (!allocated.length && (!existsSync(runtime) || !readdirSync(runtime).length))
      proof = { no_runtime_started: true };
    else {
      diagnostic.phase = "selected-receipt";
      const result = read(join(output, "selected-ci-receipt.json")) as Aggregate;
      assert.ok(
        result.owner === owner && result.source === before.head && result.tree === before.tree,
      );
      diagnostic.selected_exit = jsonInteger(result, "exit") ? result.exit : null;
      diagnostic.all_requested_runs_executed = Object.is(result.allRequestedRunsExecuted, true);
      assert.equal(new Set(result.runs.map((r) => r.runRoot)).size, result.runs.length);
      for (const run of result.runs) {
        diagnostic.phase = run.lane;
        const path = run.runRoot;
        assert.ok(
          dirname(path) === runtime && basename(path).startsWith("root-current-" + run.lane + "-"),
        );
        const facts = read(join(path, "receipt.json")) as DriverReceipt;
        const required: Record<string, string> = {
          source: "string",
          tree: "string",
          root_owner: "string",
          final_exit_code: "integer",
          owned_container_absent: "boolean",
          ...(run.lane === "install"
            ? { actual_owned_process_receipts: "integer" }
            : {
                selected_flow: "string",
                owned_loopback_port_closed: "boolean",
                recorded_process_identities_retired: "boolean",
                cleanup_errors: "array",
              }),
        };
        const valid = (key: string, type: string) =>
          type === "integer"
            ? jsonInteger(facts, key)
            : type === "array"
              ? Array.isArray(facts[key])
              : typeof facts[key] === type;
        lanes.push({
          lane: run.lane,
          flow: run.flow,
          launcher_observed_driver_exit: jsonInteger(run, "exit") ? run.exit : null,
          receipt_final_exit: jsonInteger(facts, "final_exit_code") ? facts.final_exit_code : null,
          receipt_sha256: sha(join(path, "receipt.json")),
          missing_required_fields: Object.keys(required).filter((k) => !(k in facts)),
          invalid_required_fields: Object.entries(required)
            .filter(([k, t]) => k in facts && !valid(k, t))
            .map(([k]) => k),
          source_matches_current: facts.source === before.head,
          closure_facts: Object.fromEntries(
            [
              "owned_container_absent",
              "owned_loopback_port_closed",
              "recorded_process_identities_retired",
            ].map((k) => [k, typeof facts[k] === "boolean" ? facts[k] : null]),
          ),
          cleanup_error_count: Array.isArray(facts.cleanup_errors)
            ? facts.cleanup_errors.length
            : null,
          original_driver_failure_sha256:
            facts.original_driver_failure === undefined
              ? null
              : failureDigest(facts.original_driver_failure),
          ...publicFailureFields(facts),
        });
        assert.ok(Object.entries(required).every(([k, t]) => k in facts && valid(k, t)));
        assert.ok(
          run.actualSource === before.head &&
            facts.source === before.head &&
            facts.tree === before.tree &&
            facts.root_owner === owner &&
            facts.final_exit_code === run.exit,
        );
        assert.equal(facts.owned_container_absent, true);
        if (run.lane === "install") {
          assert.equal(facts.actual_owned_process_receipts, 15);
          const processes = files(join(path, "retained-run")).filter((p) =>
            p.endsWith("process.json"),
          );
          assert.ok(
            processes.length === 15 &&
              processes.every((p) => {
                const record = read(p) as { status: unknown };
                return record.status !== null && record.status !== undefined;
              }),
          );
        } else {
          assert.ok(facts.selected_flow === run.flow && deepEquals(facts.cleanup_errors, []));
          assert.equal(facts.owned_loopback_port_closed, true);
          assert.equal(facts.recorded_process_identities_retired, true);
          if (run.lane === "postgres") {
            const parent = read(join(path, "parent-receipt.json")) as DriverReceipt;
            assert.ok(
              parent.source === before.head &&
                parent.tree === before.tree &&
                parent.root_owner === owner &&
                parent.selected_flow === run.flow,
            );
            assert.equal(parent.all_owned_fixtures_closed, true);
          }
        }
      }
      assert.ok(
        deepEquals(
          result.runs.map((r) => [r.lane, r.flow]),
          requested,
        ),
      );
      proof = {
        closed_current_runs: requested.map(([lane, flow]) => ({ lane, flow })),
        ...(requested.some(([lane]) => lane === "install")
          ? { installation_process_receipts: 15 }
          : {}),
      };
    }
    write(join(output, "runtime-close-stage.json"), {
      source: before.head,
      tree: before.tree,
      owner,
      ownership_return_qualified: true,
      ...proof,
    });
    diagnostic.ownership_return_qualified = true;
  } catch (error) {
    diagnostic.proof_error_type = error instanceof Error ? error.name : "Error";
    throw error;
  } finally {
    process.stdout.write(JSON.stringify(diagnostic) + "\n");
  }
}
