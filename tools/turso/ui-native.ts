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
import { root as checkout, sha } from "../selected-backend-ci/io.ts";
import { baselineFailureDiagnostic } from "./ui-audit.ts";
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
  stopAttachedDaemon,
  type ServerHandle,
} from "./ui-container.ts";
import { communicate, identity, now, poll, wait, type Scope } from "./ui-processes.ts";
import type { Manifest } from "./ui-record.ts";
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
];
const nativeCode = /^(?:TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED)$/;

export async function fixture(
  scope: Scope | null,
  manifest: Pick<Manifest, "binaries">,
  mode: string,
  environment: Record<string, string>,
  input?: Record_,
): Promise<Record_> {
  if (executionMode() === "orca-local")
    return localFixture(scope as Scope, manifest, mode, environment, input);
  const env = cleanEnv();
  for (const key of FIXTURE_KEYS)
    if (Object.hasOwn(environment, key)) env[key] = environment[key] as string;
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
}

export async function start(
  scope: Scope,
  manifest: Pick<Manifest, "binaries">,
  environment: Record<string, string>,
  directory: string,
  expectedSetup = false,
  clock: () => number = now,
): Promise<Started> {
  if (executionMode() === "orca-local")
    return localServerStart(scope, manifest, environment, directory, expectedSetup);
  const logpath = join(directory, "server.private.log");
  const log = openSync(logpath, "wx", 0o600);
  fchmodSync(log, 0o600);
  const launch = get(manifest, "binaries", "fvoci-migrate", "path") as string;
  const child = scope.spawn([launch, "--start"], "server", {
    env: environment,
    stdin: "ignore",
    stdout: log,
    stderr: log,
  });
  const started = clock();
  const deadline = started + SERVER_BUDGET;
  try {
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
    return { server: { child }, base, log };
  } catch (original) {
    const diagnostic =
      failureCode(original) === "UI_SERVER_START_FAILED"
        ? publishStartDiagnostic(scope, child, logpath, started, deadline, clock)
        : null;
    const cleanup: string[] = [];
    try {
      await scope.finish(child, true);
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
];

export async function browser(
  scope: Scope,
  manifest: Pick<Manifest, "bun" | "physicalInputs">,
  directory: string,
  environment: Record<string, string>,
  spec: string,
  grep?: string,
  workspace: string = checkout,
): Promise<unknown> {
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
    pkg.version === "1.63.0" &&
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
    "e2e-pending/collab-playwright.config.ts",
    "--reporter=line,json",
  ];
  if (grep) args.push("--grep", grep);
  args.push("e2e-pending/" + spec);
  const output = openSync(join(directory, "browser.private.log"), "wx", 0o600);
  let code = -1;
  try {
    fchmodSync(output, 0o600);
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
