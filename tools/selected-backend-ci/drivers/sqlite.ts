// Root-owned, one-shot current development SQLite normal-main/current Vue lane
// driver: one owned Ubuntu container runs migrate --start against a new owned
// database and storage, then the selected Vue spec, then the same-app restart.
// No compilation, package install, product patch, migration reset or
// PostgreSQL credential. Prints only its final JSON summary on stdout and
// exits with the lane's first failure code.
import { deepEquals } from "bun";
import { Database } from "bun:sqlite";
import { strict as assert } from "node:assert";
import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import process from "node:process";
import {
  checkpointPrefix,
  env,
  failureDigest,
  isFile,
  read,
  root,
  sha,
  type PreparationStep,
} from "../io.ts";
import type { Environment } from "../io.ts";
import type { Browser, Inputs } from "../types.ts";
import {
  loadCurrent,
  restoreSpec,
  validateOffReport,
  validateRestoreReport,
  productSpec,
  validateProductReport,
  type Current,
} from "./binding.ts";
import {
  actor,
  assertNoEnvFile,
  browserPacketKeys,
  checkInterrupt,
  cleanupAttempt,
  cleanupScope,
  command,
  copiedHashes,
  emit,
  failureCheckpoint,
  identityGone,
  inodeOf,
  inputCheck,
  knownBrowserCheckpoint,
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
import { AUTH_SPEC, authEventRows } from "../../turso/ui-auth-session.ts";
import * as auditModule from "../../turso/ui-audit.ts";
import {
  FIXTURE_BUDGET,
  get,
  list,
  record,
  requestPublic,
  inviteFinalActor,
  token,
} from "../../turso/ui-common.ts";
import {
  assertSelectedSuccessfulWrites,
  type SelectedNativeReceipt,
  type SelectedSyncWrite,
} from "../../turso/ui-selected-native-policy.ts";

import {
  authBrowserEnvironment,
  authConfig,
  authLaneVerdict,
  authServerEnvironment,
  browserArgs,
  startAuthSink,
  stopAuthSink,
  type AuthSink,
} from "./postgres.ts";
import { equalSpec, measureBrowser, measurementInputs, validateEqualReport } from "./equal-work.ts";
import {
  browserPort,
  restartSameApp,
  type RestartContext,
  type Seam as RestartSeam,
} from "./restart.ts";

export const sqliteDriver = import.meta.path;
export const image =
  "ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7";

/** Bind only the two existing fresh-open/reload writes to complete native rows.
 * Original bytes and every original receipt/update remain independently exact. */
export async function sqliteReadbackAppendWitness(input: {
  original: unknown;
  current: unknown;
  before: unknown;
  prepared: unknown;
  workspaceId: string;
  documentId: string;
  actorId: string;
  writes: unknown;
  clocks: [string, string];
}) {
  const { integer, uuidHex } = auditModule;
  const hash = (value: unknown) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
  const original = input.original,
    current = input.current;
  for (const key of ["state", "cutoff", "content", "text", "version", "revisions"])
    assert.ok(
      deepEquals(get(original, key), get(current, key), true),
      "original wiki bytes changed",
    );
  const oldTail = get(original, "tail");
  assert.ok(Number.isSafeInteger(oldTail) && Number(oldTail) >= 0);
  assert.ok(get(current, "tail") === Number(oldTail) + 2);
  const added: Record<string, unknown[]> = {};
  for (const key of ["updates", "receipts"]) {
    const old = list(original, key),
      now = list(current, key);
    assert.ok(now.length === old.length + 2 && deepEquals(now.slice(0, old.length), old, true));
    const delta = now.slice(old.length);
    added[key] = delta;
    assert.ok(delta.every((row, i) => get(row, "seq") === Number(oldTail) + i + 1));
  }
  const receipts = added.receipts as SelectedNativeReceipt[];
  assert.ok(Array.isArray(input.writes) && input.writes.length <= 10000);
  const writes = input.writes as SelectedSyncWrite[];
  assert.ok(
    writes.every(
      (w) =>
        w.workspaceId === input.workspaceId &&
        w.documentId === input.documentId &&
        w.actorId === input.actorId,
    ),
  );
  assertSelectedSuccessfulWrites(receipts, writes, input.workspaceId, input.documentId);
  assert.ok(receipts.every((r) => r.actor === input.actorId));
  for (let i = 0; i < 2; i++) {
    const update = added.updates?.[i],
      receipt = receipts[i];
    const hex = get(update, "hex");
    assert.ok(typeof hex === "string" && /^(?:[a-f0-9]{2})+$/.test(hex) && receipt !== undefined);
    assert.ok(
      get(update, "op") === receipt.op &&
        get(update, "seq") === receipt.seq &&
        Buffer.from(hex, "hex").length === receipt.len &&
        createHash("sha256").update(Buffer.from(hex, "hex")).digest("hex") === receipt.sha,
    );
  }
  const rows: Record<string, Record<string, number>> = {};
  for (const [table, columns, key] of [
    [
      "document_collab_updates",
      ["workspace_id", "document_id", "seq", "op_id", "payload", "created_at"],
      "updates",
    ],
    [
      "document_collab_op_receipts",
      [
        "workspace_id",
        "document_id",
        "op_id",
        "seq",
        "payload_len",
        "payload_sha256",
        "actor_user_id",
        "created_at",
      ],
      "receipts",
    ],
  ] as const) {
    const old = list(original, "appendRows", table),
      now = list(current, "appendRows", table);
    assert.ok(now.length === old.length + 2 && deepEquals(now.slice(0, old.length), old, true));
    rows[table] = {};
    for (const [index, row] of now.entries()) {
      const cells = list(row, "cells"),
        sha = get(row, "sha256");
      assert.ok(
        deepEquals(get(row, "columns"), columns, true) &&
          cells.length === columns.length &&
          sha === hash(cells),
      );
      assert.ok(
        deepEquals(cells[0], ["blob", uuidHex(input.workspaceId)], true) &&
          deepEquals(cells[1], ["blob", uuidHex(input.documentId)], true),
      );
      assert.ok(get(input.prepared, "fingerprints", table, sha) === 1);
      if (index < old.length) {
        assert.ok(get(input.before, "fingerprints", table, sha) === 1);
        continue;
      }
      assert.ok(!Object.hasOwn(record(input.before, "fingerprints", table), sha));
      const delta = index - old.length,
        receipt = receipts[delta];
      assert.ok(receipt !== undefined);
      const expected =
        key === "updates"
          ? [
              ["integer", String(receipt.seq)],
              ["blob", uuidHex(receipt.op)],
              ["blob", get(added.updates?.[delta], "hex")],
            ]
          : [
              ["blob", uuidHex(receipt.op)],
              ["integer", String(receipt.seq)],
              ["integer", String(receipt.len)],
              ["blob", receipt.sha],
              ["blob", uuidHex(input.actorId)],
            ];
      assert.ok(deepEquals(cells.slice(2, -1), expected, true));
      const time = integer(cells.at(-1));
      assert.ok(
        time >= integer(["integer", input.clocks[0]]) &&
          time <= integer(["integer", input.clocks[1]]),
      );
      assert.ok(!Object.hasOwn(rows[table], sha));
      rows[table][sha] = 1;
    }
    assert.ok(Object.keys(rows[table]).length === 2);
  }
  const publications = sqliteAppendPublicationWitness({ ...input, receipts });
  return Promise.resolve({
    ...publications,
    count: 2 as const,
    rows: { ...rows, ...publications.rows },
  });
}

/** The same complete native publication contract serves readback and PRODUCT4. */
export function sqliteAppendPublicationWitness(input: {
  original: unknown;
  current: unknown;
  before: unknown;
  prepared: unknown;
  workspaceId: string;
  documentId: string;
  receipts: SelectedNativeReceipt[];
  clocks: [string, string];
}) {
  const rows: Record<string, Record<string, number>> = {};
  const actors: Record<string, string> = {};
  const { original, current, receipts } = input;
  const { integer, uuidHex } = auditModule;
  const hash = (value: unknown) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
  assert.ok(receipts.length <= 10000);
  const eventOperations: Record<string, unknown[]> = {};
  const eventIds: Record<string, string> = {};
  const generation = get(current, "writer_generation");
  assert.ok(Number.isSafeInteger(generation) && Number(generation) > 0);
  const publicationIds = new Set<string>(),
    eventSequences = new Set<string>();
  for (const [table, columns] of [
    [
      "events",
      [
        "id",
        "seq",
        "workspace_id",
        "actor_user_id",
        "verb",
        "target_type",
        "target_id",
        "payload",
        "channel",
        "created_at",
      ],
    ],
    [
      "audit_log",
      [
        "id",
        "actor_user_id",
        "workspace_id",
        "verb",
        "target_type",
        "target_id",
        "payload",
        "ip",
        "created_at",
      ],
    ],
  ] as const) {
    const old = list(original, "appendRows", table),
      now = list(current, "appendRows", table);
    assert.ok(
      now.length === old.length + receipts.length &&
        deepEquals(now.slice(0, old.length), old, true),
    );
    rows[table] = {};
    const matched = new Set<string>();
    for (const [index, row] of now.entries()) {
      const cells = list(row, "cells"),
        sha = get(row, "sha256");
      assert.ok(
        deepEquals(get(row, "columns"), columns, true) &&
          cells.length === columns.length &&
          sha === hash(cells),
      );
      assert.ok(get(input.prepared, "fingerprints", table, sha) === 1);
      if (index < old.length) {
        assert.ok(get(input.before, "fingerprints", table, sha) === 1);
        continue;
      }
      assert.ok(!Object.hasOwn(record(input.before, "fingerprints", table), sha));
      const event = table === "events",
        offset = event ? 1 : 0;
      const id = get(cells, 0, 1);
      assert.ok(
        deepEquals(cells[0], ["blob", id], true) &&
          typeof id === "string" &&
          /^[a-f0-9]{32}$/.test(id) &&
          !/^0+$/.test(id) &&
          !publicationIds.has(id),
      );
      publicationIds.add(id);
      assert.ok(deepEquals(cells[2], ["blob", uuidHex(input.workspaceId)], true));
      assert.ok(
        deepEquals(cells[3 + offset], ["text", "document.collab_update_appended"], true) &&
          deepEquals(cells[4 + offset], ["text", "document"], true) &&
          deepEquals(cells[5 + offset], ["blob", uuidHex(input.documentId)], true),
      );
      const matches = receipts.filter(
        (receipt) =>
          deepEquals(cells[event ? 3 : 1], ["blob", uuidHex(receipt.actor)], true) &&
          deepEquals(
            cells[6 + offset],
            [
              "text",
              JSON.stringify({
                documentId: input.documentId,
                opId: receipt.op,
                seq: receipt.seq,
                writerGeneration: generation,
              }),
            ],
            true,
          ),
      );
      assert.ok(matches.length === 1 && matches[0] !== undefined && !matched.has(matches[0].op));
      matched.add(matches[0].op);
      actors[sha] = uuidHex(matches[0].actor);
      const time = integer(cells.at(-1));
      assert.ok(
        time >= integer(["integer", input.clocks[0]]) &&
          time <= integer(["integer", input.clocks[1]]),
      );
      if (event) {
        assert.ok(deepEquals(cells[8], ["text", "web"], true));
        const sequence = integer(cells[1]);
        assert.ok(sequence > 0n && !eventSequences.has(sequence.toString()));
        eventSequences.add(sequence.toString());
        const operation = cells.slice(1, 4);
        assert.ok(
          list(input.prepared, "operations", "events").filter((value) =>
            deepEquals(value, operation, true),
          ).length === 1,
        );
        eventOperations[sha] = operation;
        eventIds[sha] = id;
      } else {
        assert.ok(
          deepEquals(cells[7], ["null"], true) ||
            (Array.isArray(cells[7]) &&
              cells[7].length === 2 &&
              cells[7][0] === "text" &&
              typeof cells[7][1] === "string"),
        );
      }
      assert.ok(!Object.hasOwn(rows[table], sha));
      rows[table][sha] = 1;
    }
    assert.ok(
      matched.size === receipts.length && Object.keys(rows[table]).length === receipts.length,
    );
  }
  return { count: receipts.length, rows, eventOperations, eventIds, actors };
}

/** Read only the separate, completed observer packet, never its mixed log. */
export function readSelectedSqlitePacket(path: string): Record<string, unknown> {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = fstatSync(fd);
    assert.ok(
      before.isFile() &&
        before.nlink === 1 &&
        before.uid === process.getuid?.() &&
        before.gid === process.getgid?.() &&
        (before.mode & 0o777) === 0o600 &&
        before.size > 0 &&
        before.size < 64 * 1024 * 1024,
      "SQLite native packet ownership/bound refused",
    );
    const bytes = readFileSync(fd),
      after = fstatSync(fd);
    assert.ok(
      bytes.length === before.size &&
        after.size === before.size &&
        after.mtimeMs === before.mtimeMs &&
        after.ctimeMs === before.ctimeMs,
      "SQLite native packet changed",
    );
    const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    assert.ok(
      typeof value === "object" && value !== null && !Array.isArray(value),
      "SQLite native packet object required",
    );
    return value as Record<string, unknown>;
  } finally {
    closeSync(fd);
  }
}
const checkpointKeys = [
  "known_driver_checkpoint",
  "preparation_command_exit",
  ...browserPacketKeys,
] as const;

