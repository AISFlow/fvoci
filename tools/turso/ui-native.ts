// Hosted native fixture, normal-main server and browser children. Each child is
// an allocation of the caller's process scope and is finished before return.
import {
  accessSync,
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fchmodSync,
  lstatSync,
  openSync,
  readFileSync,
} from "node:fs";
import { join } from "node:path";
import { root as checkout, sha, digest } from "../selected-backend-ci/io.ts";
import {
  baselineFailureDiagnostic,
  OFF,
  ON,
  RESTORE,
  captureRoomLog,
  type RoomLogOrigin,
} from "./ui-audit.ts";
import { AUTH_SPEC } from "./ui-auth-session.ts";
import { browserFailureDiagnostic } from "./ui-browser-diagnostic.ts";
import {
  fixtureInput,
  cleanEnv,
  diagnosticJson,
  executionMode,
  failureCode,
  get,
  isRecord,
  parseBytes,
  parsePlain,
  privateRead,
  require,
  token,
  UiError,
  type Record_,
  bunIsolation,
} from "./ui-common.ts";
import {
  localFixture,
  localServerStart,
  lease,
  stopAttachedDaemon,
  type ServerHandle,
} from "./ui-container.ts";
import { communicate, identity, now, poll, wait, type Scope } from "./ui-processes.ts";
import {
  loadLocalLease,
  recheckFinalBrowsers,
  type BrowserEngine,
  type Manifest,
} from "./ui-record.ts";
import { awaitListening, portClosed, publishStartDiagnostic, requireSetup } from "./ui-server.ts";
import { SERVER_BUDGET } from "./ui-start-diagnostic.ts";

const FIXTURE_KEYS = [
  "FVOCI_LIBSQL_URL",
  "FVOCI_LIBSQL_AUTH_TOKEN",
  "PASSWORD_PEPPER_KEYS",
  "PASSWORD_PEPPER_ACTIVE_KEY_ID",
  "FVOCI_E2E_TURSO_NAMESPACE",
  "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE",
  "FVOCI_TEST_TURSO_DESTRUCTIVE",
  // The member actor starts with a deliberately minimal environment. Retain
  // the caller's admitted TLS trust file/directory from its private capsule.
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
];
const nativeCode = /^(?:TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED)$/;

