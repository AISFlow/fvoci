// Root-owned, one-shot current development PostgreSQL normal-main/current Vue
// lane driver. No compilation, package install, product patch, migration reset
// or external account.
//
// CLI: no argument runs the parent, which re-enters this file through the
// owned PostgreSQL wrapper as `--pg-ready <run>`, which re-enters through the
// owned Meilisearch wrapper as `--inside <run>`. Every mode first rebinds the
// whole current artifact/preparation proof. Each process prints at most its
// final JSON summary on stdout and exits with the lane's first failure code.
import { deepEquals, spawn } from "bun";
import { strict as assert } from "node:assert";
import { randomBytes, randomUUID } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  statSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { get } from "node:http";
import { basename, dirname, join, resolve } from "node:path";
import process from "node:process";
import { env, jsonInteger, parseJson, read, resolved, root, sha } from "../io.ts";
import type { Browser, Inputs } from "../types.ts";
import { loadCurrent, validateOffReport, type Current } from "./binding.ts";
import {
  assertNoEnvFile,
  attachmentJson,
  browserPacketKeys,
  bunDriver,
  checkInterrupt,
  cleanupAttempt,
  cleanupScope,
  command,
  decode,
  errorFacts,
  failureCheckpoint,
  failureDigest,
  identityGone,
  inputCheck,
  knownBrowserCheckpoint,
  list,
  now,
  ownedObjectAbsent,
  ownedRows,
  portClosed,
  readText,
  running,
  runtimeError,
  secondary,
  shellQuote,
  trapInterrupts,
  treeHashes,
  waitFor,
  writeJson,
} from "./common.ts";
import type { Child, Command, Json, Receipt, Row } from "./common.ts";
import {
  browserPort,
  restartSameApp,
  restrictedRoleQuery,
  type RestartContext,
  type Seam as RestartSeam,
} from "./restart.ts";

export const postgresDriver = import.meta.path;
// The runner's argv for this lane driver.
export const postgresDriverCommand = () => bunDriver(postgresDriver);
const image =
  "ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7";
export const pgImage =
  "postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db";
export const meiliImage =
  "getmeili/meilisearch:v1.53.2@sha256:c94e58ca09662dd6e65e8f1b0fd145767be3da7d5422a863a27b8d2b68e090c9";
const pgScript = join(root, "scripts/start-test-postgres.sh");
const meiliScript = join(root, "scripts/start-test-meili.sh");
const playwrightCli = join(root, "node_modules/playwright/cli.js");
const reporterSource = join(root, "node_modules/playwright/lib/runner/index.js");
const readyPattern = /fvoci-server listening on (http:\/\/127\.0\.0\.1:(\d+))/;
const meiliKey = "/run/fvoci/meili/api_key";
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
// Canonical lowercase UUID text only; it is interpolated into owned SQL.
export function canonicalUuid(value: unknown): string {
  assert.ok(typeof value === "string" && uuidPattern.test(value), "canonical UUID required");
  return value;
}
const hex = (bytes: number) => randomBytes(bytes).toString("hex");
const isRecord = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

// Playwright's direct CLI: the pinned package's own bin file, a regular file in
// the admitted external input closure.
export function qualifyPlaywright(workspace: string, before: Inputs, bun: string): string {
  const cli = join(workspace, "node_modules/playwright/cli.js");
  assert.ok(statSync(bun, { throwIfNoEntry: false })?.isFile());
  assert.ok(!lstatSync(cli, { throwIfNoEntry: false })?.isSymbolicLink());
  assert.ok(statSync(cli, { throwIfNoEntry: false })?.isFile());
  const manifest = read(join(workspace, "node_modules/playwright/package.json")) as {
    version?: unknown;
    bin?: { playwright?: unknown };
  };
  assert.ok(manifest.version === "1.63.0" && manifest.bin?.playwright === "cli.js");
  assert.ok(
    sha(cli) === before.external[cli],
    "Playwright CLI must be in the admitted input closure",
  );
  return cli;
}

export const browserArgs = (bun: string, cli: string, spec: string) => [
  bun,
  "--no-install",
  cli,
  "test",
  "--config",
  "e2e-pending/collab-playwright.config.ts",
  "--reporter=line,json",
  spec,
];

// The rebinding every mode performs before it touches a resource.
export interface Lane {
  current: Current;
  head: string;
  tree: string;
  owner: string;
  flow: "on" | "off";
  spec: string;
  bun: string;
  before: Inputs;
  sourceBefore: Inputs;
  binaries: Record<string, { sha256: string }>;
  server: string;
  migrate: string;
  fixture: string;
  engine: string;
  dist: string;
  reporterSha: string;
}
export async function rebind(): Promise<Lane> {
  const current = await loadCurrent("postgres", postgresDriver);
  const head = current.manifest.source,
    tree = current.manifest.tree,
    owner = env("FVOCI_CI_OWNER"),
    flow = current.flow;
  const bun = env("FVOCI_CI_BUN");
  assert.ok(
    process.env.FVOCI_E2E_SELECTED_AUXILIARY === (flow === "on" ? "normal-api" : undefined),
  );
  const before = current.before;
  const sourceBefore = await inputCheck(before, head, tree);
  const binaries = current.build.binaries;
  const named = (suffix: string) => {
    const path = Object.keys(binaries).find((item) => item.endsWith(suffix));
    assert.ok(path !== undefined);
    return path;
  };
  qualifyPlaywright(root, before, bun);
  for (const [script, pinned] of [
    [pgScript, pgImage],
    [meiliScript, meiliImage],
  ] as const) {
    assert.ok(readText(script).includes(pinned));
    assert.ok(sha(script) === before.tracked[script.slice(root.length + 1)]);
  }
  const reporterSha = sha(reporterSource),
    reporter = readText(reporterSource);
  assert.ok(reporter.includes("PLAYWRIGHT_${name}_OUTPUT_FILE"));
  assert.ok(reporter.includes('body: a.body?.toString("base64")'));
  return {
    current,
    head,
    tree,
    owner,
    flow,
    spec:
      flow === "off"
        ? "workspace-off-selected-backend.spec.ts"
        : "workspace-wiki-selected-backend.spec.ts",
    bun,
    before,
    sourceBefore,
    binaries,
    server: named("/fvoci-server"),
    migrate: named("/fvoci-migrate"),
    fixture: named("/fvoci-e2e-fixture"),
    engine: named("/collab-engine"),
    dist: join(root, "apps/web/dist"),
    reporterSha,
  };
}
export async function postInputs(lane: Lane): Promise<Inputs> {
  const checked = await inputCheck(lane.before, lane.head, lane.tree);
  assert.ok(deepEquals(treeHashes(lane.dist), lane.current.assets.dist_files));
  for (const [path, record] of Object.entries(lane.binaries))
    assert.ok(sha(path) === record.sha256);
  for (const [path, expected] of Object.entries(lane.current.abi.host_runtime_files))
    assert.ok(sha(path) === expected);
  assert.ok(sha(reporterSource) === lane.reporterSha);
  return checked;
}

