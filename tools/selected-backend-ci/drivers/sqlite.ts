// Root-owned, one-shot current development SQLite normal-main/current Vue lane
// driver: one owned Ubuntu container runs migrate --start against a new owned
// database and storage, then the selected Vue spec, then the same-app restart.
// No compilation, package install, product patch, migration reset or
// PostgreSQL credential. Prints only its final JSON summary on stdout and
// exits with the lane's first failure code.
import { deepEquals } from "bun";
import { Database } from "bun:sqlite";
import { strict as assert } from "node:assert";
import { randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import process from "node:process";
import { env, isFile, read, root, sha } from "../io.ts";
import type { Environment } from "../io.ts";
import type { Browser, Inputs } from "../types.ts";
import { loadCurrent, validateOffReport, type Current } from "./binding.ts";
import {
  actor,
  assertNoEnvFile,
  checkInterrupt,
  cleanupAttempt,
  cleanupScope,
  command,
  copiedHashes,
  emit,
  failureCheckpoint,
  failureDigest,
  identityGone,
  inodeOf,
  inputCheck,
  list,
  now,
  ownedRows,
  portClosed,
  privateText,
  probeSetup,
  readText,
  running,
  runtimeError,
  secondary,
  shellQuote,
  spawnServer,
  trapInterrupts,
  treeHashes,
  waitFor,
  writeJson,
  type Child,
  type Inode,
  type Receipt,
  type Row,
} from "./common.ts";
import { browserArgs } from "./postgres.ts";
import {
  browserPort,
  restartSameApp,
  type RestartContext,
  type Seam as RestartSeam,
} from "./restart.ts";

export const sqliteDriver = import.meta.path;
export const image =
  "ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7";
// Public preparation checkpoints: one fixed name per owned preparation step.
// The runner publishes only `checkpointPrefix + <one of these>`; never a stack
// frame, path, argv or message.
export const checkpointPrefix = "tools/selected-backend-ci/drivers/sqlite.ts#";
export const preparationSteps = [
  "container-create",
  "container-start",
  "runtime-abi",
  "copy-inputs",
  "own-inputs",
  "mode-executables",
  "runtime-ldd",
  "runtime-ldd-log",
  "runtime-ldd-dependencies",
  "mode-environment",
  "copy-static",
  "copied-owned-files",
  "copied-hashes",
  "copied-hashes-log",
  "copied-hashes-match",
  "network-mode",
  "network-attachments",
  "network-attachment-count",
  "network-attachment-id",
  "network-driver",
  "network-driver-log",
  "network-driver-host",
] as const;
export type PreparationStep = (typeof preparationSteps)[number];
export function publicCheckpoint(value: unknown): string | null {
  return typeof value === "string" &&
    value.startsWith(checkpointPrefix) &&
    (preparationSteps as readonly string[]).includes(value.slice(checkpointPrefix.length))
    ? value
    : null;
}
const checkpointKeys = ["known_driver_checkpoint", "preparation_command_exit"] as const;

// The I/O this driver owns, replaceable as one boundary in tests.
export interface Seam extends RestartSeam {
  trap: () => void;
  loadCurrent: (lane: "sqlite", driver: string) => Promise<Current>;
  inputCheck: (before: Inputs, head: string, tree: string) => Promise<Inputs>;
  treeHashes: (directory: string) => Record<string, string>;
  migrationRows: (db: string) => unknown[][];
  validateOffReport: (report: unknown, backend: "sqlite") => string[];
  restart: (context: RestartContext, seam: RestartSeam) => Promise<Receipt>;
}
export interface State {
  current: Current;
  driver: string;
  run: string;
  dbroot: string;
  storage: string;
  db: string;
  dist: string;
  name: string;
  owner: string;
  head: string;
  tree: string;
  flow: "on" | "off";
  spec: string;
  bun: string;
  server: string;
  migrate: string;
  fixture: string;
  engine: string;
  serverEnv: Record<string, string>;
  before: Inputs;
  sourceBefore: Inputs;
  receipt: Receipt;
  // The product runtime account inside and outside the container.
  identity: readonly [number, number];
  source: Environment;
  step: PreparationStep | null;
  created: boolean;
  serverProcess: Child | null;
  serverLog: number | null;
  base: string | null;
  serverRow: Row | null;
  browserEnv: Record<string, string>;
  browserInputs: Browser | null;
  databaseInode: Inode | null;
  code: number;
}

// ---- pure decisions -------------------------------------------------------

export function serverEnvironment(flow: "on" | "off", hex: () => string): Record<string, string> {
  return {
    PATH: "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: hex() }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: JSON.stringify({ fixture: hex() }),
    ENCRYPTION_ACTIVE_KEY_ID: "fixture",
    FVOCI_DATABASE_BACKEND: "sqlite",
    FVOCI_SQLITE_PATH: "/fvoci/database/app.sqlite",
    FVOCI_BIND: "127.0.0.1:0",
    FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0",
    FVOCI_COOKIE_SECURE: "0",
    STORAGE_DRIVER: "local",
    FVOCI_STORAGE_DIR: "/fvoci/storage",
    FVOCI_COLLAB_ENGINE: "/fvoci/bin/collab-engine",
    FVOCI_STATIC_DIR: "/srv/fvoci-web",
    FVOCI_COLLAB_FAMILY_LEASE_MS: "30000",
    FVOCI_COLLAB_FAMILY_RENEW_MS: "5000",
    FVOCI_COLLAB_MAX_ROOMS: "2",
    RUST_LOG: "info",
    FVOCI_REALTIME_MODE: flow,
  };
}
// `. /fvoci/inputs/environment.sh` reads this inside the container.
export const shellExports = (values: Record<string, string>) =>
  Object.entries(values)
    .map(([key, value]) => `export ${key}=${shellQuote(value)}\n`)
    .join("");
// NetworkMode text is diagnostic only. The sole actual attachment must be
// Docker's host driver, proven by the network's own id and driver.
export const singleAttachment = (networks: unknown) =>
  networks !== null &&
  typeof networks === "object" &&
  !Array.isArray(networks) &&
  Object.keys(networks).length === 1;
export function attachedNetworkId(networks: unknown): string {
  const attachment = Object.values(networks as Record<string, unknown>)[0];
  assert.ok(attachment !== null && typeof attachment === "object" && "NetworkID" in attachment);
  const id = attachment.NetworkID;
  assert.ok(typeof id === "string");
  return id;
}
export const hostDriver = (output: string, networkId: string) =>
  deepEquals(output.trim().split(/\s+/).filter(Boolean), [networkId, "host"]);
const listening = /fvoci-server listening on (http:\/\/127\.0\.0\.1:\d+)/;
export const listenBase = (log: string) => listening.exec(log)?.[1] ?? null;
// Files, compiled registry (name, text, digest) and committed receipts agree.
export function expectedMigrations(
  definitions: readonly { stem: string; sha256: string }[],
  registrySource: string,
): [number, string, string][] {
  const registry = registrySource.split("const SQLITE_STEPS:")[1]?.split("];")[0];
  assert.ok(registry !== undefined);
  const steps = [
    ...registry.matchAll(
      /"([0-9]{2}_[a-z_]+)",\s*include_str!\("\.\.\/\.\.\/migrations\/sqlite\/060\/([0-9]{2}_[a-z_]+)\.sql"\),\s*"([0-9a-f]{64})"/g,
    ),
  ];
  assert.ok(
    deepEquals(
      steps.map((step) => step[1]),
      definitions.map((file) => file.stem),
    ),
  );
  assert.ok(steps.every((step) => step[1] === step[2]));
  assert.ok(
    deepEquals(
      steps.map((step) => step[3]),
      definitions.map((file) => file.sha256),
    ),
  );
  return definitions.map((file, index) => [index + 1, "fvoci-sqlite-060", file.sha256]);
}
export function assertActorReceipt(actor: unknown): void {
  assert.ok(actor !== null && typeof actor === "object");
  const record = actor as Record<string, unknown>;
  assert.ok(record.backend === "sqlite" && record.commit === "confirmed");
  assert.ok(
    record.poolClosed && record.connectionClose === "confirmed" && record.operationSucceeded,
  );
}
// Allowlisted Playwright child environment: no owner DB URL or encryption key.
export function browserEnvironment(
  source: Environment,
  local: boolean,
  values: {
    base: string;
    flow: string;
    run: string;
    fixture: string;
    dbroot: string;
    db: string;
    serverEnv: Record<string, string>;
  },
): Record<string, string> {
  const result: Record<string, string> = {
    TMPDIR: env("TMPDIR", source),
    ...(local ? {} : { CI: "true" }),
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: env("BUN_RUNTIME_TRANSPILER_CACHE_PATH", source),
    PATH: env("PATH", source),
    LANG: source.LANG ?? "C.UTF-8",
    PLAYWRIGHT_BASE_URL: values.base,
    FVOCI_E2E_SELECTED_BACKEND: "sqlite",
    FVOCI_E2E_SELECTED_FLOW: values.flow,
    FVOCI_E2E_RESULT_DIR: values.run,
    PLAYWRIGHT_JSON_OUTPUT_FILE: join(values.run, "playwright-result.private.json"),
    FVOCI_E2E_SELECTED_FIXTURE_BIN: values.fixture,
    FVOCI_E2E_SQLITE_RUN_ROOT: values.dbroot,
    FVOCI_E2E_SQLITE_PATH: values.db,
    PASSWORD_PEPPER_KEYS: env("PASSWORD_PEPPER_KEYS", values.serverEnv),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: env("PASSWORD_PEPPER_ACTIVE_KEY_ID", values.serverEnv),
  };
  if (source.PLAYWRIGHT_BROWSERS_PATH)
    result.PLAYWRIGHT_BROWSERS_PATH = source.PLAYWRIGHT_BROWSERS_PATH;
  return result;
}
export const positiveDockerAbsence = (result: { returncode: number; stderr: string }) =>
  result.returncode !== 0 &&
  ["no such object", "no such container"].some((marker) =>
    result.stderr.toLowerCase().includes(marker),
  );
export const retiredIdentities = (rows: readonly Row[], gone: (row: Row) => boolean) =>
  rows.length > 0 && rows.every(gone);
const summaryKeys = [
  "source",
  "final_exit_code",
  "actual_browser_tests",
  "browser_exit",
  "owned_container_absent",
  "owned_loopback_port_closed",
  "recorded_process_identities_retired",
  "loopback_port_observation",
  "failed_phase",
  "observed_failed_exit",
  "original_body_log_sha256",
  "original_failure_checkpoint_sha256",
  "exact_source_artifact_inputs_unchanged",
];
// Public stdout line: fixed facts and digests only, never the private failure.
export function summary(receipt: Receipt, code: number, cleanupErrors: unknown[]): Receipt {
  const result: Receipt = Object.fromEntries(summaryKeys.map((key) => [key, receipt[key] ?? null]));
  const original = receipt.original_driver_failure;
  return Object.assign(result, {
    failure_code: code ? "SELECTED_DRIVER_FAILED" : null,
    final_exit_code: code,
    cleanup_failure_codes: cleanupErrors,
    original_driver_failure_sha256: original === undefined ? null : failureDigest(original),
  });
}

// ---- owned I/O ------------------------------------------------------------

// Records the first outcome before any cleanup. A preparation exception also
// names its fixed step and the last preparation command exit observed.
export function recordFailure(
  state: State,
  observedExit: number | null,
  failure?: { error: unknown },
  bodyLog?: string,
): void {
  const receipt = state.receipt;
  if (!("original_driver_failure" in receipt) && receipt.phase === "container-prepare") {
    receipt.known_driver_checkpoint =
      failure !== undefined && state.step !== null ? checkpointPrefix + state.step : null;
    receipt.preparation_command_exit = receipt.last_preparation_command_exit ?? null;
  }
  failureCheckpoint(receipt, state.run, observedExit, {
    ...(failure === undefined ? {} : { error: failure.error }),
    bodyLog,
    extraKeys: checkpointKeys,
  });
}

export async function prepareContainer(state: State, seam: Seam): Promise<void> {
  const { receipt, run, name } = state;
  const step = (current: PreparationStep) => {
    state.step = current;
  };
  // Each spawn resets, then records, the observed preparation command exit.
  const prepared = async (args: string[], log?: string) => {
    receipt.last_preparation_command_exit = null;
    const result = await seam.command(args, { log, required: false });
    receipt.last_preparation_command_exit = result.returncode;
    if (result.returncode)
      throw runtimeError(
        `owned command failed exit=${String(result.returncode)}; executable=${String(args[0])}`,
      );
    return result;
  };
  step("container-create");
  // prettier-ignore
  await prepared(["docker", "create", "--name", name, "--network", "host", "--user", "0",
    "--label", "fvoci.owner=" + state.owner, "--label", "fvoci.test-run=v060-current-normal-vue-sqlite",
    "--mount", `type=bind,src=${state.dbroot},dst=/fvoci/database`,
    "--mount", `type=bind,src=${state.storage},dst=/fvoci/storage`,
    "--entrypoint", "/bin/sleep", image, "1800"], join(run, "container-create.log"));
  state.created = true;
  step("container-start");
  await prepared(["docker", "start", name], join(run, "container-start.log"));
  step("runtime-abi");
  // prettier-ignore
  await prepared(["docker", "exec", name, "/bin/sh", "-ec",
    "mkdir -p /fvoci/bin /fvoci/inputs /srv/fvoci-web; chmod 0700 /fvoci/inputs; ldd --version | head -1"],
    join(run, "runtime-abi.log"));
  const copies: [string, string][] = [
    [state.server, "/fvoci/bin/fvoci-server"],
    [state.migrate, "/fvoci/bin/fvoci-migrate"],
    [state.engine, "/fvoci/bin/collab-engine"],
    [join(run, "environment.private.sh"), "/fvoci/inputs/environment.sh"],
  ];
  const destinations = copies.map(([, destination]) => destination);
  const executables = destinations.slice(0, -1);
  step("copy-inputs");
  for (const [source, destination] of copies)
    await prepared(["docker", "cp", source, name + ":" + destination]);
  step("own-inputs");
  await prepared(["docker", "exec", name, "chown", "0:0", ...destinations]);
  step("mode-executables");
  await prepared(["docker", "exec", name, "chmod", "0755", ...executables]);
  step("runtime-ldd");
  // prettier-ignore
  const ldd = (await prepared(["docker", "exec", name, "/bin/sh", "-ec",
    '. /etc/os-release; test "$ID" = ubuntu; test "$VERSION_ID" = 26.04; for binary do ldd "$binary"; done',
    "fvoci-runtime-abi", ...executables])).stdout;
  step("runtime-ldd-log");
  writeFileSync(join(run, "native-runtime-abi.log"), ldd);
  step("runtime-ldd-dependencies");
  assert.ok(!ldd.includes("not found"), "Ubuntu26 runtime ELF dependencies missing");
  step("mode-environment");
  await prepared(["docker", "exec", name, "chmod", "0600", "/fvoci/inputs/environment.sh"]);
  step("copy-static");
  await prepared(["docker", "cp", state.dist + "/.", name + ":/srv/fvoci-web"]);
  step("copied-owned-files");
  // prettier-ignore
  await prepared(["docker", "exec", name, "stat", "-c", "%n %u %g %a", ...destinations],
    join(run, "copied-owned-files.log"));
  step("copied-hashes");
  const hashes = (await prepared(["docker", "exec", name, "sha256sum", ...executables])).stdout;
  step("copied-hashes-log");
  writeFileSync(join(run, "copied-executable-hashes.log"), hashes);
  step("copied-hashes-match");
  const binaries = state.current.build.binaries;
  assert.ok(
    deepEquals(
      copiedHashes(hashes),
      [state.server, state.migrate, state.engine].map((path) => binaries[path]?.sha256),
    ),
  );
  step("network-mode");
  // prettier-ignore
  await prepared(["docker", "inspect", "--format", "{{.HostConfig.NetworkMode}}", name],
    join(run, "actual-network-mode.log"));
  step("network-attachments");
  // prettier-ignore
  const networks: unknown = JSON.parse((await prepared(["docker", "inspect", "--format",
    "{{json .NetworkSettings.Networks}}", name])).stdout);
  step("network-attachment-count");
  assert.ok(singleAttachment(networks));
  step("network-attachment-id");
  const networkId = attachedNetworkId(networks);
  step("network-driver");
  // prettier-ignore
  const driver = (await prepared(["docker", "network", "inspect", "--format", "{{.Id}} {{.Driver}}",
    networkId])).stdout;
  step("network-driver-log");
  writeFileSync(join(run, "actual-network-driver.log"), driver);
  step("network-driver-host");
  assert.ok(hostDriver(driver, networkId));
}

async function startServer(state: State, seam: Seam): Promise<void> {
  const { receipt, run } = state;
  receipt.phase = "server-startup";
  const log = join(run, "normal-server.log");
  state.serverLog = openSync(log, "w");
  // prettier-ignore
  const server = seam.spawnServer(["docker", "exec", state.name, "/bin/sh", "-ec",
    ". /fvoci/inputs/environment.sh; exec /fvoci/bin/fvoci-migrate --start"], state.serverLog);
  state.serverProcess = server;
  const deadline = performance.now() + 10_000;
  for (;;) {
    checkInterrupt();
    state.base = listenBase(readText(log));
    if (state.base !== null) break;
    assert.ok(running(server), "normal entrypoint exited before listen; see actual raw log");
    assert.ok(
      performance.now() < deadline,
      "normal entrypoint did not listen within unchanged10s process observation",
    );
    await Bun.sleep(20);
  }
}

async function qualifyServer(state: State, seam: Seam): Promise<void> {
  const { receipt, run, db, dbroot } = state;
  receipt.phase = "server-ready";
  receipt.loopback_port_observation = "observed";
  const rows = await seam.ownedRows(state.name);
  const candidates = rows.filter(
    (row) => row.args === "/fvoci/bin/fvoci-server" && !row.already_retired_at_observation,
  );
  assert.ok(candidates.length === 1, "one actual normal server required");
  const server = candidates[0] as Row;
  state.serverRow = server;
  const [uid, gid] = state.identity;
  assert.ok(server.uid === uid && server.gid === gid);
  const meta = statSync(db),
    parent = statSync(dbroot);
  assert.ok(deepEquals([meta.uid, meta.gid, meta.mode & 0o777, meta.nlink], [uid, gid, 0o600, 1]));
  assert.ok(deepEquals([parent.uid, parent.gid, parent.mode & 0o777], [uid, gid, 0o700]));
  const setup = await seam.probeSetup(state.base as string);
  assert.ok(
    setup.status === 200 &&
      setup.body !== null &&
      typeof setup.body === "object" &&
      (setup.body as Record<string, unknown>).needed === true,
  );
  // Independent read-only observer of committed migration receipts. Bun's
  // SQLite version and connection PRAGMAs are not the app runtime/FK proof.
  const applied = seam.migrationRows(db);
  const directory = join(root, "migrations/sqlite/060");
  const definitions = readdirSync(directory)
    .filter((file) => /^[0-9][0-9]_.*\.sql$/.test(file))
    .sort()
    .map((file) => ({ stem: file.slice(0, -4), sha256: sha(join(directory, file)) }));
  const expected = expectedMigrations(definitions, readText(join(root, "src/db/migrate.rs")));
  assert.ok(deepEquals(applied, expected) && applied.length === 12);
  state.databaseInode = inodeOf(db);
  Object.assign(receipt, {
    baseURL: state.base,
    actual_server: server,
    actual_process_rows_at_ready: rows,
    database_inode: state.databaseInode,
    actual_setup_needed: true,
    actual_migration_rows: applied,
    runtime_pin_oracle:
      "actual SQLx server/fixture connect path refuses version/source/FK mismatch; successful real fixture later executes its own exact3.53.4/source/FK1/current-schema reads; observer does not qualify its own runtime",
  });
  writeJson(join(run, "normal-main-ready.json"), receipt);
}

async function runBrowser(state: State, seam: Seam): Promise<void> {
  const { receipt, run, dbroot, db, flow } = state;
  const local = state.current.grant.executionMode === "orca-local";
  state.browserEnv = browserEnvironment(state.source, local, {
    base: state.base as string,
    flow,
    run,
    fixture: state.fixture,
    dbroot,
    db,
    serverEnv: state.serverEnv,
  });
  const web = join(root, "apps/web");
  // prettier-ignore
  const chromium = (await seam.command([state.bun, "--eval",
    "import { chromium } from '@playwright/test'; console.log(chromium.executablePath());"],
    { env: state.browserEnv, cwd: web })).stdout.trim();
  assert.ok(isAbsolute(chromium) && isFile(chromium));
  const browserInputs: Browser = {
    bun: { path: state.bun, sha256: sha(state.bun) },
    chromium: { path: chromium, sha256: sha(chromium) },
    chromium_directory_files: seam.treeHashes(dirname(chromium)),
  };
  state.browserInputs = browserInputs;
  writeJson(join(run, "actual-browser-inputs.json"), browserInputs);
  const args = browserArgs(state.bun, join(root, "node_modules/playwright/cli.js"), state.spec);
  receipt.phase = "browser";
  Object.assign(receipt, {
    browser_command: args,
    browser_environment_names: Object.keys(state.browserEnv).sort(),
    browser_start_utc: now(),
  });
  writeJson(join(run, "browser-start.json"), receipt);
  const started = performance.now();
  const log = join(run, "browser.log");
  // prettier-ignore
  const result = await seam.command(args,
    { log, required: false, env: state.browserEnv, cwd: web });
  state.code = result.returncode;
  if (state.code !== 0) recordFailure(state, state.code, undefined, log);
  const report = join(run, "playwright-result.private.json");
  if (existsSync(report)) {
    chmodSync(report, 0o600);
    receipt.actual_json_report_sha256 = sha(report);
  }
  Object.assign(receipt, {
    browser_exit: state.code,
    browser_seconds: (performance.now() - started) / 1000,
    browser_end_utc: now(),
    browser_log_sha256: sha(log),
  });
  const actors = readdirSync(dbroot)
    .filter((file) => /^actor-.*\.json$/.test(file))
    .sort();
  receipt.actual_actor_receipts = Object.fromEntries(
    actors.map((file) => [file, read(join(dbroot, file))]),
  );
  if (state.code !== 0) return;
  assert.ok(actors.length === 1, "exactly one fresh selected actor fixture");
  assertActorReceipt(read(join(dbroot, actors[0] as string)));
  if (flow === "on")
    assert.ok(/\b1 passed\b/.test(readText(log)), "exact one selected browser test");
  else receipt.actual_off_titles = seam.validateOffReport(read(report), "sqlite");
  assert.ok(deepEquals(inodeOf(db), state.databaseInode), "no replacement/reset DB");
  Object.assign(receipt, {
    actual_browser_tests: flow === "off" ? 8 : 1,
    retries: 0,
    ignored: 0,
    actual_fixture_pin_checks_completed: true,
    tested_product_flow:
      flow === "off"
        ? "immutable OFF8 actual Vue CAS/replay/native history/task/note/owner-transition/current revoke/lost-response newer head"
        : "actual currentVue setup/login/stable wiki create/nonempty nativeON/matching durableACK/manualrevision/fresh cookie actor body-native-ID-permission-history readback",
  });
  if (flow === "on") {
    receipt.phase = "restart";
    receipt.current_schema_server_restart = await seam.restart(
      restartContext(state, browserInputs, seam),
      seam,
    );
  }
}
function restartContext(state: State, browserInputs: Browser, seam: Seam): RestartContext {
  const current = state.current;
  assert.ok(state.serverRow !== null && state.serverProcess !== null && state.base !== null);
  return {
    run: state.run,
    name: state.name,
    browserEnv: state.browserEnv,
    code: state.code,
    head: state.head,
    tree: state.tree,
    compiledHead: state.head,
    identity: current.grant,
    parentDriver: state.driver,
    binaries: current.build.binaries,
    server: state.server,
    migrate: state.migrate,
    engine: state.engine,
    distFiles: current.assets.dist_files,
    abiFiles: current.abi.host_runtime_files,
    browserInputs,
    root,
    bun: state.bun,
    spec: state.spec,
    before: state.before,
    sourceBefore: state.sourceBefore,
    inputCheck: () => seam.inputCheck(state.before, state.head, state.tree),
    treeHashes: seam.treeHashes,
    dist: state.dist,
    storage: state.storage,
    serverRow: state.serverRow,
    serverProcess: state.serverProcess,
    base: state.base,
    receipt: state.receipt,
    projectBrowser: false,
    db: state.db,
    dbroot: state.dbroot,
  };
}

// The original packet already exists; every observation, removal and hash
// below is attempted and recorded, and none replaces the first outcome.
export function finalize(state: State, seam: Seam): Promise<number> {
  return cleanupScope(async () => {
    const { receipt, name, run } = state;
    const cleanupErrors: unknown[] = [...list(receipt, "diagnostic_errors")];
    const attempt = <T>(label: string, operation: () => T | Promise<T>) =>
      cleanupAttempt(receipt, cleanupErrors, label, operation);
    const exec = (args: string[]) => seam.command(args, { required: false });
    const server = state.serverProcess;
    if (state.created) {
      const rows = await attempt("process-observation-failed", () => seam.ownedRows(name));
      receipt.actual_process_rows_before_cleanup = rows;
      const serverRow = state.serverRow;
      if (serverRow !== null) {
        const gone = await attempt("server-identity-observation-failed", () =>
          seam.identityGone(serverRow),
        );
        if (gone === false) {
          // prettier-ignore
          const stopped = await attempt("server-sigterm-failed", () => exec(["docker", "exec",
            "--user", "0", name, "/bin/kill", "-TERM", String(serverRow.namespace_pid)]));
          if (stopped !== null) {
            receipt.owned_server_sigterm_exit = stopped.returncode;
            if (stopped.returncode) cleanupErrors.push("owned-server-sigterm-nonzero");
          }
        }
      }
      if (server !== null) {
        receipt.normal_server_exit = await attempt("normal-server-wait-failed", () =>
          waitFor(server, 10_000),
        );
        if (receipt.normal_server_exit !== 0)
          cleanupErrors.push("normal-server-finish-unconfirmed");
      }
      const remove = () => exec(["docker", "rm", "-f", "-v", name]);
      let removed = await attempt("owned-container-removal-failed", remove);
      removed ??= await attempt("exceptional-owned-removal-failed", remove);
      receipt.owned_container_cleanup_exit = removed?.returncode ?? null;
      const absent = await attempt("owned-container-absence-failed", () =>
        exec(["docker", "inspect", name]),
      );
      if (absent !== null) receipt.owned_container_absent = positiveDockerAbsence(absent);
      if (removed === null || removed.returncode || receipt.owned_container_absent !== true)
        cleanupErrors.push("owned-container-cleanup-unconfirmed");
      if (rows !== null) {
        try {
          receipt.recorded_process_identities_retired = retiredIdentities(rows, (row) =>
            seam.identityGone(row),
          );
        } catch (error) {
          cleanupErrors.push("pid-retirement-observation-failed");
          secondary(receipt, "pid-retirement-observation-failed", error);
        }
      }
      if (receipt.recorded_process_identities_retired !== true)
        cleanupErrors.push("owned-pid-retirement-unconfirmed");
    }
    if (server !== null) {
      const live = await attempt("docker-exec-poll-failed", () => running(server));
      if (live === true) {
        const waited = await attempt("docker-exec-wait-failed", () => waitFor(server, 10_000));
        if (waited === null) {
          await attempt("docker-exec-kill-failed", () => {
            server.kill("SIGKILL");
          });
          await attempt("docker-exec-force-wait-failed", () => waitFor(server, 10_000));
          cleanupErrors.push("docker-exec-force-reap-not-normal-close");
        }
      }
    }
    const serverLog = state.serverLog;
    if (serverLog !== null) {
      await attempt("server-log-close-failed", () => {
        closeSync(serverLog);
      });
      receipt.normal_server_log_sha256 = await attempt("server-log-hash-failed", () =>
        sha(join(run, "normal-server.log")),
      );
    }
    const base = state.base;
    if (base !== null) {
      receipt.owned_loopback_port_closed = await attempt("loopback-port-observation-failed", () =>
        seam.portClosed(browserPort(base)),
      );
      if (receipt.owned_loopback_port_closed !== true)
        cleanupErrors.push("owned-loopback-port-closure-unconfirmed");
    } else cleanupErrors.push("owned loopback port never observed; retirement remains unqualified");
    try {
      const after = await seam.inputCheck(state.before, state.head, state.tree);
      writeJson(join(run, "source-inputs-after.json"), after);
      assert.ok(deepEquals(state.sourceBefore, after));
      assert.ok(deepEquals(seam.treeHashes(state.dist), state.current.assets.dist_files));
      const binaries = state.current.build.binaries;
      for (const path of [state.server, state.migrate, state.fixture, state.engine])
        assert.ok(sha(path) === binaries[path]?.sha256);
      for (const [path, expected] of Object.entries(state.current.abi.host_runtime_files))
        assert.ok(sha(path) === expected);
      const inputs = state.browserInputs;
      if (inputs !== null) {
        assert.ok(sha(state.bun) === inputs.bun.sha256);
        assert.ok(sha(inputs.chromium.path) === inputs.chromium.sha256);
        assert.ok(
          deepEquals(
            seam.treeHashes(dirname(inputs.chromium.path)),
            inputs.chromium_directory_files,
          ),
        );
      }
      receipt.exact_source_artifact_inputs_unchanged = true;
    } catch (error) {
      receipt.exact_source_artifact_inputs_unchanged = false;
      cleanupErrors.push("post-input-check-failed");
      secondary(receipt, "post-input-check-failed", error);
    }
    receipt.ended_utc = await attempt("end-clock-observation-failed", now);
    let code = state.code;
    if (cleanupErrors.length) code ||= 1;
    Object.assign(receipt, {
      cleanup_errors: cleanupErrors,
      final_exit_code: code,
      retained_private_evidence: run,
      retained_dbroot: state.dbroot,
      retained_storage: state.storage,
    });
    await attempt("final-receipt-write-failed", () => {
      writeJson(join(run, "receipt.json"), receipt);
    });
    if (cleanupErrors.length) code ||= 1;
    try {
      seam.emit(JSON.stringify(summary(receipt, code, cleanupErrors)));
    } catch {
      code ||= 1;
    }
    return code;
  });
}

export async function main(
  seam: Seam = sqliteSeam,
  driver = sqliteDriver,
  source: Environment = process.env,
  identity: readonly [number, number] = [1000, 1000],
): Promise<number> {
  // The whole binding is checked before any resource is created.
  env("FVOCI_CI_SELECTED_RUNS", source);
  const current = await seam.loadCurrent("sqlite", driver);
  const { manifest, flow } = current;
  const head = manifest.source,
    tree = manifest.tree;
  const owner = env("FVOCI_CI_OWNER", source);
  const spec =
    flow === "off"
      ? "workspace-off-selected-backend.spec.ts"
      : "workspace-wiki-selected-backend.spec.ts";
  const bun = env("FVOCI_CI_BUN", source);
  assert.ok(source.FVOCI_ROOT_RUN_OWNER === owner);
  const before = current.before;
  const sourceBefore = current.sourceBefore;
  const binaries = Object.keys(current.build.binaries);
  const pick = (suffix: string) => {
    const path = binaries.find((candidate) => candidate.endsWith(suffix));
    assert.ok(path !== undefined, "missing current cohort executable");
    return path;
  };
  const server = pick("/fvoci-server"),
    migrate = pick("/fvoci-migrate"),
    fixture = pick("/fvoci-e2e-fixture"),
    engine = pick("/collab-engine");
  const dist = join(root, "apps/web/dist");
  assert.ok(isFile(bun) && isFile(join(root, "node_modules/.bin/playwright")));
  assert.ok(
    source.FVOCI_E2E_SELECTED_AUXILIARY === undefined,
    "BLOCKED: SQLite auxiliary normal writers are not ready",
  );
  const run = current.run;
  mkdirSync(run, { mode: 0o700 });
  // The restart binding re-hashes these exact bytes.
  writeJson(join(run, "source-inputs-before.json"), sourceBefore);
  const dbroot = join(run, "database"),
    storage = join(run, "storage");
  mkdirSync(dbroot, { mode: 0o700 });
  mkdirSync(storage, { mode: 0o700 });
  const db = join(dbroot, "app.sqlite");
  assert.ok(!existsSync(db));
  const name = "fvoci-v060-vue-sqlite-current-" + randomBytes(6).toString("hex");
  const serverEnv = serverEnvironment(flow, () => randomBytes(32).toString("hex"));
  privateText(join(run, "environment.private.json"), JSON.stringify(serverEnv));
  privateText(join(run, "environment.private.sh"), shellExports(serverEnv));
  const state: State = {
    current,
    driver,
    run,
    dbroot,
    storage,
    db,
    dist,
    name,
    owner,
    head,
    tree,
    flow,
    spec,
    bun,
    server,
    migrate,
    fixture,
    engine,
    serverEnv,
    before,
    sourceBefore,
    receipt: {},
    identity,
    source,
    step: null,
    created: false,
    serverProcess: null,
    serverLog: null,
    base: null,
    serverRow: null,
    browserEnv: {},
    browserInputs: null,
    databaseInode: null,
    code: 1,
  };
  state.receipt = {
    source: head,
    tree,
    compiled_source: head,
    current_binding: current.manifestPath,
    current_binding_sha256: sha(current.manifestPath),
    root_owner: owner,
    started_utc: now(),
    driver_sha256: sha(driver),
    selected_flow: flow,
    container: name,
    image,
    scope:
      "one real SQLite normal migrate--start/current Vue/native ON tracer; PG/Turso/search/OFF/restore/fullCI/shipping image pending",
    network: "host network, app bind127.0.0.1:0 only; no network namespace isolation",
    runtime_abi: current.abi,
    binary_inputs: Object.fromEntries(
      [server, migrate, fixture, engine].map((path) => [path, current.build.binaries[path]]),
    ),
    source_count: Object.keys(before.tracked).length,
    external_count: Object.keys(before.external).length,
    static_count: Object.keys(current.assets.dist_files).length,
    static_build_source: current.assets.source,
    environment_names: Object.keys(serverEnv).sort(),
    private_environment_file: join(run, "environment.private.json"),
    new_owned_dbroot: dbroot,
    new_owned_storage: storage,
    host_uid: process.getuid?.(),
    host_gid: process.getgid?.(),
    browser_retries: 0,
    workers: 1,
    original_failure_policy:
      "retain raw/traces/database/actor receipts; no reset or relaxed assertions",
    restart_constraint:
      "exact67c checkpoint preserves actor receipt privately and restarts SAME DB/storage; no reset/reseed",
    phase: "container-prepare",
    owned_container_absent: null,
    owned_loopback_port_closed: null,
    loopback_port_observation: "not-observed",
    recorded_process_identities_retired: null,
  };
  writeJson(join(run, "start.json"), state.receipt);
  seam.trap();
  try {
    await prepareContainer(state, seam);
    await startServer(state, seam);
    await qualifyServer(state, seam);
    await runBrowser(state, seam);
  } catch (error) {
    recordFailure(state, (state.receipt.browser_exit as number | undefined) ?? null, { error });
    state.code ||= 1;
  }
  return finalize(state, seam);
}

export function migrationRows(path: string): unknown[][] {
  const observer = new Database(path, { readonly: true });
  try {
    return observer
      .query("SELECT version,lineage,sql_sha256 FROM schema_migrations ORDER BY version")
      .values();
  } finally {
    observer.close();
  }
}
export const sqliteSeam: Seam = {
  trap: trapInterrupts,
  loadCurrent,
  inputCheck: (before, head, tree) => inputCheck(before, head, tree),
  treeHashes,
  migrationRows,
  validateOffReport,
  restart: restartSameApp,
  command,
  ownedRows: (name) => ownedRows(name),
  identityGone,
  pgSql: () => Promise.reject(runtimeError("the sqlite lane has no PostgreSQL role")),
  spawnServer,
  probeSetup,
  portClosed,
  emit,
  actor,
};

if (import.meta.main) {
  assertNoEnvFile();
  process.exit(await main());
}