export async function fixture(
  scope: Scope | null,
  manifest: Pick<Manifest, "binaries"> & Partial<Pick<Manifest, "finalTrust">>,
  mode: string,
  environment: Record<string, string>,
  input?: Record_,
): Promise<Record_> {
  if (executionMode() === "orca-local")
    return localFixture(
      scope as Scope,
      manifest,
      mode,
      { ...environment, E2E_DATABASE_BACKEND: "libsql-remote", FVOCI_E2E_TURSO_UI_SELECTED: "1" },
      input,
    );
  const env = cleanEnv();
  for (const key of FIXTURE_KEYS)
    if (Object.hasOwn(environment, key)) env[key] = environment[key] as string;
  if (manifest.finalTrust !== undefined) {
    env.SSL_CERT_FILE = manifest.finalTrust.path;
    Reflect.deleteProperty(env, "SSL_CERT_DIR");
  }
  env.E2E_DATABASE_BACKEND = "libsql-remote";
  env.FVOCI_E2E_TURSO_UI_SELECTED = "1";
  require(scope !== null, "UI_OWNED_PROCESS_SCOPE_REQUIRED");
  const binary = get(manifest, "binaries", "fvoci-e2e-fixture", "path") as string;
  const child = scope.spawn([binary, mode], "fixture-" + mode, {
    env,
    stdin: "pipe",
    stdout: "pipe",
    stderr: "ignore",
  });
  let original: unknown = null,
    value: Record_ | undefined;
  const failures: string[] = [];
  try {
    const stdout = await communicate(child, fixtureInput(input), 120);
    const code = poll(child);
    // Observe the bounded native outcome before process finalization can
    // fail. Missing/malformed output never supplies an invented cause.
    require(stdout.length < 64 * 1024 * 1024, "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
    // Source tokens are kept only for the failure projection that checks integer fields.
    const parsed = code !== 0 ? parseBytes(stdout) : parsePlain(stdout);
    require(isRecord(parsed), "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
    if (code !== 0) {
      const failure = Object.hasOwn(parsed, "originalFailure")
        ? parsed.originalFailure
        : "UI_NATIVE_FIXTURE_FAILED";
      require(typeof failure === "string" &&
        nativeCode.test(failure), "UI_NATIVE_FAILURE_CODE_REFUSED");
      if (mode === "baseline" && failure === "TURSO_UI_BASELINE_FAILED") {
        try {
          scope.io.output.err(diagnosticJson(baselineFailureDiagnostic(parsed)));
        } catch {
          failures.push("UI_NATIVE_BASELINE_DIAGNOSTIC_WRITE_FAILED");
        }
      }
      try {
        scope.io.write(join(scope.io.root(), "fixture-failure-" + token(6) + ".private.json"), {
          mode,
          exit: code,
          receipt: parsed,
        });
      } catch {
        failures.push("UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED");
      }
      throw new UiError(failure);
    }
    require(get(parsed, "lifecycleDrain") === "confirmed" &&
      get(parsed, "leases") === 0 &&
      get(parsed, "serverCloseReceipt") === "not-exposed-by-sdk", "UI_NATIVE_DRAIN_FAILED");
    value = parsed;
  } catch (error) {
    original = error;
  }
  try {
    await scope.finish(child);
  } catch (cleanup) {
    failures.push(failureCode(cleanup));
  }
  if (failures.length) {
    try {
      scope.io.output.err(
        diagnosticJson({
          originalFailure: original !== null ? failureCode(original) : null,
          nativeCleanupErrors: failures,
        }),
      );
    } catch {
      // The original failure stays the result.
    }
  }
  if (original !== null) throw original as Error;
  if (failures.length) throw new UiError("UI_PROCESS_CLOSURE_FAILED");
  return value as Record_;
}

export interface Started {
  server: ServerHandle;
  base: string;
  log: number;
  roomLog?: RoomLogOrigin;
}

/** The maintained same-PID launcher/publisher evidence, with the actual stdout FD. */
export function serverRoomLog(
  scope: Scope,
  manifest: Pick<Manifest, "sourceInputs" | "binaries">,
  started: Started,
  directory: string,
  target: string,
): RoomLogOrigin {
  const server = started.server,
    retired = server.hostIdentity ?? identity(server.child.pid);
  const observed = identity(retired.pid);
  const native = server.maintenanceIdentity ?? retired;
  require(observed.startTicks === retired.startTicks &&
    observed.comm === "fvoci-server" &&
    (observed.exeInspection === "UNAVAILABLE" ||
      observed.exe ===
        (server.containerId === undefined
          ? manifest.binaries["fvoci-server"]?.path
          : "/fvoci-current/bin/fvoci-server")), "UI_NORMAL_MAIN_IDENTITY_FAILED");
  if (server.containerId !== undefined)
    require(server.daemonSample !== undefined &&
      server.daemonSample.clientPid === server.child.pid &&
      server.daemonSample.maintenance.pid === native.pid &&
      server.daemonSample.maintenance.startTicks === native.startTicks &&
      server.daemonSample.retirement.pid === retired.pid &&
      server.daemonSample.retirement.startTicks ===
        retired.startTicks, "UI_SERIAL_ROOM_ORIGIN_REQUIRED");
  return captureRoomLog(
    scope,
    manifest,
    join(directory, "server.private.log"),
    started.log,
    server.child,
    native,
    target,
    retired,
  );
}

export async function start(
  scope: Scope,
  manifest: Pick<Manifest, "binaries"> & Partial<Pick<Manifest, "sourceInputs" | "finalBrowsers">>,
  environment: Record<string, string>,
  directory: string,
  expectedSetup = false,
  clock: () => number = now,
): Promise<Started> {
  if (executionMode() === "orca-local")
    return localServerStart(scope, manifest, environment, directory, expectedSetup);
  const logpath = join(directory, "server.private.log");
  const log = openSync(logpath, "wx", 0o600);
  let child: ReturnType<Scope["spawn"]> | undefined;
  let started: number | undefined, deadline: number | undefined;
  try {
    fchmodSync(log, 0o600);
    const launch = get(manifest, "binaries", "fvoci-migrate", "path") as string;
    child = scope.spawn([launch, "--start"], "server", {
      env: environment,
      stdin: "ignore",
      stdout: log,
      stderr: log,
    });
    started = clock();
    deadline = started + SERVER_BUDGET;
    const base = await awaitListening(logpath, child, deadline, clock);
    const observed = identity(child.pid);
    require(observed.comm === "fvoci-server" &&
      (observed.exeInspection === "UNAVAILABLE" ||
        observed.exe ===
          get(manifest, "binaries", "fvoci-server", "path")), "UI_NORMAL_MAIN_IDENTITY_FAILED");
    scope.io.write(join(directory, "launch-identity.private.json"), {
      observed,
      launchSha256: get(manifest, "binaries", "fvoci-migrate", "sha256"),
      serverSha256: get(manifest, "binaries", "fvoci-server", "sha256"),
      execContract: "maintained migrate adjacent-server same-PID exec",
      kernelExeProof: observed.exeInspection,
    });
    await requireSetup(base, expectedSetup);
    const running: Started = { server: { child }, base, log };
    if (manifest.finalBrowsers !== undefined) {
      require(manifest.sourceInputs !== undefined, "UI_SERIAL_ROOM_ORIGIN_REQUIRED");
      running.roomLog = serverRoomLog(
        scope,
        { binaries: manifest.binaries, sourceInputs: manifest.sourceInputs },
        running,
        directory,
        digest(environment.FVOCI_LIBSQL_URL ?? ""),
      );
    }
    return running;
  } catch (original) {
    const diagnostic =
      failureCode(original) === "UI_SERVER_START_FAILED" &&
      child !== undefined &&
      started !== undefined &&
      deadline !== undefined
        ? publishStartDiagnostic(scope, child, logpath, started, deadline, clock)
        : null;
    const cleanup: string[] = [];
    try {
      if (child !== undefined) await scope.finish(child, true);
    } catch (error) {
      cleanup.push(failureCode(error));
    }
    try {
      closeSync(log);
    } catch {
      cleanup.push("UI_SERVER_LOG_CLOSE_FAILED");
    }
    try {
      const receipt: Record_ = { originalFailure: failureCode(original), cleanupErrors: cleanup };
      if (diagnostic !== null) receipt.startDiagnostic = diagnostic;
      scope.io.write(join(directory, "start-failure.private.json"), receipt);
    } catch {
      scope.io.output.err(
        diagnosticJson({
          originalFailure: failureCode(original),
          cleanupErrors: cleanup,
          receiptWrite: "failed",
        }),
      );
    }
    throw original as Error;
  }
}

export async function stop(
  scope: Scope,
  server: ServerHandle,
  base: string,
  directory: string,
  closed: (base: string) => Promise<boolean> = portClosed,
): Promise<Record_> {
  if (server.containerId !== undefined) return stopAttachedDaemon(scope, server, base, directory);
  const allocation = scope.allocations.findIndex((a) => a.process === server.child);
  if (allocation < 0) throw new TypeError("unknown allocation");
  const code = await scope.finish(server.child, true);
  const rows = [...scope.entries.values()]
    .filter((e) => e.allocation === allocation || e.allocation === null)
    .map((e) => e.identity);
  require(code === 0 &&
    rows.length > 0 &&
    rows.every((row) => scope.retired(row)) &&
    (await closed(base)), "UI_SERVER_CLOSURE_FAILED");
  const stopped = { serverExit: code, portClosed: true, recordedIdentitiesRetired: true };
  scope.io.write(join(directory, "server-identities-" + token(6) + ".private.json"), {
    stopped,
    identities: rows,
  });
  return stopped;
}

const regularReadable = (path: string) => {
  try {
    const facts = lstatSync(path);
    if (facts.isSymbolicLink() || !facts.isFile()) return false;
    accessSync(path, constants.R_OK);
    return true;
  } catch {
    return false;
  }
};
const SECRET_BROWSER_KEYS = [
  "FVOCI_LIBSQL_URL",
  "FVOCI_LIBSQL_AUTH_TOKEN",
  "DATABASE_URL",
  "DATABASE_APP_URL",
  "PASSWORD_PEPPER_KEYS",
  "ENCRYPTION_KEYS",
];

export async function browser(
  scope: Scope,
  manifest: Pick<Manifest, "bun" | "physicalInputs" | "finalBrowsers" | "finalBrowserRuntime"> &
    Partial<Pick<Manifest, "sourceInputs">>,
  directory: string,
  environment: Record<string, string>,
  spec: string,
  grep?: string,
  workspace: string = checkout,
  config = "collab-playwright.config.ts",
  heavy?: string,
  engine?: BrowserEngine,
): Promise<unknown> {
  const flow = environment.FVOCI_E2E_SELECTED_FLOW;
  const final =
    config === "selected-final-playwright.config.ts" && spec === "selected-final-acceptance.ts";
  const equal = config === "equal-work-playwright.config.ts" && spec === "v060-equal-work.spec.ts";
  // The dedicated auth spec has its own config and exactly its three cases.
  require((config === "collab-playwright.config.ts" &&
    spec !== AUTH_SPEC &&
    spec !== "selected-final-acceptance.ts") ||
    (spec === RESTORE && config === "turso-restore-playwright.config.ts") ||
    (spec === AUTH_SPEC &&
      config === "auth-session-playwright.config.ts" &&
      grep === undefined &&
      (flow === "on" || flow === "off")) ||
    final ||
    (equal &&
      manifest.finalBrowsers !== undefined &&
      environment.FVOCI_E2E_EQUAL_WORK_PREPARED === "1"), "UI_BROWSER_CONFIG_REFUSED");
  require(final
    ? engine !== undefined
    : engine === undefined, "UI_FINAL_BROWSER_SELECTION_REQUIRED");
  let finalBindingHash: string | undefined;
  if (final) {
    recheckFinalBrowsers(manifest);
    const bindingPath = environment.FVOCI_E2E_FINAL_ACTOR_BINDING;
    require(typeof bindingPath === "string", "UI_FINAL_ACTOR_BINDING_REQUIRED");
    const binding = privateRead(bindingPath, 16384);
    require(get(binding, "browser") === engine &&
      manifest.sourceInputs !== undefined &&
      get(binding, "source") === manifest.sourceInputs.source &&
      get(binding, "tree") === manifest.sourceInputs.tree &&
      environment.FVOCI_E2E_FINAL_BROWSER === engine, "UI_FINAL_BROWSER_SELECTION_CHANGED");
    finalBindingHash = sha(bindingPath);
  }
  if (manifest.finalBrowsers !== undefined) recheckFinalBrowsers(manifest);
  const report = join(directory, "playwright.private.json");
  const env: Record<string, string> = {
    ...cleanEnv(),
    ...environment,
    CI: "true",
    FVOCI_E2E_RESULT_DIR: directory,
    PLAYWRIGHT_JSON_OUTPUT_FILE: report,
  };
  // A Bun test worker's id must not reach the Playwright child.
  Reflect.deleteProperty(env, "JEST_WORKER_ID");
  if (final && engine !== undefined) {
    env.FVOCI_E2E_FINAL_BROWSER_BIN = manifest.finalBrowsers?.[engine].path ?? "";
  } else if (manifest.finalBrowsers !== undefined) {
    env.FVOCI_E2E_FINAL_BROWSER_BIN = manifest.finalBrowsers.chromium.path;
  }
  if (manifest.finalBrowsers !== undefined) env.FVOCI_TURSO_UI_FINAL = "1";
  require(!SECRET_BROWSER_KEYS.some((key) =>
    Object.hasOwn(env, key),
  ), "UI_BROWSER_SECRET_ENV_REFUSED");
  // The official CLI from the already admitted physical input closure.
  const physical = manifest.physicalInputs;
  require(sha(physical.path) === physical.sha256, "UI_PHYSICAL_RECEIPT_CHANGED");
  const admitted = get(privateRead(physical.path, 64 * 1024 * 1024), "files", "external");
  require(isRecord(admitted), "UI_PLAYWRIGHT_CLI_REFUSED");
  const cli = join(workspace, "node_modules/playwright/cli.js");
  const packagePath = join(workspace, "node_modules/playwright/package.json");
  for (const path of [cli, packagePath]) {
    require(regularReadable(path), "UI_PLAYWRIGHT_CLI_REFUSED");
    require(admitted[path] === sha(path), "UI_PLAYWRIGHT_CLI_REFUSED");
  }
  const pkg = JSON.parse(readFileSync(packagePath, "utf8")) as unknown;
  require(isRecord(pkg) &&
    pkg.version === "1.64.0" &&
    isRecord(pkg.bin) &&
    pkg.bin.playwright === "cli.js", "UI_PLAYWRIGHT_CLI_REFUSED");
  // The checked env is the whole env: no .env or bunfig of apps/web is loaded.
  const args = [
    manifest.bun.path,
    ...bunIsolation(workspace),
    "--no-install",
    cli,
    "test",
    "--config",
    "e2e-pending/" + config,
    "--reporter=line,json",
  ];
  if (grep) args.push("--grep", grep);
  args.push("e2e-pending/" + spec);
  const output = openSync(join(directory, "browser.private.log"), "wx", 0o600);
  let code = -1;
  try {
    fchmodSync(output, 0o600);
    if (executionMode() === "orca-local") {
      const grant = loadLocalLease("current-build", lease.load);
      require(typeof heavy === "string" &&
        get(grant, "heavy", "path") === heavy, "UI_LOCAL_ALLOCATION_REFUSED");
      args.unshift(heavy);
    }
    const child = scope.spawn(args, "browser", {
      env,
      cwd: join(workspace, "apps/web"),
      stdin: "inherit",
      stdout: output,
      stderr: output,
    });
    let original: unknown = null;
    try {
      code = await wait(child, 900);
      require(code === 0, "UI_ACTUAL_BROWSER_FAILED");
    } catch (error) {
      original = error;
      // Never forward the browser log, error message, URL or attachments. The
      // report is complete after wait observes exit; a missing/invalid report
      // still produces a fixed diagnostic without replacing the failure.
      let failedReport: unknown = null;
      let source = "";
      try {
        if (regularReadable(report)) {
          chmodSync(report, 0o600);
          failedReport = privateRead(report, 64 * 1024 * 1024);
        }
        if ([ON, OFF, AUTH_SPEC].includes(spec))
          source = readFileSync(join(workspace, "apps/web/e2e-pending", spec), "utf8");
      } catch {
        // Malformed private input never supplies invented details.
      }
      try {
        scope.io.output.err(
          diagnosticJson(
            browserFailureDiagnostic(
              failedReport,
              spec,
              spec === OFF || (spec === AUTH_SPEC && flow === "off")
                ? "off"
                : grep === "selected normal main restart:"
                  ? "restart"
                  : "on",
              source,
            ),
          ),
        );
      } catch {
        // Diagnostic I/O cannot erase the original browser failure or cleanup.
      }
    }
    try {
      await scope.finish(child);
    } catch (cleanup) {
      scope.io.output.err(
        diagnosticJson({
          originalFailure: original !== null ? failureCode(original) : null,
          browserProcessClosure: failureCode(cleanup),
        }),
      );
      if (original === null) throw cleanup;
    }
    if (original !== null) throw original as Error;
  } finally {
    closeSync(output);
  }
  if (existsSync(report)) chmodSync(report, 0o600);
  require(code === 0, "UI_ACTUAL_BROWSER_FAILED");
  if (final) {
    recheckFinalBrowsers(manifest);
    require(sha(environment.FVOCI_E2E_FINAL_ACTOR_BINDING as string) ===
      finalBindingHash, "UI_FINAL_ACTOR_BINDING_CHANGED");
  }
  if (manifest.finalBrowsers !== undefined) recheckFinalBrowsers(manifest);
  return JSON.parse(readFileSync(report, "utf8"));
}

export function registeredTitles(
  spec: string,
  flow: "on" | "off",
  workspace: string = checkout,
): string[] {
  const source = readFileSync(join(workspace, "apps/web/e2e-pending", spec), "utf8");
  let titles = [...source.matchAll(/^(?: {2})?test\("([^"\n]+)"/gm)].map((m) => m[1] as string);
  if (flow === "on")
    titles = titles.filter((title) => !title.startsWith("selected normal main restart:"));
  require(titles.length === (flow === "on" ? 1 : 8), "UI_EXPECTED_REGISTRATION_CHANGED");
  return titles;
}