export interface FixtureInfo {
  kind: "pg" | "meili";
  name: string;
  image: string;
  port: number;
  rows: Row[];
  volumes: string[];
}
interface Inspected {
  Config: { Image: unknown; Labels: Record<string, unknown> };
  NetworkSettings: { Ports: Record<string, { HostIp: unknown; HostPort: unknown }[]> };
  Mounts: { Type: unknown; Name: string }[];
}
export async function fixtureInfo(
  kind: "pg" | "meili",
  run: Command = command,
): Promise<FixtureInfo> {
  const name = env("FVOCI_TEST_" + kind.toUpperCase() + "_CONTAINER"),
    prefix = "fvoci-rust-test-" + kind + "-",
    id = name.slice(prefix.length);
  assert.ok(name.startsWith(prefix) && /^[0-9a-f]{32}$/.test(id));
  const data = (parseJson((await run(["docker", "inspect", name])).stdout) as Inspected[])[0];
  assert.ok(data !== undefined && data.Config.Image === (kind === "pg" ? pgImage : meiliImage));
  assert.ok(data.Config.Labels["fvoci.test-run"] === id);
  const binding = data.NetworkSettings.Ports[kind === "pg" ? "5432/tcp" : "7700/tcp"];
  assert.ok(binding?.length === 1 && binding[0]?.HostIp === "127.0.0.1");
  const port = binding[0].HostPort;
  assert.ok(typeof port === "string" && /^[0-9]+$/.test(port));
  return {
    kind,
    name,
    image: data.Config.Image as string,
    port: Number(port),
    rows: await ownedRows(name, run),
    volumes: data.Mounts.filter((mount) => mount.Type === "volume").map((mount) => mount.Name),
  };
}

export interface FixtureClosure {
  kind: string;
  name: string;
  containerAbsent: boolean;
  recordedPIDIdentitiesRetired: boolean;
  portClosed: boolean;
  ownedVolumesAbsent: Record<string, boolean>;
}
export async function verifyFixturesClosed(
  run: string,
  seam: Pick<Seam, "command" | "identityGone" | "portClosed">,
): Promise<FixtureClosure[]> {
  const result: FixtureClosure[] = [];
  for (const file of readdirSync(run)
    .filter((entry) => /^fixture-.*-ready\.json$/s.test(entry))
    .sort()) {
    const data = read(join(run, file)) as FixtureInfo;
    const absent = await ownedObjectAbsent(["docker", "inspect", data.name], seam.command);
    const volumes: Record<string, boolean> = {};
    for (const volume of data.volumes)
      volumes[volume] = await ownedObjectAbsent(
        ["docker", "volume", "inspect", volume],
        seam.command,
      );
    result.push({
      kind: data.kind,
      name: data.name,
      containerAbsent: absent,
      recordedPIDIdentitiesRetired: data.rows.every((row) => seam.identityGone(row)),
      portClosed: await seam.portClosed(data.port),
      ownedVolumesAbsent: volumes,
    });
  }
  return result;
}

// The I/O this driver owns, replaceable as one boundary in tests.
export interface Seam extends RestartSeam {
  postInputs: () => Promise<Inputs>;
}
export const emit = (line: string) => {
  writeSync(1, line + "\n");
};
export const actor = () => [process.getuid?.(), process.getgid?.()] as const;
export function probeSetup(base: string): Promise<{ status: number; body: unknown }> {
  // node:http never consults proxy variables; the owned server is loopback.
  return new Promise((resolvePromise, reject) => {
    const request = get(base + "/api/v1/setup", { timeout: 10_000 }, (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      response.on("error", reject);
      response.on("end", () => {
        try {
          resolvePromise({
            status: response.statusCode ?? 0,
            body: JSON.parse(decode(Buffer.concat(chunks))),
          });
        } catch (error) {
          reject(error instanceof Error ? error : runtimeError("owned setup probe failed"));
        }
      });
    });
    request.on("timeout", () => request.destroy(runtimeError("owned setup probe timed out")));
    request.on("error", reject);
  });
}
export function spawnServer(args: string[], log: number): Child {
  return spawn(args, { stdin: "ignore", stdout: log, stderr: log });
}

const environmentNames = [
  "FVOCI_ROOT_CURRENT_BINDING",
  "FVOCI_ROOT_CURRENT_ALLOCATION",
  "FVOCI_ROOT_RESTART_GRANT",
  "GITHUB_RUN_ID",
  "GITHUB_RUN_ATTEMPT",
  "CI",
  "GITHUB_ACTIONS",
  "GITHUB_SHA",
  "GITHUB_REPOSITORY",
  "GITHUB_JOB",
  "FVOCI_CI_OWNER",
  "FVOCI_CI_SELECTED_RUNS",
  "FVOCI_CI_BUN",
  "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
  "TMPDIR",
  "FVOCI_E2E_SELECTED_FLOW",
  "FVOCI_SELECTED_EXECUTION_MODE",
  "FVOCI_SELECTED_LOCAL_ALLOCATION",
  "FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256",
  "FVOCI_LOCAL_RUN_ID",
  "FVOCI_LOCAL_DISPATCH_ID",
  "FVOCI_LOCAL_TASK_ID",
  "ORCA_TERMINAL_HANDLE",
  "FVOCI_LOCAL_ROOT_TERMINAL",
];
// The filtered environment the owned fixture wrappers re-enter this driver with.
export function wrapperEnvironment(flow: "on" | "off", owner: string): Record<string, string> {
  const environment: Record<string, string> = {
    PATH: env("PATH"),
    LANG: process.env.LANG ?? "C.UTF-8",
    FVOCI_ROOT_RUN_OWNER: owner,
    FVOCI_TEST_PG_MAJOR: "18",
  };
  if (flow === "on") environment.FVOCI_E2E_SELECTED_AUXILIARY = "normal-api";
  for (const name of environmentNames) {
    const value = process.env[name];
    if (value !== undefined) environment[name] = value;
  }
  if (process.env.PLAYWRIGHT_BROWSERS_PATH)
    environment.PLAYWRIGHT_BROWSERS_PATH = process.env.PLAYWRIGHT_BROWSERS_PATH;
  return environment;
}

