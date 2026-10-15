// The six real browser cases share the existing sealed native build, normal
// installation lifecycle, process scope and source preservation verifier.
import { deepEquals } from "bun";
import { closeSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import * as Y from "yjs";
import { root as checkout, digest } from "../selected-backend-ci/io.ts";
import {
  attachment,
  nativeReceiptsRetained,
  rememberActor,
  reportCases,
  RESTORE,
  uuidHex,
  roomRetirement,
  type Audit,
} from "./ui-audit.ts";
import {
  cleanEnv,
  failureCode,
  get,
  list,
  privateRead,
  record,
  require,
  token,
  valueDigest,
  type Record_,
} from "./ui-common.ts";
import { admitSerialBaseline, auditFinalPhase, defaultSteps, type FlowSteps } from "./ui-flow.ts";
import type { Started } from "./ui-native.ts";
import type { Scope } from "./ui-processes.ts";
import type { Manifest } from "./ui-record.ts";

export const RESTORE_CONFIG = "turso-restore-playwright.config.ts";

/** Compare the full CRDT, including deleted structs, rather than its visible
 * body or state vector. The archive and primary observer both expose V1 data. */
export function restoreStateDigest(row: Record_, extra: Uint8Array[] = []): string {
  const doc = new Y.Doc({ gc: false });
  const bytes = (value: unknown) => {
    require(typeof value === "string" &&
      /^(?:[a-f0-9]{2})+$/.test(value), "RESTORE_STAGE_STATE_INVALID");
    return Buffer.from(value, "hex");
  };
  try {
    Y.applyUpdate(doc, bytes(get(row, "state")));
    for (const update of list(row, "updates")) Y.applyUpdate(doc, bytes(get(update, "hex")));
    for (const update of extra) Y.applyUpdate(doc, update);
    return digest(Y.encodeStateAsUpdate(doc));
  } finally {
    doc.destroy();
  }
}

/** A fresh client's independently observed save barrier permits precisely its
 * acknowledged no-effect sync writes, never an arbitrary later native tail. */
export function verifyRestoreStage(
  before: Record_,
  after: Record_,
  witness: Record_,
  document: string,
  actor: string,
): void {
  require(get(witness, "document") === document &&
    get(witness, "actor") === actor, "RESTORE_STAGE_IDENTITY_MISMATCH");
  require(get(witness, "ack", "matched") === true &&
    /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(
      String(get(witness, "ack", "requestId")),
    ), "RESTORE_STAGE_ACK_MISSING");
  const updates = list(witness, "sync");
  require(updates.length > 0 &&
    updates.every(
      (update) =>
        get(update, "applied") === true &&
        get(update, "fullStateUnchanged") === true &&
        typeof get(update, "empty") === "boolean" &&
        /^[a-f0-9]{64}$/.test(String(get(update, "sha"))) &&
        typeof get(update, "len") === "number" &&
        Number.isSafeInteger(get(update, "len")) &&
        Number(get(update, "len")) > 0 &&
        (get(update, "empty") === false ||
          (get(update, "len") === 2 && get(update, "sha") === digest(new Uint8Array([0, 0])))),
    ), "RESTORE_STAGE_SYNC_UNCONFIRMED");
  const writes = updates.filter((update) => get(update, "empty") === false);
  require(typeof before.tail === "number" &&
    Number.isSafeInteger(before.tail) &&
    before.tail >= 0 &&
    typeof after.tail === "number" &&
    Number.isSafeInteger(after.tail) &&
    String(before.tail) === get(witness, "beforeTail") &&
    String(after.tail) === get(witness, "finalTail") &&
    BigInt(after.tail) ===
      BigInt(before.tail) + BigInt(writes.length), "RESTORE_NATIVE_TAIL_MISMATCH");
  require(restoreStateDigest(before) === get(witness, "fullStateSha256") &&
    restoreStateDigest(after) === get(witness, "fullStateSha256") &&
    deepEquals(before.content, after.content, true), "RESTORE_STAGE_STATE_CHANGED");
  const prior = list(before, "receipts"),
    current = list(after, "receipts");
  require(nativeReceiptsRetained(prior, current) &&
    current.length === prior.length + writes.length, "RESTORE_STAGE_RECEIPTS_CHANGED");
  for (const [index, update] of writes.entries()) {
    const seq = before.tail + index + 1;
    const matches = current.filter((receipt) => get(receipt, "seq") === seq);
    require(matches.length === 1 &&
      get(matches[0], "actor") === actor &&
      get(matches[0], "sha") === get(update, "sha") &&
      get(matches[0], "len") === get(update, "len") &&
      /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(
        String(get(matches[0], "op")),
      ), "RESTORE_STAGE_UPDATE_MISMATCH");
  }
}
export const RESTORE_TITLES = Object.freeze([
  "Turso restore U6: UI confirm has one forward revision and matching save ACK",
  "Turso restore source restart: fresh client retains U6 body history and tail",
  "Turso restore export: current native archive carries IDs references attachments and precision",
  "Turso restore destination: malformed input publishes nothing and identical replay has one job",
  "Turso restore destination restart: fresh client reads exact archive data and permissions",
  "Turso restore archive comparison: re-export preserves native bytes with explicit actor workspace remap",
] as const);

export function requireRestoreRegistration(workspace = checkout): void {
  const source = readFileSync(join(workspace, "apps/web/e2e-pending", RESTORE), "utf8");
  const titles = [...source.matchAll(/^test\("([^"\n]+)"/gm)].map((match) => match[1]);
  require(deepEquals(titles, RESTORE_TITLES), "RESTORE_REGISTRATION_CHANGED");
}

export interface RestoreReceipt {
  source: string;
  tree: string;
  sourceBackend: "libsql-remote";
  destinationBackend: "sqlite";
  counts: number[];
  result: "PASS" | "FAIL";
  originalFailure: string | null;
  cleanupErrors: string[];
  auditErrors: string[];
  preservation: Record_;
  sourceUnchangedDuringDestination: "PASS" | "FAIL" | "NOTRUN";
  destinationIdentity: Record_ | null;
}

export function sourceEnvironment(
  manifest: Manifest,
  environment: Record<string, string>,
  storage: string,
  namespace: string,
): Record<string, string> {
  const result = {
    ...cleanEnv(),
    ...environment,
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_E2E_TURSO_NAMESPACE: namespace,
    FVOCI_REALTIME_MODE: "on",
    FVOCI_BIND: "127.0.0.1:0",
    FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0",
    FVOCI_COOKIE_SECURE: "0",
    STORAGE_DRIVER: "local",
    FVOCI_STORAGE_DIR: storage,
    FVOCI_STATIC_DIR: join(checkout, "apps/web/dist"),
    FVOCI_COLLAB_ENGINE: get(manifest, "binaries", "collab-engine", "path") as string,
    FVOCI_COLLAB_FAMILY_LEASE_MS: "30000",
    FVOCI_COLLAB_FAMILY_RENEW_MS: "5000",
    FVOCI_COLLAB_MAX_ROOMS: "2",
    RUST_LOG: "info",
    FVOCI_MAINTENANCE_TICK_SECS: "86400",
    FVOCI_MAINTENANCE_INTERVAL_SECS: "86400",
    FVOCI_UPLOAD_GC_INTERVAL_SECS: "86400",
    FVOCI_REVISION_SWEEP_INTERVAL_SECS: "86400",
  };
  return result;
}

export function destinationEnvironment(
  manifest: Manifest,
  storage: string,
): Record<string, string> {
  return {
    ...cleanEnv(),
    FVOCI_DATABASE_BACKEND: "sqlite",
    FVOCI_SQLITE_PATH: join(storage, "app.sqlite"),
    PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: token(32) }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: JSON.stringify({ fixture: token(32) }),
    ENCRYPTION_ACTIVE_KEY_ID: "fixture",
    FVOCI_REALTIME_MODE: "on",
    FVOCI_BIND: "127.0.0.1:0",
    FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0",
    FVOCI_COOKIE_SECURE: "0",
    STORAGE_DRIVER: "local",
    FVOCI_STORAGE_DIR: storage,
    FVOCI_STATIC_DIR: join(checkout, "apps/web/dist"),
    FVOCI_COLLAB_ENGINE: get(manifest, "binaries", "collab-engine", "path") as string,
    FVOCI_COLLAB_FAMILY_LEASE_MS: "30000",
    FVOCI_COLLAB_FAMILY_RENEW_MS: "5000",
    FVOCI_COLLAB_MAX_ROOMS: "2",
    RUST_LOG: "info",
  };
}

/** An outer local fixture may own the single still-live sqld; it retires that
 * allocation after the final read-only primary audit, before Scope.close. */
export async function executeRestore(
  scope: Scope,
  manifest: Manifest,
  baseline: Record_,
  environment: Record<string, string>,
  steps: FlowSteps = defaultSteps,
  local?: { heavy: string; sourceAllocation: number },
): Promise<RestoreReceipt> {
  requireRestoreRegistration();
  admitSerialBaseline(baseline, steps.serial);
  const namespace = "tui-" + steps.token(10);
  const directory = join(steps.root(), "restore");
  mkdirSync(directory, 0o700);
  const sourceStorage = join(directory, "source-storage"),
    destinationStorage = join(directory, "destination-storage");
  mkdirSync(sourceStorage, 0o700);
  mkdirSync(destinationStorage, 0o700);
  const sourceEnv = sourceEnvironment(manifest, environment, sourceStorage, namespace);
  const destinationEnv = destinationEnvironment(manifest, destinationStorage);
  const input = join(directory, "checkpoint.private.json");
  const audit: Audit = {
    namespaces: [namespace],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: digest(sourceEnv.FVOCI_LIBSQL_URL as string),
    serverStarts: 0,
    observedFences: [],
  };
  const receipt: RestoreReceipt = {
    source: manifest.sourceInputs.source,
    tree: manifest.sourceInputs.tree,
    sourceBackend: "libsql-remote",
    destinationBackend: "sqlite",
    counts: [0, 0, 0, 0, 0, 0],
    result: "FAIL",
    originalFailure: null,
    cleanupErrors: [],
    auditErrors: [],
    preservation: { result: "NOTRUN" },
    sourceUnchangedDuringDestination: "NOTRUN",
    destinationIdentity: null,
  };
  const running: {
    started: Started;
    directory: string;
    source: boolean;
    stopped: boolean;
    identity: { pid: number; startTicks: string };
  }[] = [];
  const begin = async (
    name: string,
    env: Record<string, string>,
    source: boolean,
    setup = false,
  ) => {
    const path = join(directory, name);
    mkdirSync(path, 0o700);
    const started = await steps.start(scope, manifest, env, path, setup);
    const allocation = {
      started,
      directory: path,
      source,
      stopped: false,
      identity: steps.serverIdentity(started.server),
    };
    running.push(allocation);
    if (source) audit.serverStarts += 1;
    return allocation;
  };
  const stop = async (allocation: (typeof running)[number]) => {
    if (allocation.stopped) return;
    await steps.stop(
      scope,
      allocation.started.server,
      allocation.started.base,
      allocation.directory,
    );
    allocation.stopped = true;
    closeSync(allocation.started.log);
  };
  const run = async (
    index: number,
    allocation: (typeof running)[number],
    backend: "libsql-remote" | "sqlite",
    setup = false,
  ) => {
    steps.currentBuild();
    const path = join(allocation.directory, "case-" + String(index));
    mkdirSync(path, 0o700);
    const title = RESTORE_TITLES[index];
    require(title !== undefined, "RESTORE_CASE_REFUSED");
    const env = {
      PLAYWRIGHT_BASE_URL: allocation.started.base,
      FVOCI_E2E_SELECTED_BACKEND: backend,
      FVOCI_E2E_SELECTED_FLOW: "on",
      FVOCI_E2E_TURSO_NAMESPACE: namespace,
      FVOCI_E2E_RESTORE_INPUT: input,
      FVOCI_E2E_RESTORE_SETUP: setup ? "1" : "0",
      ...(backend === "sqlite"
        ? {
            FVOCI_E2E_RESTORE_SQLITE_PATH: destinationEnv.FVOCI_SQLITE_PATH as string,
            FVOCI_E2E_RESTORE_SQLITE_ROOT: destinationStorage,
          }
        : {}),
    };
    const report = await steps.browser(
      scope,
      manifest,
      path,
      env,
      RESTORE,
      title,
      checkout,
      RESTORE_CONFIG,
      local?.heavy,
    );
    const cases = reportCases(report, RESTORE, [title], backend);
    receipt.counts[index] = cases.length;
    return cases;
  };
  const observe = async () => {
    const state = privateRead(input, 64 * 1024 * 1024);
    const document = get(state, "source", "document", "id");
    const workspaceId = get(state, "source", "workspace", "id");
    const observed = await steps.fixture(scope, manifest, "observe", sourceEnv, {
      workspaceId,
      documentIds: [document],
    });
    const row = record(observed, "rows", document as string);
    audit.observedFences.push(...(list(row, "roomFences") as unknown[][]));
    return row;
  };
  let sourceAfter: Record_ | null = null;
  try {
    const setup = get(baseline, "setupNeeded") === true;
    const owner = setup ? null : await steps.fixture(scope, manifest, "owner", sourceEnv);
    if (owner) rememberActor(audit, owner, namespace);
    const source = await begin("source", sourceEnv, true, setup);
    const sourceCases = await run(0, source, "libsql-remote", setup);
    const state = privateRead(input);
    const actor = uuidHex(get(state, "source", "actor")),
      team = uuidHex(get(state, "source", "team", "id"));
    require(get(state, "source", "team", "slug") === namespace, "RESTORE_SOURCE_ACTOR_REFUSED");
    if (owner)
      require(actor === uuidHex(get(owner, "userId")) &&
        team === uuidHex(get(owner, "workspaceId")), "RESTORE_SOURCE_ACTOR_REFUSED");
    else {
      audit.actors[actor] = namespace + "-owner@example.invalid";
      audit.workspaces[team] = namespace;
    }
    const first = await observe();
    const firstStage = attachment(sourceCases, "restore-stage.private.json");
    require(String(get(firstStage, "before", "tail")) ===
      get(state, "source", "restoredTail"), "RESTORE_NATIVE_TAIL_MISMATCH");
    verifyRestoreStage(
      record(firstStage, "before"),
      first,
      record(firstStage, "witness"),
      String(get(state, "source", "document", "id")),
      String(get(state, "source", "actor")),
    );
    require(list(first, "receipts").length >= 2 &&
      deepEquals(
        first.content,
        get(state, "source", "restoredBody"),
        true,
      ), "RESTORE_NATIVE_ACK_MISMATCH");
    const restored = list(first, "revisions").filter((row) => get(row, "reason") === "restore");
    require(restored.length === 1 &&
      get(restored[0], "id") ===
        get(state, "source", "restoredRevision"), "RESTORE_NATIVE_REVISION_MISMATCH");
    await stop(source);
    const restarted = await begin("source-restart", sourceEnv, true);
    const restartCases = await run(1, restarted, "libsql-remote");
    const fresh = await observe();
    const restartStage = attachment(restartCases, "restore-stage.private.json");
    require(get(restartStage, "before", "tail") === first.tail &&
      restoreStateDigest(record(restartStage, "before")) === restoreStateDigest(first) &&
      deepEquals(
        get(restartStage, "before", "receipts"),
        first.receipts,
        true,
      ), "RESTORE_SOURCE_RESTART_MISMATCH");
    verifyRestoreStage(
      record(restartStage, "before"),
      fresh,
      record(restartStage, "witness"),
      String(get(state, "source", "document", "id")),
      String(get(state, "source", "actor")),
    );
    require(deepEquals(first.content, fresh.content, true) &&
      nativeReceiptsRetained(
        list(first, "receipts"),
        list(fresh, "receipts"),
      ), "RESTORE_SOURCE_RESTART_MISMATCH");
    await run(2, restarted, "libsql-remote");
    await observe();
    await stop(restarted);
    sourceAfter = await steps.fixture(scope, manifest, "baseline", sourceEnv);
    steps.write(join(directory, "source-before-destination.private.json"), sourceAfter);
    const destination = await begin("destination", destinationEnv, false, true);
    const target = digest(destinationEnv.FVOCI_SQLITE_PATH as string);
    require(target !== audit.targetSha256 &&
      sourceStorage !== destinationStorage, "RESTORE_SEPARATE_INSTALL_REQUIRED");
    receipt.destinationIdentity = {
      backend: "sqlite",
      targetSha256: target,
      sourceTargetSha256: audit.targetSha256,
    };
    const destinationCases = await run(3, destination, "sqlite");
    const counts = attachment(destinationCases, "restore-destination-counts.private.json");
    require(deepEquals(get(counts, "before", "identity"), get(counts, "after", "identity"), true) &&
      get(counts, "after", "counts", "nativeJobs") === 1, "RESTORE_DESTINATION_IDENTITY_REFUSED");
    receipt.destinationIdentity.fileIdentity = get(counts, "after", "identity");
    receipt.destinationIdentity.supplementalSqlite = get(counts, "after", "sqlite");
    await stop(destination);
    const destinationRestart = await begin("destination-restart", destinationEnv, false);
    await run(4, destinationRestart, "sqlite");
    await run(5, destinationRestart, "sqlite");
    await stop(destinationRestart);
    require(deepEquals(receipt.counts, [1, 1, 1, 1, 1, 1]), "RESTORE_ACTUAL_COUNTS_FAILED");
    receipt.result = "PASS";
  } catch (error) {
    receipt.originalFailure = failureCode(error);
  }
  for (const allocation of [...running].reverse()) {
    try {
      await stop(allocation);
    } catch (error) {
      receipt.cleanupErrors.push(failureCode(error));
      try {
        await scope.finish(allocation.started.server.child, true);
      } catch {
        receipt.cleanupErrors.push("RESTORE_SERVER_FORCE_RETIRE_FAILED");
      }
      try {
        closeSync(allocation.started.log);
      } catch {
        receipt.cleanupErrors.push("RESTORE_SERVER_LOG_CLOSE_FAILED");
      }
    }
    if (allocation.source) {
      try {
        if (steps.serial)
          (audit.roomRetirements ??= []).push(
            roomRetirement(
              scope,
              join(allocation.directory, "server.private.log"),
              allocation.identity,
              audit.targetSha256,
              allocation.started.server.hostIdentity,
            ),
          );
        audit.servers.push(
          steps.maintenanceReceipts(
            join(allocation.directory, "server.private.log"),
            allocation.identity,
            audit.targetSha256,
          ),
        );
      } catch (error) {
        receipt.auditErrors.push(failureCode(error));
      }
    }
  }
  // Maintenance generations must remain in actual launch order.
  audit.servers.reverse();
  try {
    steps.currentBuild();
    require(scope.allocations.every(
      (allocation, index) => allocation.closed || index === local?.sourceAllocation,
    ), "UI_RESOURCE_CLOSURE_FAILED");
    const final = await steps.fixture(scope, manifest, "baseline", sourceEnv);
    steps.write(join(directory, "preservation.private.json"), {
      before: baseline,
      after: final,
      audit,
    });
    receipt.preservation = { result: "observed", afterSha256: valueDigest(final) };
    require(receipt.auditErrors.length === 0, "UI_OWNERSHIP_AUDIT_FAILED");
    await auditFinalPhase(scope, manifest, baseline, final, audit, sourceEnv, steps);
    receipt.preservation.result = "PASS";
    if (sourceAfter) {
      require(deepEquals(sourceAfter, final, true), "RESTORE_SOURCE_CHANGED_BY_DESTINATION");
      receipt.sourceUnchangedDuringDestination = "PASS";
    }
  } catch (error) {
    receipt.preservation.result = receipt.preservation.afterSha256 ? "FAIL" : "NOTRUN";
    receipt.originalFailure ??= failureCode(error);
    receipt.sourceUnchangedDuringDestination = sourceAfter ? "FAIL" : "NOTRUN";
  }
  if (receipt.cleanupErrors.length || receipt.auditErrors.length || receipt.originalFailure)
    receipt.result = "FAIL";
  try {
    steps.write(join(directory, "restore-result.private.json"), receipt);
  } catch {
    receipt.originalFailure ??= "UI_RECEIPT_WRITE_FAILED";
  }
  require(receipt.result === "PASS" && receipt.originalFailure === null, receipt.originalFailure ??
    "UI_RESOURCE_CLOSURE_FAILED");
  return receipt;
}
