import type { CommandOptions, Completed } from "./drivers/common.ts";
import { nativeArchitecture, type NativeArchitecture } from "./types.ts";
import { deepEquals, spawn, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import { randomBytes } from "node:crypto";
import { Buffer } from "node:buffer";
import {
  constants,
  chmodSync,
  closeSync,
  cpSync,
  existsSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  statfsSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative } from "node:path";
import process from "node:process";
import {
  admittedBrowser,
  assertRuntimeJob,
  browserInventory,
  identity,
  localAllocation,
  runtimeAccess,
} from "./admission.ts";
import { reference } from "./build.ts";
import {
  accessible,
  ancestors,
  assertHandoffActor,
  below,
  bunDriver,
  call,
  digest,
  env,
  failureDigest,
  files,
  gid,
  groups,
  inventory,
  jsonInteger,
  observedExit,
  publicCheckpoint,
  read,
  root,
  sha,
  sourceInputText,
  spawnSelectedCommand,
  tool,
  uid,
  wrapperInterruptGrace,
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
  purpose?: "equal-work" | "auth-session",
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
        if (purpose === "equal-work") {
          assert.equal(receipt.purpose, purpose);
          assert.ok(
            receipt.equal_work && receipt.actual_browser_tests === 1 && receipt.retries === 0,
          );
        } else if (purpose === "auth-session") {
          // Exactly the three dedicated auth cases with their event oracle; no
          // restart, restore or original suite runs in this lane.
          assert.equal(receipt.purpose, purpose);
          assert.ok(
            typeof receipt.auth_session === "object" &&
              receipt.auth_session !== null &&
              (receipt.auth_session as Record<string, unknown>).cases === 3 &&
              (receipt.auth_session as Record<string, unknown>).events === "PASS" &&
              receipt.actual_browser_tests === 3 &&
              receipt.retries === 0 &&
              receipt.current_schema_server_restart === undefined,
          );
        } else {
          if (flow === "on")
            assert.equal(receipt.current_schema_server_restart?.restartBrowserExit, 0);
          assert.ok(
            receipt.actual_browser_tests === (flow === "off" ? 8 : 1) && receipt.retries === 0,
          );
          if (lane === "sqlite" && flow === "on")
            assert.ok(
              deepEquals(receipt.sqlite_restore_browser, {
                exit: 0,
                actual_tests: 1,
                skipped: 0,
                retries: 0,
              }),
            );
        }
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
  assert.ok(!lstatSync(cache).isSymbolicLink());
  assert.ok(statSync(cache).uid === runner && statSync(cache).gid === gid());
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
    selectedImageHost()
      ? admittedBrowser(output, [uid(), gid()])
      : prepareBrowser(
          output,
          call([
            bun,
            "--eval",
            "console.log(require('@playwright/test').chromium.executablePath())",
          ]),
        ),
};
export async function runtimePermissions(
  output: string,
  sqliteParent: string,
  dockerGid: number,
  boundary: PermissionBoundary = permissionBoundary,
): Promise<void> {
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
        if (facts.isSymbolicLink() || facts.uid !== runnerUid)
          try {
            process.stderr.write(
              `selected permissions descendant prefix=${prefix === output ? "output" : "sqlite"} symlink=${facts.isSymbolicLink() ? "1" : "0"} owner_match=${facts.uid === runnerUid ? "1" : "0"}\n`,
            );
          } catch {
            // Diagnostic output cannot replace the original permission refusal.
          }
        assert.ok(!facts.isSymbolicLink() && facts.uid === runnerUid);
        if (facts.isDirectory()) check(path);
      }
    }
    check(prefix);
  }
  if (selectedImageHost())
    assert.equal(env("SQLITE3_LIB_DIR"), "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/lib");
  else assert.ok(below(realpathSync(env("SQLITE3_LIB_DIR")), sqliteParent));
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
  const hostPath = (path: string) =>
    path === root || path === temp || below(path, root) || below(path, temp);
  const hostPaths = selectedImageHost()
    ? Object.fromEntries(Object.entries(accessiblePaths).filter(([path]) => hostPath(path)))
    : accessiblePaths;
  if (selectedImageHost()) {
    assert.equal(
      sha(process.execPath),
      before.external[bun],
      "host controller Bun differs from the admitted Bun",
    );
    hostPaths[process.execPath] = constants.R_OK | constants.X_OK;
  }
  // Docker includes the existing primary GID in getgroups(); setpriv does
  // not unless requested. Keep both actual actors and the strict preflight
  // on the same set without granting any access beyond primary GID 1000.
  const allowedGroups = new Set([dockerGid, ...(selectedImageHost() ? [1000] : [])]),
    needed: Record<string, unknown> = {};
  for (const [path, mask] of Object.entries(hostPaths)) {
    // The same complete set is checked in the image below. Host group grants
    // apply only to actual bind-mounted source and private preparation files.
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
  if (selectedImageHost())
    await selectedImageAccess({ groups: groupList, files: accessiblePaths }, root, true);
  // Retain the original host preflight for source ancestry and the actual
  // controller executable; image mount ancestry cannot prove host traversal.
  const result = spawnSync(
    [
      "sudo",
      "setpriv",
      "--reuid=1000",
      "--regid=1000",
      "--groups=" + groupList.join(","),
      selectedImageHost() ? process.execPath : bun,
      join(import.meta.dir, "runtime-access-check.ts"),
    ],
    {
      stdin: Buffer.from(JSON.stringify({ groups: groupList, files: hostPaths })),
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
export const laneDriver = (lane: Lane) =>
  join(root, "tools/selected-backend-ci/drivers", lane + ".ts");
// The grants hash these driver modules by path. The runner never imports a
// lane-driver module: its top level would run before the lease is checked.
export const bindingModule = join(root, "tools/selected-backend-ci/drivers/binding.ts");
export const restartHelper = join(root, "tools/selected-backend-ci/drivers/restart.ts");
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
  execute(driver, environment, log, signal) {
    return executeDriver(driverCommand(driver), environment, log, signal);
  },
};
// Outlasts the longest grace a lane driver gives its own fixture wrappers (the
// PostgreSQL wrapper, 2 x wrapperInterruptGrace) and the cleanup after it.
export const driverInterruptGrace = 3 * wrapperInterruptGrace;

// Private Bun errors become only fixed source locations, never messages or argv.
export function driverFailureDiagnostics(text: string): string[] {
  if (text.length > 16384 || Buffer.byteLength(text, "utf8") > 16384) return [];
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if ((code < 32 && code !== 10 && code !== 9) || code === 127) return [];
  }
  const lines = text.split("\n");
  if (lines.length > 128) return [];
  const sources = [
    "runtime.ts",
    "admission.ts",
    "io.ts",
    "drivers/binding.ts",
    "drivers/common.ts",
    "drivers/install.ts",
    "drivers/postgres.ts",
    "drivers/sqlite.ts",
    "drivers/restart.ts",
  ].map((name) => "tools/selected-backend-ci/" + name);
  const kinds = [
    "Error",
    "AssertionError",
    "CheckError",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "RuntimeError",
  ];
  const result = new Set<string>();
  let kind: string | undefined;
  for (const line of lines) {
    const projected =
      /^selected-driver failure type=([A-Za-z]+) source=(tools\/selected-backend-ci\/[A-Za-z/.-]+):([1-9][0-9]{0,4})$/.exec(
        line,
      );
    if (projected) {
      if (
        kinds.includes(projected[1] ?? "") &&
        sources.includes(projected[2] ?? "") &&
        Number(projected[3]) <= 10000
      )
        result.add(line);
    } else {
      const header = /^(error|Error|[A-Za-z]+Error)(?: \[[A-Z0-9_]+\])?:/.exec(line);
      if (header) {
        const name = header[1] === "error" ? "Error" : header[1];
        kind = kinds.includes(name ?? "") ? name : undefined;
      }
      const frame = /^\s+at .*?\(?(\/[^()]*?):([1-9][0-9]{0,4}):[1-9][0-9]{0,4}\)?$/.exec(line);
      const source = sources.find((name) => join(root, name) === frame?.[1]),
        sourceLine = Number(frame?.[2]);
      if (kind && source && sourceLine <= 10000)
        result.add(`selected-driver failure type=${kind} source=${source}:${String(sourceLine)}`);
    }
    if (result.size === 4) break;
  }
  return [...result];
}

// A lane driver owns fixture wrappers whose EXIT traps remove their
// containers. On an interrupt it is not killed at once, as a stage child is:
// it gets SIGINT and is waited for, so it closes its lane and writes its
// receipt before the runner returns. Only a driver still running after
// `grace` is SIGKILLed. The interrupt is then raised as before (exit 130).
export async function executeDriver(
  command: string[],
  environment: Environment,
  log: string,
  signal: AbortSignal,
  grace = driverInterruptGrace,
): Promise<number> {
  signal.throwIfAborted();
  const fd = openSync(log, "wx+", 0o600);
  let deadline: ReturnType<typeof setTimeout> | undefined;
  try {
    const child = spawn(command, {
      cwd: root,
      env: environment,
      stdout: fd,
      stderr: fd,
      stdin: "inherit",
    });
    const interrupt = () => {
      child.kill("SIGINT");
      deadline ??= setTimeout(() => {
        child.kill("SIGKILL");
      }, grace);
    };
    signal.addEventListener("abort", interrupt, { once: true });
    try {
      const exitCode = await child.exited;
      if (signal.aborted) throw signal.reason;
      const code = observedExit({ exitCode, signalCode: child.signalCode ?? undefined });
      if (code !== 0)
        try {
          const facts = fstatSync(fd),
            path = lstatSync(log);
          if (
            facts.isFile() &&
            !path.isSymbolicLink() &&
            facts.dev === path.dev &&
            facts.ino === path.ino &&
            facts.uid === uid() &&
            facts.gid === gid() &&
            (facts.mode & 0o777) === 0o600 &&
            realpathSync(log) === log
          ) {
            const bytes = Buffer.alloc(Math.min(facts.size, 16384)),
              offset = facts.size - bytes.length,
              count = readSync(fd, bytes, 0, bytes.length, offset),
              lines = bytes.subarray(0, count).toString("utf8").split("\n");
            for (const line of driverFailureDiagnostics(
              lines
                .slice(offset ? 1 : 0)
                .slice(-128)
                .join("\n"),
            ))
              process.stderr.write(line + "\n");
          }
        } catch {
          // Diagnostic failure cannot replace the actual child outcome.
        }
      return code;
    } finally {
      signal.removeEventListener("abort", interrupt);
    }
  } finally {
    clearTimeout(deadline);
    closeSync(fd);
  }
}
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
  if (selectedImageHost()) {
    const files = Object.fromEntries(
      Object.keys(before.external).map((path) => [path, constants.R_OK]),
    );
    for (const path of [browser.bun.path, browser.chromium.path])
      files[path] = constants.R_OK | constants.X_OK;
    await selectedImageAccess({ groups: groups(), files }, root);
    boundary.access(
      Object.keys(before.external).filter((path) => below(path, root) || below(path, output)),
      browser,
    );
  } else boundary.access(Object.keys(before.external), browser);
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
  const purpose = local ? localAllocation("run").purpose : undefined;
  assert.ok(purpose === undefined || purpose === "equal-work" || purpose === "auth-session");
  if (purpose)
    assert.ok(
      deepEquals(runs, selectedRuns),
      "a lane purpose requires both complete backend pairs",
    );
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
  const equalPairs: object[] = [];
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
  // Not once: the runner now waits for the driver, and a second SIGINT must not
  // end it before the aggregate receipt is written.
  process.on("SIGINT", interrupt);
  try {
    for (const [lane, flow] of runs) {
      const runRoot = join(runtime, "root-current-" + lane + "-" + randomBytes(6).toString("hex")),
        driver = laneDriver(lane);
      const manifest: Record<string, unknown> = {
        ...(lane !== "install" && purpose ? { purpose } : {}),
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
        lane === "install" ? undefined : purpose,
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
    if (purpose === "equal-work") {
      // The registered consumer is loaded only after identity/lease validation
      // and successful driver retirement; CLI discovery loads no driver code.
      const consumer = join(root, "tools/selected-backend-ci/drivers/equal-work.ts");
      const { comparePair } = (await import(consumer)) as typeof import("./drivers/equal-work.ts");
      for (const backend of ["postgres", "sqlite"] as const) {
        const receipt = (flow: Flow) => {
          const result = results.find((r) => r.lane === backend && r.flow === flow);
          assert.ok(result && result.exit === 0 && result.retirement?.qualified === true);
          return read(join(result.runRoot, "receipt.json")) as DriverReceipt;
        };
        equalPairs.push(comparePair(receipt("on"), receipt("off")));
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
        purpose: purpose ?? null,
        equalPairs,
        ...(purpose === "auth-session"
          ? { authCasesPerBackendRun: 3, hostedAuth: "NOT_CONNECTED" }
          : { normalBothAndRestartRequired: true, offBothRequired: true, offTestsPerBackend: 8 }),
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

export const CI_WEB_IMAGE =
  "ghcr.io/aisflow/fvoci-ci-web@sha256:cb1f5290f97ad5fb156b852ac0d21466e91571063979af4e9838172f41d8e23c";

export const CI_WEB_IMAGE_SOURCE = "a1d59f717dd522fd5ca5a63aff83118f84889ad6";
export const CI_WEB_SDK_SHA256 = {
  x86_64: "dfd2e7b84d19f6e8ca03b80ded497d69606df8685ed6f1a929b6e8ecaba35892",
  aarch64: "171384584c2a218a591e6d42784578c28ab1f64741487090b285006022260cca",
} as const;

// The host keeps the selected drivers and their Docker/UID/process controls.
// Only these fixed preparation, admission and browser operations enter the
// existing tool image. There is deliberately no arbitrary command operation.
export const SELECTED_IMAGE_ENV = [
  "FVOCI_WEB_CONTAINER_IMAGE",
  "FVOCI_WEB_CONTAINER_SDK_SHA256",
  "FVOCI_WEB_CONTAINER_IMAGE_SOURCE",
  "FVOCI_SELECTED_CI_OUTPUT",
  "FVOCI_SELECTED_CI_SQLITE_PARENT",
  "FVOCI_WEB_BUILD_HANDOFF",
  "FVOCI_WEB_BUILD_HANDOFF_SHA256",
  "FVOCI_WEB_BUILD_PHASE",
  "FVOCI_SELECTED_IMAGE_DEADLINE",
  "RUNNER_TEMP",
  "CARGO_HOME",
  "CARGO_TARGET_DIR",
  "CARGO_BUILD_JOBS",
  "CARGO_INCREMENTAL",
  "CARGO_PROFILE_DEV_DEBUG",
  "CARGO_PROFILE_TEST_DEBUG",
  "LIBCLANG_PATH",
  "SQLITE3_LIB_DIR",
  "SQLITE3_INCLUDE_DIR",
  "SQLITE3_STATIC",
  "SQLITE3_NO_PKG_CONFIG",
] as const;

export function selectedImageHost(env: NodeJS.ProcessEnv = process.env): boolean {
  return (
    env.FVOCI_WEB_CONTAINER_IMAGE !== undefined &&
    env.FVOCI_SELECTED_CI_SQLITE_PARENT !== undefined &&
    env.FVOCI_SELECTED_IMAGE_WORKLOAD !== "1"
  );
}

export function selectedImageBudget(value: string | undefined, now: number): number {
  const deadline = Number(value),
    remaining = deadline - now;
  if (!Number.isSafeInteger(deadline) || remaining <= 0 || remaining > 900_000)
    fail("original selected entry budget exhausted or invalid");
  return remaining;
}

export type SelectedImageOperation =
  "prepare" | "consume" | "pending" | "inputs" | "access" | "config-list" | "browser";

function selectedImageInvocation(
  operation: SelectedImageOperation,
  root: string,
  environment: NodeJS.ProcessEnv,
  browserArgs: string[] = [],
  actor?: { groups: number[] },
): { args: string[]; name: string; envFile: string; nodeTemp?: string } {
  if (!selectedImageHost() || process.platform !== "linux" || process.arch !== "x64")
    fail("selected image operations require the native Linux x64 host controller");
  const architecture = containerArchitecture(
    process.platform,
    process.arch,
    checkedOutput(["uname", "-m"], root),
  );
  if (
    architecture !== "x86_64" ||
    (process.env.DOCKER_DEFAULT_PLATFORM !== undefined &&
      process.env.DOCKER_DEFAULT_PLATFORM !== "linux/amd64")
  )
    fail("selected image requires the physical native x64 daemon");
  qualifyContainerDaemon(
    dockerEndpoint(root),
    checkedOutput(["docker", "info", "--format", "{{.OSType}} {{.Architecture}}"], root),
    architecture,
  );
  selectedImageBudget(process.env.FVOCI_SELECTED_IMAGE_DEADLINE, Date.now());
  const temp = realpathSync(process.env.RUNNER_TEMP ?? fail("RUNNER_TEMP required")),
    output = join(temp, "fvoci-selected-current"),
    owner = process.getuid?.(),
    uid = actor ? 1000 : owner,
    gid = actor ? 1000 : process.getgid?.();
  if (
    !uid ||
    !gid ||
    process.env.FVOCI_SELECTED_CI_OUTPUT !== output ||
    realpathSync(root) !== root
  )
    fail("selected image requires the exact owned output and source paths");
  if (statSync(output).uid !== owner || lstatSync(output).isSymbolicLink())
    fail("selected image output owner changed");
  const sdk = containerImage(
    process.env.FVOCI_WEB_CONTAINER_IMAGE ?? "",
    process.env.FVOCI_WEB_CONTAINER_SDK_SHA256 ?? "",
    false,
    process.env,
  );
  if (sdk.image !== CI_WEB_IMAGE || sdk.sdk !== CI_WEB_SDK_SHA256.x86_64)
    fail("selected image differs from the published native SDK");
  const metadata = checkedOutput(
    [
      "docker",
      "image",
      "inspect",
      "--format",
      '{{.Id}} {{.Os}} {{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}} {{json .RepoDigests}}',
      sdk.image,
    ],
    root,
  );
  const imageSource = qualifyImageSource(
    root,
    metadata,
    sdk,
    architecture,
    checkedOutput(["git", "rev-parse", "HEAD"], root),
  );
  if (imageSource !== process.env.FVOCI_WEB_CONTAINER_IMAGE_SOURCE)
    fail("selected image source changed");
  if (statfsSync(output).bavail * statfsSync(output).bsize < 20_000_000_000)
    fail("selected image requires the existing 20GB disk floor");
  const privateDir = join(output, "tmp");
  const privateFacts = statSync(privateDir);
  if (
    lstatSync(privateDir).isSymbolicLink() ||
    privateFacts.uid !== owner ||
    (privateFacts.mode & 0o777) !== 0o700
  )
    fail("selected image private temporary directory required");
  const name = "fvoci-selected-tool-" + randomBytes(16).toString("hex"),
    envFile = join(privateDir, name + ".env");
  const variables: NodeJS.ProcessEnv = {
    ...Object.fromEntries(
      Object.entries(environment).filter(
        ([key]) =>
          /^(?:CI|GITHUB_(?:ACTIONS|SHA|REPOSITORY|RUN_ID|RUN_ATTEMPT|JOB|OUTPUT)|RUNNER_TEMP|LANG|LC_ALL|CARGO_(?:BUILD_JOBS|INCREMENTAL|PROFILE_DEV_DEBUG|PROFILE_TEST_DEBUG)|FVOCI_E2E_PENDING|FVOCI_E2E_PROFILE)$/.test(
            key,
          ) ||
          (["pending", "browser"].includes(operation) &&
            /^(?:FVOCI_(?!TEST_TURSO|LIBSQL)[A-Z0-9_]+|BUN_RUNTIME_TRANSPILER_CACHE_PATH|PLAYWRIGHT_[A-Z0-9_]+|TEST_DATABASE_URL|DATABASE_[A-Z0-9_]+|MEILI_[A-Z0-9_]+|PASSWORD_PEPPER_[A-Z0-9_]+|ENCRYPTION_[A-Z0-9_]+|SMTP_[A-Z0-9_]+|GROUP_LABEL|NET_[A-Z0-9_]+|RUST_LOG)$/.test(
              key,
            )),
      ),
    ),
    ...Object.fromEntries(SELECTED_IMAGE_ENV.map((key) => [key, process.env[key]])),
    FVOCI_SELECTED_IMAGE_WORKLOAD: "1",
    ROOT: root,
    HOME: join(output, "home"),
    TMPDIR: actor ? "/tmp" : privateDir,
    BUN_INSTALL_CACHE_DIR: "/tmp/fvoci-bun-install-cache",
    CARGO_HOME: join(temp, "fvoci-image-cargo"),
    PLAYWRIGHT_BROWSERS_PATH: join(output, "browser"),
  };
  // Compiler PATH comes from the admitted image, not the host environment.
  const imageEnvironment: unknown = JSON.parse(
    checkedOutput(
      ["docker", "image", "inspect", "--format", "{{json .Config.Env}}", sdk.image],
      root,
    ),
  );
  if (!Array.isArray(imageEnvironment)) fail("selected image environment missing");
  const imageEntries: unknown[] = imageEnvironment;
  const imagePath = imageEntries.find(
    (value: unknown) => typeof value === "string" && value.startsWith("PATH="),
  );
  if (typeof imagePath !== "string") fail("selected image PATH missing");
  variables.PATH = join(output, "tools") + ":" + imagePath.slice(5);
  if (!["prepare", "consume", "pending"].includes(operation)) delete variables.GITHUB_OUTPUT;
  delete variables.JEST_WORKER_ID;
  const withheld = new Set([
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "FVOCI_WEB_CONTAINER_LOCAL_IMAGE",
    "FVOCI_WEB_CONTAINER_LOCAL",
  ]);
  const entries = Object.entries(variables).filter(
    (entry): entry is [string, string] => entry[1] !== undefined && !withheld.has(entry[0]),
  );
  if (
    entries.some(([key, value]) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key) || /[\r\n\0]/.test(value))
  )
    fail("selected image environment cannot be encoded privately");
  if (operation === "browser") {
    const cli = join(root, "node_modules/playwright/cli.js");
    if (
      browserArgs[0] !== join(output, "tools/bun") ||
      browserArgs[1] !== "--no-install" ||
      browserArgs[2] !== cli ||
      browserArgs[3] !== "test"
    )
      fail("selected image accepts only the admitted Playwright entrypoint");
  } else if (browserArgs.length) fail("selected operation does not accept commands");
  const args = [
    "docker",
    "run",
    "--pull=never",
    "--platform",
    "linux/amd64",
    "--init",
    "--name",
    name,
    "--user",
    `${String(uid)}:${String(gid)}`,
    "--cpus=2",
    "--memory=16g",
    "--memory-swap=16g",
    "--pids-limit=1024",
    "--cap-drop=ALL",
    "--security-opt=no-new-privileges",
    "--network=host",
    "--env-file",
    envFile,
    "--workdir",
    operation === "browser" ? join(root, "apps/web") : root,
    "--interactive",
  ];
  for (const group of actor?.groups ?? (uid === 1000 ? (process.getgroups?.() ?? []) : [gid])) {
    if (group === 0) fail("selected image refuses privileged supplementary groups");
    if (group !== gid) args.push("--group-add", String(group));
  }
  const mounts = [root, output, join(temp, "fvoci-sqlite"), join(temp, "fvoci-image-cargo")];
  if (["prepare", "consume"].includes(operation)) {
    if (process.env.FVOCI_WEB_BUILD_HANDOFF !== join(temp, "fvoci-web-build-handoff"))
      fail("selected packet canonical path required");
    mounts.push(process.env.FVOCI_WEB_BUILD_HANDOFF);
  }
  if (variables.GITHUB_OUTPUT) {
    const path = realpathSync(variables.GITHUB_OUTPUT);
    assert.ok(below(path, temp), "owned CI output must remain within runner temp");
    if (!lstatSync(path).isFile() || statSync(path).uid !== owner)
      fail("owned CI output file required");
    mounts.push(path);
  }
  for (const path of mounts) {
    if (path.includes(",") || path === "/" || path === "/var/run")
      fail("selected bind path refused");
    if (realpathSync(path) !== path || lstatSync(path).isSymbolicLink())
      fail("selected bind must be physical");
    const readonly =
      (path === root && !["prepare", "consume"].includes(operation)) ||
      (path === process.env.FVOCI_WEB_BUILD_HANDOFF && operation === "consume") ||
      (path === join(temp, "fvoci-image-cargo") && !["prepare", "consume"].includes(operation));
    args.push("--mount", `type=bind,source=${path},target=${path}${readonly ? ",readonly" : ""}`);
  }
  selectedImageBudget(process.env.FVOCI_SELECTED_IMAGE_DEADLINE, Date.now());
  // Bun's node shims and package-cache version indexes contain symlinks.
  // Keep both on owned tool /tmp outside output; TMPDIR/evidence stay put.
  // The later actor owns output but only traverses RUNNER_TEMP; /var/tmp also
  // permits that actor's private allocation without widening parent access.
  const nodeTemp = actor ? undefined : mkdtempSync("/var/tmp/fvoci-selected-tool.");
  if (nodeTemp) args.push("--mount", `type=bind,source=${nodeTemp},target=/tmp`);
  else args.push("--tmpfs", "/tmp:rw,mode=0700,uid=1000,gid=1000");
  args.push(
    sdk.image,
    ...(operation === "browser"
      ? browserArgs
      : [
          "bun",
          "--no-env-file",
          join(root, "tools/web-e2e/run-web-e2e.ts"),
          "selected-image-phase",
          root,
          operation,
        ]),
  );
  try {
    writeFileSync(envFile, entries.map(([key, value]) => `${key}=${value}\n`).join(""), {
      flag: "wx",
      mode: 0o600,
    });
  } catch (error) {
    rmSync(envFile, { force: true });
    if (nodeTemp) rmSync(nodeTemp, { recursive: true });
    throw error;
  }
  return { args, name, envFile, nodeTemp };
}

export function finishToolImage(name: string, code: number) {
  const inspected = Bun.spawnSync(
    [
      "docker",
      "inspect",
      "--format",
      "{{.State.Running}} {{.State.ExitCode}} {{.State.OOMKilled}}",
      name,
    ],
    { stdout: "pipe", stderr: "pipe" },
  );
  if (inspected.exitCode !== 0) return { code: code || 1, inspected, removeExit: -1 };
  const state = inspected.stdout.toString().trim();
  if (state !== `false ${String(code)} false`) code ||= 1;
  if (!state.startsWith("false ")) {
    Bun.spawnSync(["docker", "stop", "--time", "10", name], { stdout: "pipe", stderr: "pipe" });
    code ||= 1;
  }
  const removed = Bun.spawnSync(["docker", "rm", "-v", name], { stdout: "pipe", stderr: "pipe" });
  if (removed.exitCode !== 0) code ||= 1;
  return { code, inspected, removeExit: removed.exitCode };
}

export async function selectedImageResult(
  operation: SelectedImageOperation,
  root: string,
  options: CommandOptions = {},
  browserArgs: string[] = [],
  actor?: { groups: number[] },
): Promise<Completed> {
  const cancellation = new AbortController();
  let cancelled = 0;
  const interrupt = () => {
    cancelled ||= 130;
    cancellation.abort(new RuntimeExit(cancelled));
  };
  const terminate = () => {
    cancelled ||= 143;
    cancellation.abort(new RuntimeExit(cancelled));
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  let result: Completed | undefined,
    failure: unknown,
    retirementCode = 1,
    retirementPending = false,
    temporaryRemovable = true;
  let invocation: ReturnType<typeof selectedImageInvocation> | undefined;
  try {
    invocation = selectedImageInvocation(
      operation,
      root,
      options.env ?? process.env,
      browserArgs,
      actor,
    );
    if (options.log && !existsSync(options.log)) closeSync(openSync(options.log, "wx", 0o600));
    const deadline = AbortSignal.timeout(
      selectedImageBudget(process.env.FVOCI_SELECTED_IMAGE_DEADLINE, Date.now()),
    );
    const signal = AbortSignal.any([
      cancellation.signal,
      deadline,
      ...(options.signal ? [options.signal] : []),
    ]);
    const fd = options.log ? openSync(options.log, "w", 0o600) : undefined;
    try {
      const materialize = ["prepare", "consume"].includes(operation),
        browserSource = join(
          process.env.FVOCI_SELECTED_CI_OUTPUT ?? fail("selected output required"),
          "browser-source",
        ),
        started = performance.now();
      const commands = materialize
        ? [
            invocation.args.map((arg, at) => (at === 1 ? "create" : arg)),
            ["docker", "cp", invocation.name + ":/opt/playwright/.", browserSource],
            invocation.args,
          ]
        : [invocation.args];
      if (materialize) mkdirSync(browserSource, { mode: 0o700 });
      for (const [at, args] of commands.entries()) {
        if (!materialize || at !== 1) {
          retirementPending = true;
          temporaryRemovable = false;
        }
        const child = spawnSelectedCommand(
          args,
          process.env,
          fd ?? "pipe",
          fd ?? "pipe",
          signal,
          options.input,
        );
        const bytes = async (stream: typeof child.stdout) => {
          if (typeof stream === "number") fail("selected image stream missing");
          return await new Response(stream).arrayBuffer();
        };
        const [stdout, stderr, exit] = await Promise.allSettled([
          fd === undefined ? bytes(child.stdout) : Promise.resolve(new ArrayBuffer(0)),
          fd === undefined ? bytes(child.stderr) : Promise.resolve(new ArrayBuffer(0)),
          child.exited,
        ]);
        signal.throwIfAborted();
        if (
          stdout.status === "rejected" ||
          stderr.status === "rejected" ||
          exit.status === "rejected"
        ) {
          const failed = [stdout, stderr, exit].find((value) => value.status === "rejected");
          if (failed?.status === "rejected" && failed.reason instanceof Error) throw failed.reason;
          fail("selected image stream or exit collection failed");
        }
        const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
        result = {
          returncode: observedExit({
            exitCode: exit.value,
            signalCode: child.signalCode ?? undefined,
          }),
          stdout: decoder.decode(stdout.value),
          stderr: decoder.decode(stderr.value),
        };
        if (result.returncode !== 0) break;
        if (materialize && at === 1) {
          retirementPending = false;
          const closed = finishToolImage(invocation.name, 0);
          temporaryRemovable = closed.removeExit === 0;
          if (closed.code !== 0) throw new RuntimeExit(closed.code);
          let bytes = 0;
          for (const path of [browserSource, ...files(browserSource, true)]) {
            const facts = lstatSync(path);
            assert.ok(facts.uid === uid() && facts.gid === gid() && !facts.isSymbolicLink());
            if (facts.isFile()) bytes += facts.size;
          }
          assert.equal(statSync(browserSource).mode & 0o777, 0o700);
          process.stderr.write(
            "selected-image browser_materialization elapsed_seconds=" +
              ((performance.now() - started) / 1000).toFixed(3) +
              " bytes=" +
              String(bytes) +
              " exit=0\n",
          );
        }
      }
    } finally {
      if (fd !== undefined) closeSync(fd);
    }
  } catch (error) {
    failure = error;
  } finally {
    if (invocation) {
      if (result?.returncode && !options.log) {
        try {
          writeFileSync(
            join(dirname(invocation.envFile), invocation.name + ".private.log"),
            result.stderr,
            { flag: "wx", mode: 0o600 },
          );
        } catch {
          process.stderr.write("selected image private failure capture failed\n");
        }
        if (operation === "inputs")
          try {
            for (const line of driverFailureDiagnostics(result.stderr))
              process.stderr.write(line + "\n");
          } catch {
            // Keep the original image exit and mandatory retirement.
          }
      }
      retirementCode = result?.returncode ?? (cancelled || 1);
      try {
        if (retirementPending) {
          const closed = finishToolImage(invocation.name, retirementCode);
          retirementCode = closed.code;
          temporaryRemovable = closed.removeExit === 0;
        }
      } catch {
        retirementCode ||= 1;
        process.stderr.write("selected image retirement unconfirmed\n");
      }
      try {
        rmSync(invocation.envFile, { force: true });
      } catch {
        retirementCode ||= 1;
        process.stderr.write("selected image private environment removal failed\n");
      }
      if (temporaryRemovable && invocation.nodeTemp)
        try {
          rmSync(invocation.nodeTemp, { recursive: true });
        } catch {
          retirementCode ||= 1;
          process.stderr.write("selected image private node temporary removal failed\n");
        }
    }
    try {
      if (options.log && existsSync(options.log)) chmodSync(options.log, 0o600);
    } catch {
      retirementCode ||= 1;
      process.stderr.write("selected image private log mode unconfirmed\n");
    }
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
  }
  if (cancelled && result === undefined) throw new RuntimeExit(cancelled);
  if (failure instanceof Error) throw failure;
  if (failure !== undefined) fail("selected image operation threw an unsupported failure");
  if (!result) fail("selected image child exit missing");
  if (retirementCode && options.required !== false)
    fail(
      `owned command failed exit=${String(retirementCode)}; executable=docker; selected=${operation}`,
    );
  return { ...result, returncode: retirementCode };
}

// Existing full Inputs shape, checked by the same inputCheck in the actual
// compiler namespace. No compile receipt is regenerated.
export async function selectedImageInputs(before: Inputs, root: string): Promise<Inputs> {
  const result = await selectedImageResult("inputs", root, { input: JSON.stringify(before) });
  return JSON.parse(result.stdout) as Inputs;
}

export async function selectedImageAccess(
  request: { groups: number[]; files: Record<string, number> },
  root: string,
  actor = false,
) {
  return selectedImageResult(
    "access",
    root,
    { input: JSON.stringify(request) },
    [],
    actor ? { groups: request.groups } : undefined,
  );
}

export async function selectedImageConfigList(root: string): Promise<number> {
  return (await selectedImageResult("config-list", root, { required: false })).returncode;
}

export async function selectedImageBrowser(
  args: string[],
  options: CommandOptions,
  root: string,
): Promise<Completed> {
  return selectedImageResult("browser", root, options, args);
}

export function containerImage(
  image: string,
  sdk: string,
  local: boolean,
  env: NodeJS.ProcessEnv,
): { image: string; sdk: string; local: boolean } {
  if (!/^[a-f0-9]{64}$/.test(sdk)) fail("an explicit sealed SQLite SDK SHA-256 is required");
  if (
    !(
      local ? /^sha256:[a-f0-9]{64}$/ : /^ghcr\.io\/aisflow\/fvoci-ci-web@sha256:[a-f0-9]{64}$/
    ).test(image) ||
    (local && (env.CI !== undefined || Object.keys(env).some((key) => key.startsWith("GITHUB_"))))
  )
    fail("registry image digest required; local engine IDs require explicit non-CI local SDK mode");
  return { image, sdk, local };
}

export class RuntimeExit extends Error {
  constructor(readonly status: number) {
    super("web e2e command failed");
  }
}

export function dockerEndpoint(root: string): string {
  let endpoint: unknown = process.env.DOCKER_HOST;
  if (process.env.DOCKER_CONTEXT || !endpoint) {
    const json = checkedOutput(
      [
        "docker",
        "context",
        "inspect",
        "--format",
        "{{json .Endpoints.docker.Host}}",
        process.env.DOCKER_CONTEXT || checkedOutput(["docker", "context", "show"], root),
      ],
      root,
    );
    try {
      endpoint = JSON.parse(json);
    } catch {
      fail("invalid Docker endpoint metadata");
    }
  }
  if (typeof endpoint !== "string") fail("a local Docker endpoint is required");
  return endpoint;
}

export function checkedOutput(argv: string[], cwd: string): string {
  const result = Bun.spawnSync(argv, { cwd, env: process.env, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0 || result.signalCode) fail("container browser prerequisite failed");
  return result.stdout.toString().trim();
}

export function containerArchitecture(
  system: string,
  runtime: string,
  kernel: string,
  darwinArm?: string,
): NativeArchitecture {
  if (system !== "linux" && system !== "darwin")
    fail("a Linux or Darwin Docker client is required");
  if (system === "darwin" && darwinArm !== "0" && darwinArm !== "1")
    fail("physical Darwin architecture is required");
  const physical = nativeArchitecture(
    system === "darwin" ? (darwinArm === "1" ? "arm64" : "x64") : kernel,
  );
  if (nativeArchitecture(runtime) !== physical || nativeArchitecture(kernel) !== physical)
    fail("translated Docker clients are unsupported");
  return physical;
}

export function qualifyContainerDaemon(
  endpoint: string,
  info: string,
  arch: NativeArchitecture,
): void {
  if (!/^unix:\/\/\/[^\s?#\0]+$/.test(endpoint)) fail("a local Unix Docker endpoint is required");
  const match = /^linux (\S+)$/.exec(info);
  if (!match || nativeArchitecture(match[1] as string) !== arch)
    fail("Docker requires a native Linux daemon matching the physical client architecture");
}

export function containerImageSource(
  metadata: string,
  sdk: { image: string; local: boolean },
  arch: NativeArchitecture,
): string {
  const match = /^(sha256:[a-f0-9]{64}) linux (\S+) ([a-f0-9]{40}) (.+)$/.exec(metadata);
  if (!match || nativeArchitecture(match[2] as string) !== arch)
    fail("the pinned native ci-web image must match the client and daemon");
  let digests: unknown;
  try {
    digests = JSON.parse(match[4] as string);
  } catch {
    fail("invalid image digest metadata");
  }
  if (sdk.local ? match[1] !== sdk.image : !Array.isArray(digests) || !digests.includes(sdk.image))
    fail("the actual immutable image identity is required");
  return match[3] as string;
}

export function qualifyImageSource(
  root: string,
  metadata: string,
  sdk: { image: string; sdk: string; local: boolean },
  arch: NativeArchitecture,
  current: string,
): string {
  const imageSource = containerImageSource(metadata, sdk, arch);
  if (![imageSource, current].every((sha) => /^[a-f0-9]{40}$/.test(sha)))
    fail("exact image and consumer source revisions required");
  const published = !sdk.local && sdk.image === CI_WEB_IMAGE;
  if (published && (imageSource !== CI_WEB_IMAGE_SOURCE || sdk.sdk !== CI_WEB_SDK_SHA256[arch]))
    fail("the selected publication source and native SDK seal must match its pinned image");
  checkedOutput(
    [
      "git",
      "diff",
      "--quiet",
      imageSource,
      current,
      "--",
      // The selected immutable publication contains its own qualified recipe.
      // Future recipe edits cannot change it; all copied/compiler inputs below
      // must still match. Overrides retain full current-recipe qualification.
      ...(published ? [] : ["docker/ci-base/Dockerfile"]),
      "xtask/src",
      "xtask/Cargo.toml",
      "xtask/Cargo.lock",
      ":(glob)xtask/*.tsv",
      "tests/support/official_sqld.rs",
      "rust-toolchain.toml",
      ".bun-version",
    ],
    root,
  );
  return imageSource;
}

/** A refusal: the CLI prints the message to stderr and exits 1. */
export class CheckError extends Error {}

export function fail(message: string): never {
  throw new CheckError(message);
}