export interface ParentFacts {
  head: string;
  tree: string;
  owner: string;
  flow: "on" | "off";
  sourceBefore: Inputs;
}
export interface ParentSeam {
  command: Command;
  emit: (line: string) => void;
  verifyFixturesClosed: (run: string) => Promise<FixtureClosure[]>;
  postInputs: () => Promise<Inputs>;
}
const write = (path: string, value: unknown) => {
  writeJson(path, value);
};
// The parent owns the PostgreSQL wrapper; it qualifies the fixture closure,
// the child receipt and the unchanged inputs whatever the wrapper did.
export async function parent(run: string, facts: ParentFacts, seam: ParentSeam): Promise<number> {
  mkdirSync(run, { mode: 0o700 });
  write(join(run, "source-inputs-before.json"), facts.sourceBefore);
  const environment = wrapperEnvironment(facts.flow, facts.owner);
  const summary: Receipt = {
    source: facts.head,
    tree: facts.tree,
    compiled_source: facts.head,
    root_owner: facts.owner,
    selected_flow: facts.flow,
    phase: "owned-fixture-wrapper",
    wrapper_exit: null,
    started_utc: null,
    fixture_closure: null,
    all_owned_fixtures_closed: false,
    actual_child_receipt_present: false,
    exact_source_artifact_inputs_unchanged: false,
    retained_private_evidence: run,
    scope:
      "one normal restricted PG18/current Vue tracer; no full0.6/Turso/restore/CI/shipping acceptance",
  };
  let code = 1;
  const cleanupErrors: unknown[] = [];
  const packet = "parent-original-failure.private.json";
  try {
    summary.started_utc = now();
    const result = await seam.command(
      ["bash", pgScript, ...postgresDriverCommand(), "--pg-ready", run],
      { log: join(run, "owned-fixtures.log"), required: false, env: environment },
    );
    code = result.returncode;
    summary.wrapper_exit = code;
    if (code !== 0)
      failureCheckpoint(summary, run, code, {
        bodyLog: join(run, "owned-fixtures.log"),
        packetName: packet,
      });
  } catch (error) {
    failureCheckpoint(summary, run, summary.wrapper_exit, { error, packetName: packet });
    code ||= 1;
  }
  return cleanupScope(async () => {
    cleanupErrors.push(...((summary.diagnostic_errors as unknown[] | undefined) ?? []));
    const checks = await cleanupAttempt(
      summary,
      cleanupErrors,
      "fixture-closure-observation-failed",
      () => seam.verifyFixturesClosed(run),
    );
    summary.fixture_closure = checks;
    if (checks !== null) {
      const complete = await cleanupAttempt(
        summary,
        cleanupErrors,
        "fixture-closure-shape-failed",
        () =>
          checks.length === 2 &&
          checks.every(
            (row) =>
              row.containerAbsent &&
              row.recordedPIDIdentitiesRetired &&
              row.portClosed &&
              Object.values(row.ownedVolumesAbsent).every(Boolean),
          ),
      );
      summary.all_owned_fixtures_closed = complete === true;
    }
    const child = await cleanupAttempt(summary, cleanupErrors, "child-receipt-read-failed", () =>
      read(join(run, "receipt.json")),
    );
    summary.actual_child_receipt_present = isRecord(child);
    if (
      summary.all_owned_fixtures_closed !== true ||
      !isRecord(child) ||
      !jsonInteger(child, "final_exit_code") ||
      child.final_exit_code !== 0
    )
      code ||= 1;
    const inputsEqual = await cleanupAttempt(
      summary,
      cleanupErrors,
      "parent-post-input-check-failed",
      async () => {
        const after = await seam.postInputs();
        write(join(run, "parent-source-inputs-after.json"), after);
        return deepEquals(facts.sourceBefore, after, true);
      },
    );
    summary.exact_source_artifact_inputs_unchanged = inputsEqual === true;
    if (inputsEqual !== true) code ||= 1;
    summary.driver_sha256 = await cleanupAttempt(
      summary,
      cleanupErrors,
      "parent-driver-hash-failed",
      () => sha(postgresDriver),
    );
    summary.ended_utc = await cleanupAttempt(
      summary,
      cleanupErrors,
      "parent-end-clock-failed",
      now,
    );
    if (cleanupErrors.length) code ||= 1;
    Object.assign(summary, { final_exit_code: code, cleanup_errors: cleanupErrors });
    await cleanupAttempt(summary, cleanupErrors, "parent-final-receipt-write-failed", () => {
      write(join(run, "parent-receipt.json"), summary);
    });
    const parentHash = await cleanupAttempt(
      summary,
      cleanupErrors,
      "parent-receipt-hash-failed",
      () => sha(join(run, "parent-receipt.json")),
    );
    if (cleanupErrors.length) code ||= 1;
    try {
      seam.emit(
        JSON.stringify({
          source: facts.head,
          final_exit_code: code,
          actual_child_receipt_present: summary.actual_child_receipt_present,
          all_owned_fixtures_closed: summary.all_owned_fixtures_closed,
          exact_source_artifact_inputs_unchanged: summary.exact_source_artifact_inputs_unchanged,
          parent_receipt_sha256: parentHash,
          original_failure_checkpoint_sha256: summary.original_failure_checkpoint_sha256 ?? null,
          failure_code: code ? "SELECTED_PG_PARENT_FAILED" : null,
          cleanup_failure_codes: cleanupErrors,
        }),
      );
    } catch {
      code ||= 1;
    }
    return code;
  });
}

// A nonzero browser exit: mode 0600 on the original raw files, then the
// allowlisted report projection, then the first-failure packet. No copy.
export function recordBrowserFailure(receipt: Receipt, run: string, code: number): void {
  const browserLog = join(run, "browser.log"),
    reportPath = join(run, "playwright-result.private.json");
  if (statSync(browserLog, { throwIfNoEntry: false })?.isFile()) chmodSync(browserLog, 0o600);
  if (statSync(reportPath, { throwIfNoEntry: false })?.isFile()) {
    chmodSync(reportPath, 0o600);
    try {
      receipt.actual_json_report_sha256 = sha(reportPath);
      Object.assign(receipt, knownBrowserCheckpoint(read(reportPath)));
    } catch {
      Object.assign(receipt, {
        known_browser_test: null,
        known_browser_status: null,
        known_browser_checkpoint: null,
        browser_report_state: "report-unreadable",
      });
      list(receipt, "diagnostic_errors").push("browser-report-read-failed");
    }
  } else
    Object.assign(receipt, {
      known_browser_test: null,
      known_browser_status: null,
      known_browser_checkpoint: null,
      browser_report_state: "report-missing",
    });
  failureCheckpoint(receipt, run, code, { bodyLog: browserLog, extraKeys: browserPacketKeys });
}

// The child's owned resources, as far as the body got before it stopped.
export interface InsideState {
  receipt: Receipt;
  run: string;
  storage: string;
  name: string;
  code: number;
  created: boolean;
  serverRow: Row | null;
  serverProcess: Child | null;
  serverLog: number | null;
  base: string | null;
  bun: string;
  sourceBefore: Inputs;
  browserInputs: Browser | null;
}
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
const preservedStateQuery = `SELECT jsonb_build_object('versions',(SELECT jsonb_agg(version ORDER BY version) FROM fvoci.schema_migrations),
          'documents',(SELECT jsonb_agg(to_jsonb(d)) FROM fvoci.documents d),
          'states',(SELECT jsonb_agg(jsonb_build_object('workspace',workspace_id,'document',document_id,'encoding',encoding,'state',encode(state,'base64'),'tail',tail_seq,'cutoff',snapshot_cutoff_seq)) FROM fvoci.document_states),
          'updates',(SELECT jsonb_agg(jsonb_build_object('workspace',workspace_id,'document',document_id,'seq',seq,'op',op_id,'payload',encode(payload,'base64'))) FROM fvoci.document_collab_updates),
          'commands',(SELECT jsonb_agg(to_jsonb(c)) FROM fvoci.wiki_create_commands c),
          'revisions',(SELECT jsonb_agg(jsonb_build_object('id',id,'workspace',workspace_id,'target',target_id,'reason',reason,'creator',created_by,'snapshot',encode(y_snapshot,'base64'),'content',content_json)) FROM fvoci.revisions))`;