// The I/O this driver owns, replaceable as one boundary in tests.
export interface Seam extends RestartSeam {
  trap: () => void;
  loadCurrent: (lane: "sqlite", driver: string) => Promise<Current>;
  inputCheck: (before: Inputs, head: string, tree: string) => Promise<Inputs>;
  treeHashes: (directory: string) => Record<string, string>;
  migrationRows: (db: string) => unknown[][];
  validateOffReport: (report: unknown, backend: "sqlite") => string[];
  restart: (context: RestartContext, seam: RestartSeam) => Promise<Receipt>;
  products: (state: State, seam: Seam) => Promise<RestartContext["sqliteProducts"]>;
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
  sink?: AuthSink;
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
  "restore_browser_exit",
  "restore_failure_code",
  ...browserPacketKeys,
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

// Report only fixed codes and the allowlisted reporter location. Bootstrap
// failures can precede JSON reporting; never publish their raw log or path.
export function recordRestoreFailure(
  state: State,
  code: number,
  log: string,
  report: string,
): void {
  const receipt = state.receipt;
  Object.assign(receipt, {
    restore_browser_exit: code,
    restore_failure_code: "SELECTED_RESTORE_BROWSER_FAILED",
    known_browser_test: null,
    known_browser_status: null,
    known_browser_checkpoint: null,
    browser_report_state: "report-missing",
  });
  try {
    chmodSync(log, 0o600);
    if (/\bEACCES: permission denied, mkdir\b/.test(readText(log)))
      receipt.restore_failure_code = "PLAYWRIGHT_OUTPUT_EACCES";
  } catch (error) {
    secondary(receipt, "restore-log-diagnostic-failed", error);
  }
  if (existsSync(report)) {
    try {
      chmodSync(report, 0o600);
      receipt.sqlite_restore_report_sha256 = sha(report);
      Object.assign(receipt, knownBrowserCheckpoint(read(report), "sqlite-restore"));
    } catch (error) {
      receipt.browser_report_state = "report-unreadable";
      secondary(receipt, "restore-report-diagnostic-failed", error);
    }
  }
  recordFailure(state, code, undefined, log);
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
  state.serverLog = openSync(log, "wx", 0o600);
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
  const equal = state.current.manifest.purpose === "equal-work";
  const measurements = join(run, "equal-work-samples");
  if (equal) state.browserEnv.FVOCI_E2E_EQUAL_WORK_DIR = measurements;
  const sink = state.sink;
  if (sink !== undefined)
    Object.assign(state.browserEnv, authBrowserEnvironment(state.head, state.tree, sink));
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
  if (equal) args[args.indexOf("--config") + 1] = "e2e-pending/equal-work-playwright.config.ts";
  if (sink !== undefined) args[args.indexOf("--config") + 1] = authConfig;
  const eventsBefore = sink === undefined ? null : authEventRows(authEvents(db), "sqlite");
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
  const invoke = async () => {
    const result = await seam.command(args, { log, required: false, env: state.browserEnv, cwd: web });
    state.code = result.returncode;
    if (state.code !== 0) recordFailure(state, state.code, undefined, log);
    return result;
  };
  const measured = equal
    ? await measureBrowser(measurements, state.name, state.base as string, invoke, seam.command)
    : null;
  const result = measured?.result ?? (await invoke());
  state.code = result.returncode;
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
  assert.ok(deepEquals(inodeOf(db), state.databaseInode), "no replacement/reset DB");
  if (equal) {
    receipt.purpose = "equal-work";
    receipt.equal_work = {
      inputs: measurementInputs(
        state.before,
        state.current.build,
        state.current.assets,
        browserInputs,
        image,
      ),
      browser: validateEqualReport(read(report), "sqlite", flow),
      samples: measured?.samples,
    };
    Object.assign(receipt, {
      actual_browser_tests: 1,
      retries: 0,
      ignored: 0,
      actual_fixture_pin_checks_completed: true,
    });
    return;
  }
  if (eventsBefore !== null) {
    const after = authEventRows(authEvents(db), "sqlite");
    const binding = { backend: "sqlite", flow, source: state.head, tree: state.tree } as const;
    const verdict = authLaneVerdict(read(report), binding, eventsBefore, after);
    privateText(join(run, "auth-session-verdict.private.json"), JSON.stringify(verdict));
    Object.assign(receipt, {
      purpose: "auth-session",
      auth_session: {
        cases: verdict.cases,
        events: verdict.events,
        verdict_sha256: sha(join(run, "auth-session-verdict.private.json")),
      },
      actual_browser_tests: 3,
      retries: 0,
      ignored: 0,
      actual_fixture_pin_checks_completed: true,
    });
    return;
  }
  if (flow === "on")
    assert.ok(/\b1 passed\b/.test(readText(log)), "exact one selected browser test");
  else receipt.actual_off_titles = seam.validateOffReport(read(report), "sqlite");
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
    // Retain the original one-test invocation and receipt. Restore is a
    // separately counted, mandatory invocation on this same SQLite server.
    const restoreArgs = browserArgs(
      state.bun,
      join(root, "node_modules/playwright/cli.js"),
      restoreSpec,
    );
    restoreArgs[restoreArgs.indexOf("--config") + 1] =
      "e2e-pending/revision-restore-playwright.config.ts";
    const restoreReport = join(run, "restore-playwright-result.private.json");
    const restoreResult = await seam.command(restoreArgs, {
      log: join(run, "restore-browser.log"),
      required: false,
      env: { ...state.browserEnv, PLAYWRIGHT_JSON_OUTPUT_FILE: restoreReport },
      cwd: web,
    });
    state.code = restoreResult.returncode;
    if (state.code !== 0) {
      recordRestoreFailure(state, state.code, join(run, "restore-browser.log"), restoreReport);
      return;
    }
    chmodSync(restoreReport, 0o600);
    validateRestoreReport(read(restoreReport));
    receipt.sqlite_restore_browser = { exit: 0, actual_tests: 1, skipped: 0, retries: 0 };
    receipt.sqlite_restore_report_sha256 = sha(restoreReport);
    receipt.phase = "restart";
    const context = restartContext(state, browserInputs, seam);
    context.sqliteProducts = await seam.products(state, seam);
    receipt.current_schema_server_restart = await seam.restart(context, seam);
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
    // Read only: a pass never gains an empty diagnostic_errors member.
    const diagnostics = receipt.diagnostic_errors;
    const cleanupErrors: unknown[] = Array.isArray(diagnostics)
      ? [...(diagnostics as unknown[])]
      : [];
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
    const sink = state.sink;
    if (sink !== undefined) {
      const stopped = await attempt("smtp-sink-stop-failed", () => stopAuthSink(sink));
      receipt.auth_smtp_sink = stopped;
      if (stopped === null || stopped.exit !== -15 || stopped.forced || !stopped.listenerAbsent)
        cleanupErrors.push("owned-smtp-sink-retirement-unconfirmed");
    }
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
    current.manifest.purpose === "equal-work"
      ? equalSpec
      : current.manifest.purpose === "auth-session"
        ? AUTH_SPEC
        : flow === "off"
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
  if (current.manifest.purpose === "equal-work") serverEnv.METRICS_ALLOW_IPS = "127.0.0.1";
  const auth = current.manifest.purpose === "auth-session";
  if (auth) Object.assign(serverEnv, authServerEnvironment(null));
  const environmentFiles = () => {
    privateText(join(run, "environment.private.json"), JSON.stringify(serverEnv));
    privateText(join(run, "environment.private.sh"), shellExports(serverEnv));
  };
  // The auth files wait for the owned sink, which finalize then retires.
  if (!auth) environmentFiles();
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
    if (auth) {
      state.sink = await startAuthSink(run, bun);
      Object.assign(serverEnv, authServerEnvironment(state.sink));
      environmentFiles();
    }
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
// Read-only `auth.%` event observation; a full bound is refused by the caller.
export function authEvents(path: string): unknown[][] {
  const observer = new Database(path, { readonly: true });
  try {
    return observer
      .query(
        "SELECT seq,verb,channel,workspace_id,actor_user_id,target_type,target_id,payload FROM events WHERE verb LIKE 'auth.%' ORDER BY seq LIMIT 10001",
      )
      .values();
  } finally {
    observer.close();
  }
}
/** Source-bound adapter for the existing restart owner, with separate exact
 * PRODUCT4 browser/report/native receipts and no second normal server manager. */
export async function productCallbacks(
  state: State,
  seam: Seam,
): Promise<NonNullable<RestartContext["sqliteProducts"]>> {
  assert.ok(
    state.flow === "on" &&
      state.current.manifest.purpose === undefined &&
      state.serverRow !== null &&
      state.serverProcess !== null &&
      state.serverLog !== null &&
      state.base !== null,
  );
  const manifest = {
    sourceInputs: { source: state.head, tree: state.tree, files: state.before.tracked },
    binaries: Object.fromEntries(
      Object.entries(state.current.build.binaries).map(([path, binary]) => [
        binary.target.name,
        { ...binary, path },
      ]),
    ),
  };
  const target = failureDigest(
    JSON.stringify({ source: state.head, tree: state.tree, inode: state.databaseInode }),
  );
  const oldOrigin = auditModule.captureSelectedRoomLog(
    manifest,
    join(state.run, "normal-server.log"),
    state.serverLog,
    state.serverProcess,
    state.serverRow,
    state.base,
    target,
  );
  const directory = join(state.run, "products");
  mkdirSync(directory, 0o700);
  const sourceConfig = join(root, "src/config.rs");
  assert.ok(sha(sourceConfig) === state.before.tracked["src/config.rs"]);
  const config = readText(sourceConfig);
  const constant = (name: string) => {
    const match = new RegExp("pub const " + name + ": u32 = ([0-9]+);").exec(config);
    assert.ok(match !== null);
    return Number(match[1]);
  };
  const policy = {
    revisionKeep: constant("DEFAULT_REVISION_KEEP"),
    revisionSnapshotIntervalHours: constant("DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS"),
  };
  const mainSource = join(root, "src/main.rs");
  assert.ok(
    sha(mainSource) === state.before.tracked["src/main.rs"] &&
      /FamilyMaintenanceLeasePolicy::new\(\s*std::time::Duration::from_secs\(300\),\s*std::time::Duration::from_secs\(60\),\s*\)/.test(
        readText(mainSource),
      ),
    "selected native maintenance policy changed",
  );
  assert.ok(
    (state.serverEnv.REVISION_KEEP ?? String(policy.revisionKeep)) ===
      String(policy.revisionKeep) &&
      (state.serverEnv.REVISION_SNAPSHOT_INTERVAL_HOURS ??
        String(policy.revisionSnapshotIntervalHours)) ===
        String(policy.revisionSnapshotIntervalHours),
  );
  const namespace = "tui-" + token(10);
  const audit: import("../../turso/ui-audit.ts").Audit = {
    namespaces: [namespace],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: target,
    serverStarts: 1,
    observedFences: [],
  };
  const nativeEnvironment: Record<string, string> = {};
  for (const key of [
    "PATH",
    "LANG",
    "LD_LIBRARY_PATH",
    "TMPDIR",
    "RUNNER_TEMP",
    "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
    "CI",
    "GITHUB_ACTIONS",
    "GITHUB_SHA",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_ATTEMPT",
    "FVOCI_CI_BUN",
    "FVOCI_CI_OWNER",
    "FVOCI_CI_SELECTED_RUNS",
    "FVOCI_ROOT_RUN_OWNER",
    "FVOCI_ROOT_CURRENT_BINDING",
    "FVOCI_ROOT_CURRENT_ALLOCATION",
    "FVOCI_ROOT_RESTART_GRANT",
    "FVOCI_SELECTED_EXECUTION_MODE",
    "FVOCI_SELECTED_LOCAL_ALLOCATION",
    "FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256",
    "FVOCI_LOCAL_RUN_ID",
    "FVOCI_LOCAL_DISPATCH_ID",
    "FVOCI_LOCAL_TASK_ID",
    "FVOCI_LOCAL_ROOT_TERMINAL",
    "ORCA_TERMINAL_HANDLE",
  ]) {
    const value = state.source[key];
    if (value !== undefined) nativeEnvironment[key] = value;
  }
  assert.ok(sha(process.execPath) === state.browserInputs?.bun.sha256);
  const native = async (
    mode: string,
    input: Record<string, unknown>,
    label: string,
    signal?: AbortSignal,
  ) => {
    assert.ok(deepEquals(inodeOf(state.db), state.databaseInode));
    const nativeLog = join(directory, label + ".native.private.log");
    const packetPath = join(directory, label + ".native.private.json");
    privateText(nativeLog, "");
    const result = await seam.command(
      [
        process.execPath,
        "--no-env-file",
        "--config=" + join(root, "bunfig.toml"),
        join(root, "tools/turso/ui-container.ts"),
        "--selected-sqlite-observe",
        mode,
        packetPath,
      ],
      {
        env: nativeEnvironment,
        input: JSON.stringify({ container: state.name, inode: state.databaseInode, input }),
        cwd: root,
        log: nativeLog,
        waitOnInterrupt: true,
        signal,
      },
    );
    assert.ok(result.returncode === 0 && result.stdout === "" && result.stderr === "");
    const parsed = readSelectedSqlitePacket(packetPath);
    assert.ok(
      parsed.connectionClose === "confirmed" &&
        parsed.lifecycleDrain === "confirmed" &&
        parsed.leases === 0,
    );
    assert.ok(deepEquals(inodeOf(state.db), state.databaseInode));
    return parsed;
  };
  const serial = async (label: string, validate: boolean, signal?: AbortSignal) => {
    const start = BigInt(Date.now()) * 1000n;
    const observation = await native("serial-observe", policy, label, signal);
    const end = BigInt(Date.now()) * 1000n;
    if (validate) {
      const checked = auditModule.validateSerialObservation(observation, policy, false, "sqlite");
      auditModule.assertNativeClock(checked, start, end);
      return checked;
    }
    const checked = {
      dataset: record(observation, "dataset"),
      checks: record(observation, "checks"),
    };
    auditModule.assertNativeClock(checked, start, end);
    return checked;
  };
  let original: import("../../turso/ui-audit.ts").SerialObservation | undefined;
  let current: import("../../turso/ui-audit.ts").SerialObservation | undefined;
  let restartedOrigin: import("../../turso/ui-audit.ts").SelectedRoomOrigin | undefined;
  let originalSeed: Record<string, unknown> | undefined;
  let preparedSeed: unknown;
  let storageOriginal: Record<string, string> | undefined;
  const targets: {
    workspaceId: string;
    documentId: string;
    receipts: SelectedNativeReceipt[];
  }[] = [];
  const writes: SelectedSyncWrite[] = [];
  let productPassed = false;
  let productReadbacks: unknown[] = [];
  return Promise.resolve({
    async afterSeedStopped(rows, seed) {
      const retired = await auditModule.selectedRoomRetirement(manifest, oldOrigin, target, rows);
      original = await serial("original", true);
      original.roomHistory = [retired];
      auditModule.assertRoomClosure(original, target);
      storageOriginal = seam.treeHashes(state.storage);
      const seedId = String(get(seed, "document", "id")),
        workspaceId = String(get(seed, "workspaceId"));
      originalSeed = await native(
        "observe",
        { workspaceId, documentIds: [seedId] },
        "original-seed",
      );
    },
    async afterReadback(server) {
      assert.ok(original !== undefined && originalSeed !== undefined);
      assert.ok(
        get(server.readback, "source") === state.head &&
          get(server.readback, "tree") === state.tree &&
          get(server.readback, "freshActorId") === get(server.seed, "freshActorId"),
      );
      restartedOrigin = auditModule.captureSelectedRoomLog(
        manifest,
        server.logpath,
        server.log,
        server.child,
        server.row,
        server.base,
        target,
      );
      const logged = await requestPublic(server.base, "/api/v1/auth/login", {
        email: "Admin@Example.COM",
        password: "supersecret1",
      });
      const bootstrap = await requestPublic(
        server.base,
        "/api/v1/auth/me",
        undefined,
        logged.cookie,
      );
      assert.ok(
        get(bootstrap.value, "isInstanceAdmin") === true &&
          get(bootstrap.value, "userId") === get(logged.value, "userId"),
      );
      const workspace = await requestPublic(
        server.base,
        "/api/v1/workspaces",
        { name: namespace, slug: namespace },
        logged.cookie,
      );
      const workspaceId = String(get(workspace.value, "id"));
      const owner = await inviteFinalActor(
        server.base,
        workspaceId,
        logged.cookie,
        namespace + "-owner@example.invalid",
        "Aa1!" + token(16),
        "owner",
        "owner",
      );
      const member = await inviteFinalActor(
        server.base,
        workspaceId,
        owner.cookie,
        namespace + "-member@example.invalid",
        "Aa1!" + token(16),
        "member",
        "member",
      );
      const outsider = await inviteFinalActor(
        server.base,
        workspaceId,
        owner.cookie,
        namespace + "-outsider@example.invalid",
        "Aa1!" + token(16),
        "member",
        "outsider",
      );
      for (const actor of [owner, member, outsider]) {
        const actual = await requestPublic(server.base, "/api/v1/auth/me", undefined, actor.cookie);
        assert.ok(
          get(actual.value, "userId") === actor.userId &&
            get(actual.value, "email") === actor.email,
        );
        audit.actors[auditModule.uuidHex(actor.userId)] = actor.email;
      }
      audit.workspaces[auditModule.uuidHex(workspaceId)] = namespace;
      current = await serial("prepared", false);
      const nativeActor = (id: string, email?: string) => {
        const rows = list(current, "checks", "users").filter((row) => get(row, "id") === id);
        assert.ok(rows.length === 1 && (email === undefined || get(rows[0], "email") === email));
        return rows[0];
      };
      for (const [actor, role] of [
        [owner, "owner"],
        [member, "member"],
        [outsider, "member"],
      ] as const) {
        nativeActor(actor.userId, actor.email);
        assert.ok(
          list(current, "checks", "memberships").filter(
            (row) =>
              get(row, "workspaceId") === workspaceId &&
              get(row, "userId") === actor.userId &&
              get(row, "role") === role,
          ).length === 1,
        );
      }
      const originalActor = (id: string) => {
        const rows = list(original, "checks", "users").filter((row) => get(row, "id") === id);
        assert.ok(rows.length === 1);
        const sessions = Object.entries(
          record(current, "dataset", "allocations", "sessions"),
        ).filter(
          ([hash, refs]) =>
            !Object.hasOwn(record(original, "dataset", "fingerprints", "sessions"), hash) &&
            list(refs, "actors").includes(auditModule.uuidHex(id)),
        );
        assert.ok(sessions.length === 1 && sessions[0] !== undefined);
        return {
          id: auditModule.uuidHex(id),
          email: String(get(rows[0], "email")),
          rowFingerprint: String(get(rows[0], "rowFingerprint")),
          sessionId: String(get(sessions[0][1], "self")),
        };
      };
      const preparationActor = originalActor(String(get(bootstrap.value, "userId")));
      assert.ok(
        preparationActor.email === get(bootstrap.value, "email") &&
          preparationActor.sessionId === auditModule.uuidHex(get(bootstrap.value, "sessionId")),
      );
      audit.sqlitePreparation = {
        preparationActor,
        readbackActor: originalActor(String(get(server.seed, "freshActorId"))),
        workspace: auditModule.uuidHex(get(server.seed, "workspaceId")),
        document: auditModule.uuidHex(get(server.seed, "document", "id")),
        original,
        prepared: current,
        state: {
          original: list(
            get(originalSeed, "rows", String(get(server.seed, "document", "id"))),
            "stateRow",
          ),
          current: [],
          after: [],
        },
      };
      const seedWorkspace = String(get(server.seed, "workspaceId")),
        seedId = String(get(server.seed, "document", "id"));
      const seedNative = await native(
        "observe",
        { workspaceId: seedWorkspace, documentIds: [seedId] },
        "current-seed",
      );
      const seedRow = get(seedNative, "rows", seedId),
        oldSeed = get(originalSeed, "rows", seedId);
      audit.sqlitePreparation.readback = await sqliteReadbackAppendWitness({
        original: oldSeed,
        current: seedRow,
        before: original.dataset,
        prepared: current.dataset,
        workspaceId: seedWorkspace,
        documentId: seedId,
        actorId: String(get(server.seed, "freshActorId")),
        writes: get(server.readback, "readbackWrites"),
        clocks: [String(original.checks.clockMicros), String(current.checks.clockMicros)],
      });
      preparedSeed = seedRow;
      assert.ok(
        deepEquals(get(seedRow, "stateColumns"), [
          "workspace_id",
          "document_id",
          "state",
          "encoding",
          "compacted_at",
          "created_at",
          "updated_at",
          "writer_generation",
          "snapshot_cutoff_seq",
          "tail_seq",
        ]),
      );
      audit.sqlitePreparation.state.current = list(seedRow, "stateRow");
      audit.observedFences.push(...(list(seedRow, "roomFences") as unknown[][]));
      audit.sqlitePreparation.maintenance = {
        server: auditModule.selectedMaintenanceSnapshot(manifest, restartedOrigin, target),
        leaseMicros: "300000000",
      };
      auditModule.assertSqlitePreparedPreserved(original.dataset, current.dataset, audit);
      assert.ok(storageOriginal !== undefined);
      const storagePrepared = seam.treeHashes(state.storage);
      for (const [path, hash] of Object.entries(storageOriginal))
        assert.ok(
          storagePrepared[path] === hash,
          "original storage bytes changed during preparation",
        );
      privateText(
        join(directory, "prepared-storage.private.json"),
        JSON.stringify(storagePrepared),
      );
      const binding: import("../../../apps/web/e2e-pending/selected-final-contract.ts").FinalBinding =
        {
          schema: 1,
          backend: "sqlite",
          source: state.head,
          tree: state.tree,
          browser: "chromium",
          flow: "on",
          suite: "products",
          namespace,
          workspaceId,
          workspaceSlug: namespace,
          setupNeeded: false,
          schemaCurrent: true,
          commit: "confirmed",
          lifecycleDrain: "confirmed",
          leases: 0,
          owner: { userId: owner.userId, email: owner.email, password: owner.password },
          member: { userId: member.userId, email: member.email, password: member.password },
          outsider: { userId: outsider.userId, email: outsider.email, password: outsider.password },
        };
      const bindingPath = join(directory, "actors.private.json"),
        reportPath = join(directory, "playwright.private.json");
      privateText(bindingPath, JSON.stringify(binding));
      const bindingHash = sha(bindingPath);
      const browserEnv: Record<string, string> = {};
      for (const key of [
        "PATH",
        "LANG",
        "TMPDIR",
        "PLAYWRIGHT_BROWSERS_PATH",
        "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
      ])
        if (state.browserEnv[key] !== undefined) browserEnv[key] = state.browserEnv[key];
      Object.assign(browserEnv, {
        CI: "true",
        PLAYWRIGHT_BASE_URL: server.base,
        FVOCI_E2E_SELECTED_BACKEND: "sqlite",
        FVOCI_E2E_SELECTED_FLOW: "on",
        FVOCI_E2E_SELECTED_SOURCE: state.head,
        FVOCI_E2E_SELECTED_TREE: state.tree,
        FVOCI_E2E_FINAL_BROWSER: "chromium",
        FVOCI_E2E_FINAL_SUITE: "products",
        FVOCI_E2E_TURSO_NAMESPACE: namespace,
        FVOCI_E2E_FINAL_ACTOR_BINDING: bindingPath,
        FVOCI_E2E_FINAL_BROWSER_BIN: state.browserInputs?.chromium.path ?? "",
        FVOCI_E2E_RESULT_DIR: directory,
        PLAYWRIGHT_JSON_OUTPUT_FILE: reportPath,
      });
      const args = browserArgs(
        process.execPath,
        join(root, "node_modules/playwright/cli.js"),
        productSpec,
      );
      args[args.indexOf("--config") + 1] = "e2e-pending/selected-final-playwright.config.ts";
      privateText(join(directory, "browser.private.log"), "");
      const result = await seam.command(args, {
        env: browserEnv,
        cwd: join(root, "apps/web"),
        required: false,
        log: join(directory, "browser.private.log"),
      });
      state.code = result.returncode;
      if (state.code !== 0)
        recordFailure(state, state.code, undefined, join(directory, "browser.private.log"));
      assert.ok(state.code === 0, "SQLite PRODUCT4 browser failed");
      chmodSync(reportPath, 0o600);
      const report = read(reportPath);
      assert.ok(sha(bindingPath) === bindingHash, "PRODUCT4 actor binding changed");
      const titles = validateProductReport(report, binding, bindingHash);
      const cases = auditModule.reportCases(
        report,
        "selected-final-acceptance.ts",
        titles,
        "sqlite",
      );
      productReadbacks = cases.map((item) => {
        const wire = auditModule.attachment([item], "selected-final-writes.json"),
          readback = auditModule.attachment([item], "selected-final-native-readback.json");
        for (const evidence of [wire, readback])
          assert.ok(
            get(evidence, "source") === state.head &&
              get(evidence, "tree") === state.tree &&
              get(evidence, "browser") === "chromium" &&
              get(evidence, "flow") === "on",
          );
        for (const actor of list(wire, "actors")) {
          assert.ok(Object.hasOwn(audit.actors, auditModule.uuidHex(get(actor, "actorId"))));
          writes.push(...(list(actor, "writes") as typeof writes));
        }
        return readback;
      });
      productPassed = true;
      state.receipt.sqlite_products_browser = {
        exit: 0,
        actual_tests: 4,
        skipped: 0,
        retries: 0,
        titles,
        report_sha256: sha(reportPath),
        binding_sha256: bindingHash,
      };
    },
    async beforeRestartStop() {
      // Browser actors have closed; let the live dispatcher finish their
      // committed tail before cancellation deliberately leaves queued work.
      const signal = AbortSignal.timeout(FIXTURE_BUDGET * 1000);
      for (let sample = 0; ; sample++) {
        signal.throwIfAborted();
        const observation = await serial("settlement-" + String(sample), false, signal);
        const checks = observation.checks;
        const head = get(checks, "eventHead");
        const relays = list(checks, "outbox");
        assert.ok(typeof head === "string" && /^(0|[1-9][0-9]*)$/.test(head));
        assert.ok(
          deepEquals(relays.map((relay) => get(relay, "consumer")).sort(), [
            "github",
            "mail",
            "notifications",
            "push",
            "webhooks",
          ]),
          "SQLite relay settlement requires the exact current registry",
        );
        if (
          relays.every((relay) => {
            const seq = get(relay, "lastSeq");
            assert.ok(typeof seq === "string" && /^(0|[1-9][0-9]*)$/.test(seq));
            return BigInt(seq) === BigInt(head);
          })
        ) {
          signal.throwIfAborted();
          return;
        }
      }
    },
    async afterRestartStopped(rows) {
      assert.ok(
        original !== undefined &&
          current !== undefined &&
          restartedOrigin !== undefined &&
          storageOriginal !== undefined,
      );
      const retired = await auditModule.selectedRoomRetirement(
        manifest,
        restartedOrigin,
        target,
        rows,
      );
      const after = await serial("after", true);
      auditModule.retainRoomHistory(after, original, retired);
      auditModule.assertRoomClosure(after, target);
      audit.servers.push(
        auditModule.maintenanceReceipts(
          restartedOrigin.origin.log.path,
          restartedOrigin.origin.identity,
          target,
        ),
      );
      audit.serial = { before: original, after, policy, expectedBackend: "sqlite" };
      assert.ok(
        productPassed && audit.sqlitePreparation !== undefined && originalSeed !== undefined,
        "PRODUCT4 did not complete before normal retirement",
      );
      const finalSeed = await native(
        "observe",
        {
          workspaceId: audit.sqlitePreparation.workspace.replace(
            /^(.{8})(.{4})(.{4})(.{4})(.{12})$/,
            "$1-$2-$3-$4-$5",
          ),
          documentIds: [
            audit.sqlitePreparation.document.replace(
              /^(.{8})(.{4})(.{4})(.{4})(.{12})$/,
              "$1-$2-$3-$4-$5",
            ),
          ],
        },
        "after-seed",
      );
      const finalRow = get(finalSeed, "rows", Object.keys(record(finalSeed, "rows"))[0] as string);
      assert.ok(preparedSeed !== undefined);
      const oldRow = preparedSeed;
      for (const key of [
        "state",
        "cutoff",
        "tail",
        "content",
        "text",
        "version",
        "updates",
        "receipts",
        "revisions",
        "appendRows",
      ])
        assert.ok(
          deepEquals(get(oldRow, key), get(finalRow, key), true),
          "original wiki native bytes/receipts changed after retirement",
        );
      audit.sqlitePreparation.state.after = list(finalRow, "stateRow");
      audit.sqlitePreparation.products = [];
      const documents = new Map<string, Set<string>>();
      for (const row of productReadbacks) {
        const workspace = String(get(row, "workspaceId")),
          document = String(get(row, "targetId"));
        auditModule.uuidHex(workspace);
        auditModule.uuidHex(document);
        if (!documents.has(workspace)) documents.set(workspace, new Set());
        documents.get(workspace)?.add(document);
      }
      for (const write of writes) {
        if (!documents.has(write.workspaceId)) documents.set(write.workspaceId, new Set());
        documents.get(write.workspaceId)?.add(write.documentId);
      }
      for (const [workspaceId, ids] of documents) {
        const observed = await native(
          "observe",
          { workspaceId, documentIds: [...ids] },
          "after-products-" + token(4),
        );
        for (const documentId of ids) {
          const row = get(observed, "rows", documentId);
          if (audit.documents !== undefined)
            audit.documents[auditModule.uuidHex(documentId)] = auditModule.uuidHex(workspaceId);
          audit.observedFences.push(...(list(row, "roomFences") as unknown[][]));
          const receipts = list(row, "receipts") as SelectedNativeReceipt[];
          assertSelectedSuccessfulWrites(receipts, writes, workspaceId, documentId);
          assert.ok(
            !Object.values(record(original.dataset, "allocations", "documents")).some(
              (refs) => get(refs, "self") === auditModule.uuidHex(documentId),
            ),
            "PRODUCT4 target was preexisting",
          );
          const publications = sqliteAppendPublicationWitness({
            original: { appendRows: { events: [], audit_log: [] } },
            current: row,
            before: original.dataset,
            prepared: after.dataset,
            workspaceId,
            documentId,
            receipts,
            clocks: [String(current.checks.clockMicros), String(after.checks.clockMicros)],
          });
          audit.sqlitePreparation.products.push({
            ...publications,
            workspace: auditModule.uuidHex(workspaceId),
            document: auditModule.uuidHex(documentId),
          });
          targets.push({
            workspaceId,
            documentId,
            receipts: list(row, "receipts") as (typeof targets)[number]["receipts"],
          });
          for (const readback of productReadbacks.filter(
            (row) => get(row, "workspaceId") === workspaceId && get(row, "targetId") === documentId,
          ))
            assert.ok(
              deepEquals(get(row, "content"), get(readback, "content"), true) &&
                get(row, "text") === get(readback, "text"),
            );
        }
      }

      auditModule.auditSelectedFinalNative(original.dataset, after.dataset, audit, targets, writes);
      const storageAfter = seam.treeHashes(state.storage);
      for (const [path, hash] of Object.entries(storageOriginal))
        assert.ok(storageAfter[path] === hash, "original storage bytes changed");
      privateText(join(directory, "audit.private.json"), JSON.stringify(audit));
      state.receipt.sqlite_products_native = {
        observation: "PASS",
        preservation: "PASS",
        wire_receipts: "PASS",
        normal_room_retirement: "PASS",
        audit_sha256: sha(join(directory, "audit.private.json")),
      };
    },
  });
}

export const sqliteSeam: Seam = {
  products: productCallbacks,
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