// The original packet exists before any observation, removal or hash can fail;
// every step records its own failure and the first exit code is kept.
export async function finalize(state: InsideState, seam: Seam): Promise<number> {
  return cleanupScope(async () => {
    const { receipt, run, name } = state;
    let code = state.code;
    const cleanupErrors: unknown[] = [
      ...((receipt.diagnostic_errors as unknown[] | undefined) ?? []),
    ];
    const attempt = <T>(label: string, operation: () => T | Promise<T>) =>
      cleanupAttempt(receipt, cleanupErrors, label, operation);
    // Bounded synthetic tracer state, kept before the wrappers remove their
    // private fixture. No password hashes, session tokens or keys.
    try {
      write(
        join(run, "retained-native-tracer-state.json"),
        await seam.pgSql(preservedStateQuery, false),
      );
      receipt.retained_native_state_sha256 = sha(join(run, "retained-native-tracer-state.json"));
    } catch (error) {
      receipt.native_evidence_preservation_error = errorFacts(error);
      cleanupErrors.push("native-evidence-preservation-failed");
    }
    if (state.created) {
      const rows = await attempt("process-observation-failed", () => seam.ownedRows(name));
      receipt.actual_process_rows_before_cleanup = rows;
      const serverRow = state.serverRow;
      if (serverRow !== null) {
        const gone = await attempt("server-identity-observation-failed", () =>
          seam.identityGone(serverRow),
        );
        if (gone === false) {
          const stopped = await attempt("server-sigterm-failed", () =>
            seam.command(
              [
                "docker",
                "exec",
                "--user",
                "0",
                name,
                "/bin/kill",
                "-TERM",
                String(serverRow.namespace_pid),
              ],
              { required: false },
            ),
          );
          if (stopped !== null) {
            receipt.owned_server_sigterm_exit = stopped.returncode;
            if (stopped.returncode) cleanupErrors.push("owned-server-sigterm-nonzero");
          }
        }
      }
      const serverProcess = state.serverProcess;
      if (serverProcess !== null) {
        receipt.normal_server_exit = await attempt("normal-server-wait-failed", () =>
          waitFor(serverProcess, 10_000),
        );
        if (receipt.normal_server_exit !== 0)
          cleanupErrors.push("normal-server-finish-unconfirmed");
      }
      const remove = () => seam.command(["docker", "rm", "-f", "-v", name], { required: false });
      const removed =
        (await attempt("owned-container-removal-failed", remove)) ??
        (await attempt("exceptional-owned-removal-failed", remove));
      receipt.owned_container_cleanup_exit = removed === null ? null : removed.returncode;
      const absent = await attempt("owned-container-absence-failed", () =>
        ownedObjectAbsent(["docker", "inspect", name], seam.command),
      );
      if (absent !== null) receipt.owned_container_absent = absent;
      if (removed === null || removed.returncode || receipt.owned_container_absent !== true)
        cleanupErrors.push("owned-container-cleanup-unconfirmed");
      if (rows !== null) {
        try {
          receipt.recorded_process_identities_retired =
            rows.length > 0 && rows.every((row) => seam.identityGone(row));
        } catch (error) {
          cleanupErrors.push("pid-retirement-observation-failed");
          secondary(receipt, "pid-retirement-observation-failed", error);
        }
      }
      if (receipt.recorded_process_identities_retired !== true)
        cleanupErrors.push("owned-pid-retirement-unconfirmed");
    }
    const serverProcess = state.serverProcess;
    if (serverProcess !== null) {
      const stillRunning = await attempt("docker-exec-poll-failed", () => running(serverProcess));
      if (stillRunning === true) {
        const waited = await attempt("docker-exec-wait-failed", () =>
          waitFor(serverProcess, 10_000),
        );
        if (waited === null) {
          await attempt("docker-exec-kill-failed", () => {
            serverProcess.kill("SIGKILL");
          });
          await attempt("docker-exec-force-wait-failed", () => waitFor(serverProcess, 10_000));
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
      const after = await seam.postInputs();
      write(join(run, "source-inputs-after.json"), after);
      assert.ok(deepEquals(after, state.sourceBefore, true));
      const browser = state.browserInputs;
      if (browser !== null) {
        assert.ok(sha(state.bun) === browser.bun.sha256);
        assert.ok(sha(browser.chromium.path) === browser.chromium.sha256);
        assert.ok(
          deepEquals(treeHashes(dirname(browser.chromium.path)), browser.chromium_directory_files),
        );
      }
      receipt.exact_source_artifact_inputs_unchanged = true;
    } catch (error) {
      receipt.exact_source_artifact_inputs_unchanged = false;
      cleanupErrors.push("post-input-check-failed");
      secondary(receipt, "post-input-check-failed", error);
    }
    receipt.ended_utc = await attempt("end-clock-observation-failed", now);
    if (cleanupErrors.length) code ||= 1;
    Object.assign(receipt, {
      cleanup_errors: cleanupErrors,
      final_exit_code: code,
      retained_private_evidence: run,
      retained_storage: state.storage,
    });
    await attempt("final-receipt-write-failed", () => {
      write(join(run, "receipt.json"), receipt);
    });
    if (cleanupErrors.length) code ||= 1;
    const original = receipt.original_driver_failure;
    const summary: Record<string, unknown> = Object.fromEntries(
      summaryKeys.map((key) => [key, receipt[key] ?? null]),
    );
    Object.assign(summary, {
      failure_code: code ? "SELECTED_DRIVER_FAILED" : null,
      final_exit_code: code,
      cleanup_failure_codes: cleanupErrors,
      original_driver_failure_sha256: original === undefined ? null : failureDigest(original),
    });
    try {
      seam.emit(JSON.stringify(summary));
    } catch {
      code ||= 1;
    }
    return code;
  });
}

interface Report {
  config: { workers: number };
  errors: unknown[];
  stats: Record<string, unknown>;
  suites: { specs: { tests: { results: Json[] }[] }[] }[];
}
const attachment = (result: Json, name: string): Json => {
  const entries = (result.attachments as Json[]).filter((entry) => entry.name === name);
  assert.ok(entries.length === 1 && entries[0]?.contentType === "application/json");
  const value = attachmentJson(entries[0].body);
  assert.ok(isRecord(value));
  return value;
};

// --inside: the owned app container, normal server, restricted-role facts,
// browser run and restart, then finalize whatever was reached.
export async function inside(lane: Lane, run: string): Promise<number> {
  const pg = await fixtureInfo("pg");
  const meili = await fixtureInfo("meili");
  write(join(run, "fixture-meili-ready.json"), meili);
  const ownerUrl = env("TEST_DATABASE_URL");
  const url = /^postgres:\/\/postgres:([^@/?#]*)@127\.0\.0\.1:([0-9]+)\/postgres$/.exec(ownerUrl);
  assert.ok(url && Number(url[2]) === pg.port);
  const ownerPassword = decodeURIComponent(url[1] as string);
  assert.ok(ownerPassword.length >= 16);
  assert.ok(env("FVOCI_MEILI_URL") === "http://127.0.0.1:" + String(meili.port));
  const masterKey = env("MEILI_MASTER_KEY");
  assert.ok(masterKey === env("FVOCI_MEILI_KEY") && masterKey.length >= 16);
  const role = "fvoci_v060_app_" + hex(8),
    appPassword = hex(24);
  assert.ok(appPassword !== ownerPassword);
  const appUrl = `postgres://${role}:${appPassword}@127.0.0.1:${String(pg.port)}/postgres`;
  // The password stays in the owned child environment, never argv or logs.
  const pgSql = async (sql: string, app: boolean) => {
    const result = await command(
      [
        "docker",
        "exec",
        "-i",
        "-e",
        "PGPASSWORD",
        pg.name,
        "psql",
        "-h",
        "127.0.0.1",
        "-U",
        app ? role : "postgres",
        "-d",
        "postgres",
        "-X",
        "-qAt",
        "-v",
        "ON_ERROR_STOP=1",
      ],
      {
        input: sql,
        required: false,
        env: { ...process.env, PGPASSWORD: app ? appPassword : ownerPassword },
      },
    );
    if (result.returncode) {
      // Keep the actual diagnostics privately; no credential or environment dump.
      appendFileSync(join(run, "postgres-query-errors.log"), result.stderr);
      throw runtimeError(`owned PostgreSQL diagnostic failed exit=${String(result.returncode)}`);
    }
    return parseJson(result.stdout.trim());
  };
  const seam: Seam = {
    command,
    ownedRows: (name) => ownedRows(name, command),
    identityGone,
    pgSql,
    spawnServer,
    probeSetup,
    portClosed,
    postInputs: () => postInputs(lane),
    emit,
    actor,
  };
  const storage = join(run, "storage");
  mkdirSync(storage, { mode: 0o700 });
  const name = "fvoci-v060-vue-pg-current-" + hex(6);
  const { head, flow, owner, current } = lane;
  const serverEnv: Record<string, string> = {
    PATH: "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: hex(32) }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: JSON.stringify({ fixture: hex(32) }),
    ENCRYPTION_ACTIVE_KEY_ID: "fixture",
    FVOCI_DATABASE_BACKEND: "postgres",
    POSTGRES_USER: "postgres",
    POSTGRES_DB: "postgres",
    POSTGRES_PASSWORD: ownerPassword,
    FVOCI_DB_HOST: "127.0.0.1:" + String(pg.port),
    FVOCI_APP_ROLE: role,
    FVOCI_APP_PASSWORD: appPassword,
    DATABASE_APP_URL: appUrl,
    MEILI_MASTER_KEY: masterKey,
    FVOCI_MEILI_URL: env("FVOCI_MEILI_URL"),
    FVOCI_MEILI_KEY_FILE: meiliKey,
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
  for (const [file, body] of [
    [join(run, "environment.private.json"), JSON.stringify(serverEnv)],
    [
      join(run, "environment.private.sh"),
      Object.entries(serverEnv)
        .map(([key, value]) => `export ${key}=${shellQuote(value)}\n`)
        .join(""),
    ],
  ] as const) {
    const fd = openSync(file, "wx", 0o600);
    try {
      fchmodSync(fd, 0o600);
      writeFileSync(fd, body);
    } finally {
      closeSync(fd);
    }
  }
  const bundleHashes = Object.fromEntries(
    Object.entries(lane.binaries).map(([path, record]) => [path, record.sha256]),
  );
  const receipt: Receipt = {
    source: head,
    tree: lane.tree,
    compiled_source: head,
    root_owner: owner,
    started_utc: now(),
    driver_sha256: sha(postgresDriver),
    selected_flow: flow,
    current_binding: current.manifestPath,
    current_binding_sha256: sha(current.manifestPath),
    container: name,
    image,
    postgres_image: pgImage,
    meili_image: meiliImage,
    network:
      "app host network with loopback port0; PG/Meili maintained isolated container loopback port0 mappings; no app network namespace isolation",
    scope:
      "one actual PG18 normal migrate--start/current Vue/native ON tracer, restricted app role and real scoped search/startup consumers; full0.6/Turso/search correctness/OFF/restore/fullCI/shipping pending",
    runtime_abi: current.abi,
    actual_cohort_ELF_hashes: bundleHashes,
    source_count: Object.keys(lane.before.tracked).length,
    external_count: Object.keys(lane.before.external).length,
    static_count: Object.keys(current.assets.dist_files).length,
    actual_json_reporter_source: { path: reporterSource, sha256: lane.reporterSha },
    static_build_source: current.assets.source,
    environment_names: Object.keys(serverEnv).sort(),
    new_owned_storage: storage,
    host_uid: process.getuid?.(),
    host_gid: process.getgid?.(),
    browser_retries: 0,
    workers: 1,
    application_role: role,
    credential_separation:
      "normal prepare creates NOSUPERUSER/NOBYPASSRLS role; existing exec_server removes preparation-only credentials, app role pool is independently observed; owner URL only in private provisioning fixture process",
    original_failure_policy:
      "retain raw logs/traces and bounded native/ID/receipt diagnostic without auth secret values; no reset/relaxed assertions",
    phase: "container-prepare",
    owned_container_absent: null,
    owned_loopback_port_closed: null,
    loopback_port_observation: "not-observed",
    recorded_process_identities_retired: null,
  };
  write(join(run, "start.json"), receipt);
  const state: InsideState = {
    receipt,
    run,
    storage,
    name,
    code: 1,
    created: false,
    serverRow: null,
    serverProcess: null,
    serverLog: null,
    base: null,
    bun: lane.bun,
    sourceBefore: lane.sourceBefore,
    browserInputs: null,
  };
  trapInterrupts();
  try {
    await body(lane, state, seam, { role, masterKey, ownerUrl, serverEnv, pgPort: pg.port });
  } catch (error) {
    failureCheckpoint(receipt, run, receipt.browser_exit ?? null, {
      error,
      extraKeys: browserPacketKeys,
    });
    state.code ||= 1;
  }
  return finalize(state, seam);
}

interface Secrets {
  role: string;
  masterKey: string;
  ownerUrl: string;
  serverEnv: Record<string, string>;
  pgPort: number;
}
async function body(lane: Lane, state: InsideState, seam: Seam, secrets: Secrets): Promise<void> {
  const { receipt, run, name, storage } = state;
  const { head, flow, current } = lane;
  const { role, masterKey } = secrets;
  const log = (file: string) => ({ log: join(run, file) });
  await seam.command(
    [
      "docker",
      "create",
      "--name",
      name,
      "--network",
      "host",
      "--user",
      "0",
      "--label",
      "fvoci.owner=" + lane.owner,
      "--label",
      "fvoci.test-run=v060-current-normal-vue-pg",
      "--mount",
      `type=bind,src=${storage},dst=/fvoci/storage`,
      "--entrypoint",
      "/bin/sleep",
      image,
      "1800",
    ],
    log("container-create.log"),
  );
  state.created = true;
  await seam.command(["docker", "start", name], log("container-start.log"));
  await seam.command(
    [
      "docker",
      "exec",
      name,
      "/bin/sh",
      "-ec",
      "mkdir -p /fvoci/bin /fvoci/inputs /srv/fvoci-web; chmod 0700 /fvoci/inputs; ldd --version | head -1",
    ],
    log("runtime-abi.log"),
  );
  const copies = [
    [lane.server, "/fvoci/bin/fvoci-server"],
    [lane.migrate, "/fvoci/bin/fvoci-migrate"],
    [lane.engine, "/fvoci/bin/collab-engine"],
    [join(run, "environment.private.sh"), "/fvoci/inputs/environment.sh"],
  ] as const;
  const executables = copies.slice(0, -1).map(([, destination]) => destination);
  for (const [source, destination] of copies)
    await seam.command(["docker", "cp", source, name + ":" + destination]);
  await seam.command(["docker", "exec", name, "chown", "0:0", ...copies.map(([, d]) => d)]);
  await seam.command(["docker", "exec", name, "chmod", "0755", ...executables]);
  const runtimeLdd = (
    await seam.command([
      "docker",
      "exec",
      name,
      "/bin/sh",
      "-ec",
      '. /etc/os-release; test "$ID" = ubuntu; test "$VERSION_ID" = 26.04; for binary do ldd "$binary"; done',
      "fvoci-runtime-abi",
      ...executables,
    ])
  ).stdout;
  writeFileSync(join(run, "native-runtime-abi.log"), runtimeLdd);
  assert.ok(!runtimeLdd.includes("not found"), "Ubuntu26 runtime ELF dependencies missing");
  await seam.command(["docker", "exec", name, "chmod", "0600", "/fvoci/inputs/environment.sh"]);
  await seam.command(["docker", "cp", lane.dist + "/.", name + ":/srv/fvoci-web"]);
  const hashes = (await seam.command(["docker", "exec", name, "sha256sum", ...executables])).stdout;
  assert.ok(
    deepEquals(
      hashes
        .split("\n")
        .filter((line) => line !== "")
        .map((line) => line.trim().split(/\s+/)[0]),
      [lane.server, lane.migrate, lane.engine].map((path) => lane.binaries[path]?.sha256),
    ),
  );
  writeFileSync(join(run, "copied-executable-hashes.log"), hashes);
  receipt.phase = "server-startup";
  const serverLogPath = join(run, "normal-server.log");
  state.serverLog = openSync(serverLogPath, "w");
  state.serverProcess = seam.spawnServer(
    [
      "docker",
      "exec",
      name,
      "/bin/sh",
      "-ec",
      ". /fvoci/inputs/environment.sh; exec /fvoci/bin/fvoci-migrate --start",
    ],
    state.serverLog,
  );
  const deadline = performance.now() + 10_000;
  for (;;) {
    const matched = readyPattern.exec(readText(serverLogPath));
    if (matched) {
      state.base = matched[1] as string;
      break;
    }
    assert.ok(
      running(state.serverProcess),
      "normal entrypoint exited before listen; see actual raw log",
    );
    assert.ok(
      performance.now() < deadline,
      "normal entrypoint did not listen within unchanged10s process observation",
    );
    await Bun.sleep(20);
    checkInterrupt();
  }
  const base = state.base;
  Object.assign(receipt, { phase: "server-ready", loopback_port_observation: "observed" });
  const rows = await seam.ownedRows(name);
  const candidates = rows.filter(
    (row) => row.args === "/fvoci/bin/fvoci-server" && !row.already_retired_at_observation,
  );
  assert.ok(candidates.length === 1);
  const serverRow = candidates[0] as Row;
  state.serverRow = serverRow;
  assert.ok(serverRow.uid === 1000 && serverRow.gid === 1000);
  const setup = await seam.probeSetup(base);
  assert.ok(setup.status === 200 && (setup.body as Json | null)?.needed === true);
  const flags = (await seam.pgSql(restrictedRoleQuery, true)) as Json & {
    rls: Record<string, Json>;
  };
  const registry =
    readText(join(root, "src/db/migrate.rs"))
      .split("const POSTGRES_STEPS:")
      .slice(1)
      .join("const POSTGRES_STEPS:")
      .split("];")[0] ?? "";
  const steps = [
    ...registry.matchAll(
      /include_str!\("\.\.\/\.\.\/migrations\/postgres\/060\/([0-9]{2})_[a-z_]+\.sql"\),\s*"([0-9a-f]{64})"/g,
    ),
  ];
  const versions = steps.map((step) => Number(step[1]));
  const ledger = steps.map((step, index) => [versions[index], "fvoci-postgres-060", step[2]]);
  assert.ok(flags.user === role && flags.version === "180003");
  assert.ok(
    flags.superuser === false &&
      flags.bypassrls === false &&
      flags.owns_schema === false &&
      flags.owns_tables === false,
  );
  assert.ok(
    deepEquals(flags.versions, versions) &&
      deepEquals(
        versions,
        Array.from({ length: 12 }, (_, index) => index + 1),
      ),
  );
  assert.ok(deepEquals(flags.ledger, ledger));
  assert.ok(
    isRecord(flags.rls) &&
      Object.keys(flags.rls).length === 5 &&
      Object.values(flags.rls).every((row) => row.enabled === true),
  );
  assert.ok(
    flags.rls.revisions?.forced === true && flags.rls.wiki_create_commands?.forced === true,
  );
  const keyMetadata = (
    await seam.command(["docker", "exec", name, "stat", "-c", "%u %g %a", meiliKey])
  ).stdout.trim();
  assert.ok(keyMetadata === "0 1000 640");
  const scopedKey = (
    await seam.command(["docker", "exec", "--user", "1000", name, "cat", meiliKey])
  ).stdout.trim();
  assert.ok(scopedKey.length >= 16 && scopedKey !== masterKey);
  assert.ok(readText(serverLogPath).includes("meilisearch enabled"));
  assert.ok(readText(serverLogPath).includes("outbox dispatcher started"));
  Object.assign(receipt, {
    baseURL: base,
    actual_server: serverRow,
    actual_process_rows_at_ready: rows,
    actual_setup_needed: true,
    actual_restricted_role_schema_rls: flags,
    actual_scoped_search_key_metadata: keyMetadata,
    actual_scoped_search_key_distinct_from_master: true,
    actual_search_and_outbox_started: true,
  });
  write(join(run, "normal-main-ready.json"), receipt);
  const browserEnv: Record<string, string> = {
    TMPDIR: env("TMPDIR"),
    ...(current.grant.executionMode !== "orca-local" ? { CI: "true" } : {}),
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: env("BUN_RUNTIME_TRANSPILER_CACHE_PATH"),
    PATH: env("PATH"),
    LANG: process.env.LANG ?? "C.UTF-8",
    PLAYWRIGHT_BASE_URL: base,
    FVOCI_E2E_SELECTED_BACKEND: "postgres",
    FVOCI_E2E_SELECTED_FLOW: flow,
    ...(flow === "on" ? { FVOCI_E2E_SELECTED_AUXILIARY: "normal-api" } : {}),
    FVOCI_E2E_SELECTED_SOURCE: head,
    FVOCI_E2E_SELECTED_COMPILED_SOURCE: head,
    FVOCI_E2E_RESULT_DIR: run,
    FVOCI_E2E_ADMIN_DATABASE_URL: secrets.ownerUrl,
    PLAYWRIGHT_JSON_OUTPUT_FILE: join(run, "playwright-result.private.json"),
    CARGO_TARGET_DIR: dirname(dirname(lane.fixture)),
    PASSWORD_PEPPER_KEYS: secrets.serverEnv.PASSWORD_PEPPER_KEYS as string,
    PASSWORD_PEPPER_ACTIVE_KEY_ID: secrets.serverEnv.PASSWORD_PEPPER_ACTIVE_KEY_ID as string,
  };
  if (process.env.PLAYWRIGHT_BROWSERS_PATH)
    browserEnv.PLAYWRIGHT_BROWSERS_PATH = process.env.PLAYWRIGHT_BROWSERS_PATH;
  const chromium = (
    await seam.command(
      [
        lane.bun,
        "--eval",
        "import { chromium } from '@playwright/test'; console.log(chromium.executablePath());",
      ],
      { env: browserEnv, cwd: join(root, "apps/web") },
    )
  ).stdout.trim();
  assert.ok(chromium.startsWith("/") && statSync(chromium, { throwIfNoEntry: false })?.isFile());
  const browserInputs: Browser = {
    bun: { path: lane.bun, sha256: sha(lane.bun) },
    chromium: { path: chromium, sha256: sha(chromium) },
    chromium_directory_files: treeHashes(dirname(chromium)),
  };
  state.browserInputs = browserInputs;
  write(join(run, "actual-browser-inputs.json"), browserInputs);
  // Bun itself, without bunx's node shim, for both selected flows.
  const args = browserArgs(lane.bun, playwrightCli, lane.spec);
  receipt.phase = "browser";
  Object.assign(receipt, {
    browser_command: args,
    browser_environment_names: Object.keys(browserEnv).sort(),
    browser_start_utc: now(),
  });
  write(join(run, "browser-start.json"), receipt);
  const started = performance.now();
  const result = await seam.command(args, {
    log: join(run, "browser.log"),
    required: false,
    env: browserEnv,
    cwd: join(root, "apps/web"),
  });
  state.code = result.returncode;
  const code = state.code;
  if (code !== 0) recordBrowserFailure(receipt, run, code);
  const reportPath = join(run, "playwright-result.private.json");
  Object.assign(receipt, {
    browser_exit: code,
    browser_seconds: (performance.now() - started) / 1000,
    browser_end_utc: now(),
    browser_log_sha256: sha(join(run, "browser.log")),
  });
  if (existsSync(reportPath) && !("actual_json_report_sha256" in receipt)) {
    chmodSync(reportPath, 0o600);
    receipt.actual_json_report_sha256 = sha(reportPath);
  }
  if (code === 0 && flow === "on") {
    assert.ok(/\b1 passed\b/.test(readText(join(run, "browser.log"))));
    const report = read(reportPath) as Report;
    assert.ok(
      jsonInteger(report.config, "workers") &&
        report.config.workers === 1 &&
        deepEquals(report.errors, []),
    );
    assert.ok(
      report.stats.expected === 1 &&
        ["unexpected", "flaky", "skipped"].every((field) => report.stats[field] === 0),
    );
    assert.ok(report.suites.length === 1 && report.suites[0]?.specs.length === 1);
    const cases = report.suites[0].specs[0]?.tests ?? [];
    assert.ok(cases.length === 1 && cases[0]?.results.length === 1);
    const actual = cases[0].results[0] as Json;
    assert.ok(actual.status === "passed" && actual.retry === 0);
    const tracer = attachment(actual, "selected-vue-native-readback.json");
    assert.ok(
      tracer.selected === "postgres" &&
        (tracer.canonicalEmojiOracleControls as unknown[]).length === 6,
    );
    assert.ok((tracer.nativeHistoryOracleControls as unknown[]).length === 2);
    assert.ok(tracer.firstAck !== tracer.finalAck && tracer.creatorId !== tracer.freshActorId);
    const document = tracer.document as Json;
    privateJson(join(run, "selected-vue-native-readback.private.json"), tracer);
    receipt.typed_tracer_receipt_sha256 = sha(
      join(run, "selected-vue-native-readback.private.json"),
    );
    const aux = attachment(actual, "selected-wiki-auxiliary-mounted.json");
    assert.ok(aux.source === head && aux.compiledSource === head);
    assert.ok(aux.workspaceId === tracer.workspaceId && aux.documentId === document.id);
    assert.ok(
      aux.readerId === tracer.freshActorId && aux.nativeBodyAndManualRevisionUnchanged === true,
    );
    for (const phase of ["ownerMounted", "freshMounted", "reloadedMounted", "afterDenialMounted"]) {
      const observed = aux[phase] as Json & { responses: Json[] };
      assert.ok(
        deepEquals(
          observed.responses.map((r) => r.consumer),
          ["tags", "task-origins", "task-projects", "comments"],
        ),
      );
      assert.ok(observed.responses.every((r) => r.status === 200));
      assert.ok(
        ["tags", "origins", "projects", "comments"].every(
          (key) => ((observed[key] as Json).items as unknown[]).length > 0,
        ),
      );
    }
    const denials = aux.denials as Json[];
    assert.ok(denials.length === 8 && denials.every((r) => r.status === 404));
    privateJson(join(run, "selected-wiki-auxiliary-mounted.private.json"), aux);
    receipt.typed_auxiliary_receipt_sha256 = sha(
      join(run, "selected-wiki-auxiliary-mounted.private.json"),
    );
    const facts = (await seam.pgSql(
      `SELECT jsonb_build_object('workspace',(SELECT id FROM fvoci.workspaces WHERE slug='acme'),
          'actors',(SELECT jsonb_agg(jsonb_build_object('id',u.id,'email',u.email,'role',m.role))
                    FROM fvoci.users u JOIN fvoci.memberships m ON m.user_id=u.id
                    JOIN fvoci.workspaces w ON w.id=m.workspace_id WHERE w.slug='acme'),
          'open_client_roles',(SELECT jsonb_agg(DISTINCT usename) FROM pg_stat_activity
                               WHERE backend_type='client backend' AND pid<>pg_backend_pid()))`,
      false,
    )) as { workspace: unknown; actors: Json[]; open_client_roles: unknown };
    const tenant = canonicalUuid(facts.workspace);
    assert.ok(tenant === tracer.workspaceId && document.workspaceId === tenant);
    assert.ok(
      deepEquals(
        new Set(facts.actors.map((a) => a.id)),
        new Set([tracer.creatorId, tracer.freshActorId]),
      ),
    );
    assert.ok(
      deepEquals(
        facts.actors.map((a) => [a.email, a.role] as [string, string]).sort(compareTuples),
        [
          ["admin@example.com", "owner"],
          ["collab-member@example.com", "member"],
        ],
      ),
    );
    assert.ok(
      deepEquals(facts.open_client_roles, [role]),
      "preparation/actor provisioning owner connections must be closed",
    );
    const wrong = randomUUID();
    const hidden = await seam.pgSql(
      `BEGIN READ ONLY; SET LOCAL app.tenant_id='${wrong}'; SELECT jsonb_build_object('documents',(SELECT count(*) FROM fvoci.documents),'commands',(SELECT count(*) FROM fvoci.wiki_create_commands),'revisions',(SELECT count(*) FROM fvoci.revisions)); COMMIT;`,
      true,
    );
    assert.ok(deepEquals(hidden, { documents: 0, commands: 0, revisions: 0 }, true));
    const durable = (await seam.pgSql(
      `BEGIN READ ONLY; SET LOCAL app.tenant_id='${tenant}';
          SELECT jsonb_build_object('documents',(SELECT count(*) FROM fvoci.documents),
            'commands',(SELECT count(*) FROM fvoci.wiki_create_commands),
            'bound_receipts',(SELECT count(*) FROM fvoci.wiki_create_commands c JOIN fvoci.documents d ON d.id=c.document_id AND d.workspace_id=c.workspace_id),
            'manual_revisions',(SELECT count(*) FROM fvoci.revisions WHERE reason='manual'),
            'native_bytes',(SELECT coalesce(sum(octet_length(state)),0) FROM fvoci.document_states)+(SELECT coalesce(sum(octet_length(payload)),0) FROM fvoci.document_collab_updates),
            'tail',(SELECT max(tail_seq) FROM fvoci.document_states),
            'native_op_receipts',(SELECT count(*) FROM fvoci.document_collab_op_receipts)); COMMIT;`,
      true,
    )) as Record<string, number>;
    if (process.env.FVOCI_E2E_SELECTED_AUXILIARY === "normal-api") {
      const project = (aux.fixture as { project: Json }).project;
      const projectId = canonicalUuid(project.id),
        rootId = canonicalUuid(project.rootDocumentId);
      assert.ok(rootId !== document.id);
      const extra = await seam.pgSql(
        `BEGIN READ ONLY; SET LOCAL app.tenant_id='${tenant}'; SELECT jsonb_build_object('root',(SELECT id FROM fvoci.documents WHERE workspace_id='${tenant}' AND project_id='${projectId}' AND id='${rootId}' AND parent_id IS NULL AND deleted_at IS NULL),'wiki_count',(SELECT count(*) FROM fvoci.documents WHERE project_id IS NULL),'project_count',(SELECT count(*) FROM fvoci.documents WHERE project_id='${projectId}')); COMMIT;`,
        true,
      );
      assert.ok(deepEquals(extra, { root: rootId, wiki_count: 1, project_count: 1 }, true));
      assert.ok(durable.documents === 2);
      assert.ok(
        durable.commands === 1 && durable.bound_receipts === 1 && durable.manual_revisions === 1,
      );
      receipt.actual_auxiliary_project_root = extra;
    } else
      assert.ok(
        durable.documents === 1 &&
          durable.commands === 1 &&
          durable.bound_receipts === 1 &&
          durable.manual_revisions === 1,
      );
    assert.ok(
      (durable.native_bytes as number) > 2 &&
        (durable.tail as number) >= 1 &&
        (durable.native_op_receipts as number) >= 1,
    );
    Object.assign(receipt, {
      actual_browser_tests: 1,
      retries: 0,
      ignored: 0,
      actual_actor_and_closed_provisioning: facts,
      actual_wrong_tenant_hidden: hidden,
      actual_restricted_role_durable_commit: durable,
      tested_product_flow:
        "identical actual currentVue setup/login/stable wiki create/nonempty nativeON/matching durableACK/manual DSSV reconstruction/fresh cookie actor and new connection native-body-ID-permission-history readback",
    });
    receipt.phase = "restart";
    const context: RestartContext = {
      run,
      name,
      browserEnv,
      code,
      head,
      tree: lane.tree,
      compiledHead: head,
      identity: current.grant,
      parentDriver: postgresDriver,
      binaries: lane.binaries,
      server: lane.server,
      migrate: lane.migrate,
      engine: lane.engine,
      distFiles: current.assets.dist_files,
      abiFiles: current.abi.host_runtime_files,
      browserInputs,
      root,
      bun: lane.bun,
      spec: lane.spec,
      playwrightCli,
      before: lane.before,
      sourceBefore: lane.sourceBefore,
      inputCheck: () => inputCheck(lane.before, head, lane.tree),
      treeHashes,
      dist: lane.dist,
      storage,
      serverRow,
      serverProcess: state.serverProcess,
      base,
      receipt,
      projectBrowser: true,
      role,
      masterKey,
    };
    receipt.current_schema_server_restart = await restartSameApp(context, seam);
  }
  if (code === 0 && flow === "off") {
    const titles = validateOffReport(read(reportPath), "postgres");
    Object.assign(receipt, {
      actual_browser_tests: 8,
      retries: 0,
      ignored: 0,
      actual_off_titles: titles,
      tested_product_flow:
        "immutable OFF8 actual Vue CAS/replay/native history/task/note/owner-transition/current revoke/lost-response newer head",
    });
  }
}
// Python tuple ordering for (email, role) pairs.
const compareTuples = (a: [string, string], b: [string, string]) =>
  a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0;
// Exclusive JSON write, then mode 0600.
function privateJson(path: string, value: unknown): void {
  write(path, value);
  chmodSync(path, 0o600);
}

export async function main(argv: string[] = process.argv.slice(2)): Promise<number> {
  assertNoEnvFile();
  const runs = env("FVOCI_CI_SELECTED_RUNS");
  const lane = await rebind();
  assert.ok(
    process.env.FVOCI_ROOT_RUN_OWNER === lane.owner,
    "explicit root-owned foreground allocation marker required",
  );
  if (argv.length === 0) {
    trapInterrupts();
    const verifySeam = { command, identityGone, portClosed };
    return parent(
      lane.current.run,
      {
        head: lane.head,
        tree: lane.tree,
        owner: lane.owner,
        flow: lane.flow,
        sourceBefore: lane.sourceBefore,
      },
      {
        command,
        verifyFixturesClosed: (run) => verifyFixturesClosed(run, verifySeam),
        postInputs: () => postInputs(lane),
        emit,
      },
    );
  }
  const [mode, given] = argv;
  assert.ok(argv.length === 2 && (mode === "--pg-ready" || mode === "--inside") && given);
  const run = resolved(given);
  assert.ok(
    run === lane.current.run &&
      dirname(run) === resolve(runs) &&
      /^root-current-postgres-[0-9a-f]{12}$/.test(basename(run)),
  );
  const facts = statSync(run);
  assert.ok(facts.isDirectory() && facts.uid === 1000 && (facts.mode & 0o777) === 0o700);
  if (mode === "--pg-ready") {
    write(join(run, "fixture-pg-ready.json"), await fixtureInfo("pg"));
    trapInterrupts();
    const result = await command(
      ["bash", meiliScript, ...postgresDriverCommand(), "--inside", run],
      { log: join(run, "owned-meili-and-tracer.log"), required: false },
    );
    return result.returncode;
  }
  return inside(lane, run);
}

if (import.meta.main) process.exit(await main());
