// The guarded ON, restart and OFF consumer against the current primary, its
// preservation audit, and the member actor the browser fixture calls back.
import { deepEquals } from "bun";
import {
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { ownedListener, startLocalLibsql } from "../keycloak/local-libsql.ts";
import { localTursoUiRun } from "../selected-backend-ci/admission.ts";
import { root as checkout, digest, listenerAbsent, sha, write } from "../selected-backend-ci/io.ts";
import {
  assertPreserved,
  attachment,
  maintenanceReceipts,
  nativeReceiptsRetained,
  OFF,
  ON,
  rememberActor,
  reportCases,
  startupBlockers,
  uuidHex,
  type Audit,
} from "./ui-audit.ts";
import {
  AUTH_SPEC,
  authSessionActor,
  authSessionEventEffects,
  authSessionWitnesses,
} from "./ui-auth-session.ts";
import {
  cleanEnv,
  diagnosticJson,
  executionMode,
  failureCode,
  get,
  isRecord,
  list,
  privateRead,
  record,
  require,
  root,
  stdio,
  token,
  valueDigest,
  type Output,
  type Record_,
  bunIsolation,
  UiError,
} from "./ui-common.ts";
import { lease, shellQuote, type ServerHandle } from "./ui-container.ts";
import { browser, fixture, registeredTitles, start, stop } from "./ui-native.ts";
import {
  identity,
  now,
  pause,
  poll,
  wait,
  withProcesses,
  type Child,
  type Scope,
} from "./ui-processes.ts";
import { currentBuild, loadLocalLease, type Manifest } from "./ui-record.ts";
import type { executeRestore } from "./ui-restore.ts";

export const ENTRY = join(import.meta.dir, "ui.ts");
const RESTART_TITLE =
  "selected normal main restart: fresh actor reads persisted native history and manual revision";

/** The steps the consumer composes; tests replace individual steps. */
export interface FlowSteps {
  fixture: typeof fixture;
  start: typeof start;
  stop: typeof stop;
  browser: typeof browser;
  serverIdentity: (server: ServerHandle) => { pid: number; startTicks: string };
  maintenanceReceipts: typeof maintenanceReceipts;
  currentBuild: () => Manifest;
  write: typeof write;
  root: () => string;
  token: (bytes: number) => string;
  output: Output;
}
export const allocationIdentity = (server: ServerHandle) =>
  server.maintenanceIdentity ?? identity(server.child.pid);
export const defaultSteps: FlowSteps = {
  fixture,
  start,
  stop,
  browser,
  serverIdentity: allocationIdentity,
  maintenanceReceipts,
  currentBuild: () => currentBuild(lease.load),
  write,
  root,
  token,
  output: stdio,
};

/**
 * The executable the browser fixture runs to create the member actor. The
 * fixture passes only an allowlisted environment without RUNNER_TEMP, and the
 * actor's process scope writes its closure receipt under RUNNER_TEMP, so the
 * wrapper carries this consumer's own value. Bun loads no .env file from the
 * fixture's working directory (the original read none either).
 */
export function memberWrapper(
  runnerTemp: string = get(process.env, "RUNNER_TEMP") as string,
): string {
  return (
    "#!/bin/sh\nRUNNER_TEMP=" +
    shellQuote(runnerTemp) +
    "\nexport RUNNER_TEMP\nexec " +
    shellQuote(process.execPath) +
    " " +
    bunIsolation().map(shellQuote).join(" ") +
    " " +
    shellQuote(ENTRY) +
    " --actor\n"
  );
}

/** The normal server environment of one namespace allocation. */
export function allocationEnvironment(
  manifest: Manifest,
  environment: Record<string, string>,
  namespace: string,
  flow: "on" | "off",
  storage: string,
): Record<string, string> {
  const env: Record<string, string> = {
    ...cleanEnv(),
    ...environment,
    FVOCI_E2E_TURSO_NAMESPACE: namespace,
    FVOCI_REALTIME_MODE: flow,
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
  // Existing scheduler configuration: retain the immediate startup sweep
  // and account for each generation; avoid a second cadence within this
  // bounded UI allocation. No consumer is disabled.
  for (const name of [
    "FVOCI_MAINTENANCE_TICK_SECS",
    "FVOCI_MAINTENANCE_INTERVAL_SECS",
    "FVOCI_UPLOAD_GC_INTERVAL_SECS",
    "FVOCI_REVISION_SWEEP_INTERVAL_SECS",
  ])
    env[name] = "86400";
  return env;
}

interface Allocation {
  audit: Audit;
  namespace: string;
  directory: string;
  setupNeeded: boolean;
}

/** The namespace owner, the actor binding and the member fixture the browser calls. */
async function allocateActors(
  scope: Scope,
  manifest: Manifest,
  baseline: Record_,
  env: Record<string, string>,
  { audit, namespace, directory, setupNeeded }: Allocation,
  steps: FlowSteps,
): Promise<{ owner: Record_ | null; bindingPath: string; capsule: string; wrapper: string }> {
  const source = manifest.sourceInputs;
  const owner = setupNeeded ? null : await steps.fixture(scope, manifest, "owner", env);
  if (owner !== null) {
    require(get(owner, "commit") === "confirmed" &&
      Boolean(get(owner, "freshPrimaryReadback")), "UI_OWNER_READBACK_FAILED");
    rememberActor(audit, owner, namespace);
  }
  const bindingPath = join(directory, "actor-binding.json");
  steps.write(bindingPath, {
    schema: 1,
    backend: "libsql-remote",
    setupNeeded,
    namespace,
    ownerEmail: namespace + "-owner@example.invalid",
    memberEmail: namespace + "-member@example.invalid",
    workspaceSlug: namespace,
    source: source.source,
    tree: source.tree,
    schemaCurrent: true,
    commit: setupNeeded ? "not-attempted" : "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
    baselineSha256: valueDigest(baseline),
    owner,
  });
  const capsule = join(directory, "actor-input.private.json");
  const actorEnvironment = { ...env };
  delete actorEnvironment.ENCRYPTION_KEYS;
  delete actorEnvironment.ENCRYPTION_ACTIVE_KEY_ID;
  // The member fixture never relays mail.
  for (const key of ["SMTP_HOST", "SMTP_PORT", "SMTP_FROM"])
    Reflect.deleteProperty(actorEnvironment, key);
  steps.write(capsule, {
    manifest,
    environment: actorEnvironment,
    namespace,
    workspaceId: owner !== null ? get(owner, "workspaceId") : null,
  });
  const wrapper = join(directory, "member-fixture");
  const fd = openSync(wrapper, "wx", 0o700);
  try {
    fchmodSync(fd, 0o700);
    writeFileSync(fd, memberWrapper());
  } finally {
    closeSync(fd);
  }
  return { owner, bindingPath, capsule, wrapper };
}

export interface Receipt {
  source: string;
  tree: string;
  backend: "libsql-remote";
  counts: { on: number; restart: number; off: number };
  restore: "NOTRUN";
  precisionWorkload: "NOTRUN";
  matchedOnOffCost: "NOTRUN";
  cleanupErrors: string[];
  originalFailure: string | null;
  preservation: Record_;
  receiptWrite: string;
  uiResult?: "PASS" | "FAIL";
  auditErrors?: string[];
}

export async function executeUi(
  scope: Scope,
  manifest: Manifest,
  baseline: Record_,
  environment: Record<string, string>,
  steps: FlowSteps = defaultSteps,
): Promise<Receipt> {
  const source = manifest.sourceInputs;
  require(startupBlockers(baseline).length === 0, "UI_EXISTING_BACKGROUND_WORK_REFUSED");
  const audit: Audit = {
    namespaces: [],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: digest(get(environment, "FVOCI_LIBSQL_URL") as string),
    serverStarts: 0,
    observedFences: [],
  };
  const receipt: Receipt = {
    source: source.source,
    tree: source.tree,
    backend: "libsql-remote",
    counts: { on: 0, restart: 0, off: 0 },
    restore: "NOTRUN",
    precisionWorkload: "NOTRUN",
    matchedOnOffCost: "NOTRUN",
    cleanupErrors: [],
    originalFailure: null,
    preservation: { result: "NOTRUN" },
    receiptWrite: "not-attempted",
  };
  let lastEnvironment = environment;
  const serverAllocations: [ServerHandle, string, { pid: number; startTicks: string }][] = [];
  let failure: unknown = null;
  try {
    for (const flow of ["on", "off"] as const) {
      const namespace = "tui-" + steps.token(10);
      audit.namespaces.push(namespace);
      const directory = join(steps.root(), flow);
      mkdirSync(directory, 0o700);
      const storage = join(directory, "storage");
      mkdirSync(storage, 0o700);
      const env = allocationEnvironment(manifest, environment, namespace, flow, storage);
      lastEnvironment = env;
      const setupNeeded = flow === "on" && Boolean(get(baseline, "setupNeeded"));
      const { owner, bindingPath, capsule, wrapper } = await allocateActors(
        scope,
        manifest,
        baseline,
        env,
        { audit, namespace, directory, setupNeeded },
        steps,
      );
      let server: ServerHandle | null = null,
        base: string | null = null,
        log: number | null = null;
      try {
        ({ server, base, log } = await steps.start(scope, manifest, env, directory, setupNeeded));
        audit.serverStarts += 1;
        serverAllocations.push([server, directory, steps.serverIdentity(server)]);
        const browserEnv: Record<string, string> = {
          PLAYWRIGHT_BASE_URL: base,
          FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
          FVOCI_E2E_SELECTED_FLOW: flow,
          FVOCI_E2E_TURSO_NAMESPACE: namespace,
          FVOCI_E2E_TURSO_SOURCE: source.source,
          FVOCI_E2E_TURSO_TREE: source.tree,
          FVOCI_E2E_SELECTED_FIXTURE_BIN: wrapper,
          FVOCI_E2E_TURSO_PRIVATE_INPUT: capsule,
          FVOCI_E2E_TURSO_ACTOR_BINDING: bindingPath,
        };
        steps.write(join(directory, "binding.json"), {
          schema: 1,
          ready: true,
          flow,
          source: source.source,
          tree: source.tree,
          compiledSource: source.source,
          buildSha256: sha(join(steps.root(), "current-build.json")),
          baselineSha256: valueDigest(baseline),
        });
        steps.write(join(directory, "normal-main-ready.json"), {
          baseURL: base,
          selected_flow: flow,
          source: source.source,
          tree: source.tree,
          compiled_source: source.source,
          current_binding: join(directory, "binding.json"),
          current_binding_sha256: sha(join(directory, "binding.json")),
        });
        const spec = flow === "on" ? ON : OFF;
        const titles = registeredTitles(spec, flow);
        const report = await steps.browser(
          scope,
          manifest,
          directory,
          browserEnv,
          spec,
          flow === "on" ? "selected normal main:" : undefined,
        );
        const cases = reportCases(report, spec, titles);
        receipt.counts[flow] = cases.length;
        if (flow === "on") {
          const seed = attachment(cases, "selected-vue-native-readback.json");
          if (owner === null) {
            audit.actors[uuidHex(get(seed, "creatorId"))] = namespace + "-owner@example.invalid";
            audit.workspaces[uuidHex(get(seed, "workspaceId"))] = namespace;
          }
          require(get(seed, "selected") === "libsql-remote" &&
            (owner === null || get(seed, "workspaceId") === get(owner, "workspaceId")) &&
            get(seed, "firstAck") !== get(seed, "finalAck"), "UI_NATIVE_SEED_MISMATCH");
          const documentId = get(seed, "document", "id") as string;
          (audit.documents as Record<string, string>)[uuidHex(documentId)] = uuidHex(
            get(seed, "workspaceId"),
          );
          const query = { workspaceId: get(seed, "workspaceId"), documentIds: [documentId] };
          const observed = await steps.fixture(scope, manifest, "observe", env, query);
          const row = record(observed, "rows", documentId);
          audit.observedFences.push(...(list(row, "roomFences") as unknown[][]));
          // The browser matches each persist barrier request to its ACK. The
          // server generates separate append operation IDs (room.rs); the
          // primary observer checks their payload lengths, digests and tail.
          // Both distinct edits must have durable operations, and every whole
          // receipt observed here must survive the restart below.
          const committedReceipts = list(row, "receipts");
          require(committedReceipts.length >= 2, "UI_DURABLE_ACK_RECEIPT_MISSING");
          require(deepEquals(get(row, "content"), get(seed, "persisted", "contentJson"), true) &&
            deepEquals(
              get(row, "text"),
              get(seed, "persistedText"),
              true,
            ), "UI_NATIVE_CURRENT_BODY_MISMATCH");
          steps.write(join(directory, "native-before-restart.private.json"), observed);
          const old = await steps.stop(scope, server, base, directory);
          server = null;
          closeSync(log);
          log = null;
          const checkpoint = join(directory, "restart-checkpoint.private.json");
          steps.write(checkpoint, {
            schema: 1,
            source: source.source,
            tree: source.tree,
            compiledSource: source.source,
            selected: "libsql-remote",
            stopped: old,
            seed,
          });
          const restartDir = join(directory, "restart");
          mkdirSync(restartDir, 0o700);
          ({ server, base, log } = await steps.start(scope, manifest, env, restartDir));
          audit.serverStarts += 1;
          serverAllocations.push([server, restartDir, steps.serverIdentity(server)]);
          const restartEnv: Record<string, string> = { ...browserEnv };
          delete restartEnv.FVOCI_E2E_SELECTED_FIXTURE_BIN;
          delete restartEnv.FVOCI_E2E_TURSO_PRIVATE_INPUT;
          Object.assign(restartEnv, {
            PLAYWRIGHT_BASE_URL: base,
            FVOCI_E2E_SELECTED_RESTART_SOURCE: source.source,
            FVOCI_E2E_SELECTED_RESTART_CHECKPOINT: checkpoint,
          });
          reportCases(
            await steps.browser(
              scope,
              manifest,
              restartDir,
              restartEnv,
              ON,
              "selected normal main restart:",
            ),
            ON,
            [RESTART_TITLE],
          );
          receipt.counts.restart = 1;
          const after = await steps.fixture(scope, manifest, "observe", env, query);
          // Restart may append session revisions/generation. The exact original
          // current body and all acknowledged ops must survive.
          const fresh = record(after, "rows", documentId);
          audit.observedFences.push(...(list(fresh, "roomFences") as unknown[][]));
          require(deepEquals(get(fresh, "content"), get(row, "content"), true) &&
            deepEquals(get(fresh, "text"), get(row, "text"), true) &&
            nativeReceiptsRetained(
              committedReceipts,
              list(fresh, "receipts"),
            ), "UI_FRESH_PRIMARY_RESTART_MISMATCH");
          steps.write(join(restartDir, "native-after-restart.private.json"), after);
        }
      } catch (error) {
        receipt.originalFailure ??= failureCode(error);
        throw error;
      } finally {
        if (server !== null) {
          try {
            await steps.stop(scope, server, base as string, directory);
          } catch {
            receipt.cleanupErrors.push("UI_SERVER_CLOSURE_FAILED");
            try {
              await scope.finish(server.child, true);
            } catch {
              receipt.cleanupErrors.push("UI_SERVER_FORCE_RETIRE_FAILED");
            }
          }
        }
        if (log !== null) {
          try {
            closeSync(log);
          } catch {
            receipt.cleanupErrors.push("UI_SERVER_LOG_CLOSE_FAILED");
          }
        }
      }
      require(receipt.cleanupErrors.length === 0, "UI_RESOURCE_CLOSURE_FAILED");
    }
    require(deepEquals(receipt.counts, { on: 1, restart: 1, off: 8 }), "UI_ACTUAL_COUNTS_FAILED");
    receipt.uiResult = "PASS";
  } catch (error) {
    receipt.uiResult = "FAIL";
    receipt.originalFailure ??= failureCode(error);
    failure = error;
  }
  // The primary is observed after every outcome; a failed audit never erases
  // the bounded after-state itself.
  const auditErrors: string[] = [];
  audit.namespaces.forEach((namespace, index) => {
    try {
      const directory = join(steps.root(), index === 0 ? "on" : "off");
      for (const name of (existsSync(directory) ? readdirSync(directory) : []).filter((n) =>
        /^member-.*\.json$/.test(n),
      ))
        rememberActor(audit, privateRead(join(directory, name)), namespace);
    } catch (error) {
      auditErrors.push(failureCode(error));
    }
  });
  for (const [, directory, serverIdentity] of serverAllocations) {
    try {
      audit.servers.push(
        steps.maintenanceReceipts(
          join(directory, "server.private.log"),
          serverIdentity,
          audit.targetSha256,
        ),
      );
    } catch (error) {
      auditErrors.push(failureCode(error));
    }
  }
  receipt.auditErrors = auditErrors;
  try {
    require(scope.closure(), "UI_RESOURCE_CLOSURE_FAILED");
    steps.currentBuild();
    const final = await steps.fixture(scope, manifest, "baseline", lastEnvironment);
    steps.write(join(steps.root(), "preservation.private.json"), {
      before: baseline,
      after: final,
      audit,
    });
    receipt.preservation = { result: "observed", afterSha256: valueDigest(final) };
    require(auditErrors.length === 0, "UI_OWNERSHIP_AUDIT_FAILED");
    assertPreserved(baseline, final, audit);
    receipt.preservation.result = "PASS";
  } catch (error) {
    receipt.preservation.result = receipt.preservation.afterSha256 ? "FAIL" : "NOTRUN";
    receipt.preservation.failure = failureCode(error);
    receipt.uiResult = "FAIL";
    receipt.originalFailure ??= failureCode(error);
  }
  receipt.receiptWrite = "attempted";
  try {
    steps.write(join(steps.root(), "ui-result.private.json"), receipt);
  } catch {
    receipt.receiptWrite = "failed";
    steps.output.err(
      diagnosticJson({
        originalFailure: receipt.originalFailure,
        receiptWrite: "failed",
        preservation: receipt.preservation,
        cleanupErrors: receipt.cleanupErrors,
      }),
    );
    receipt.originalFailure ??= "UI_RECEIPT_WRITE_FAILED";
  }
  if (failure !== null) throw failure as Error;
  require(receipt.uiResult === "PASS" &&
    receipt.receiptWrite !== "failed", receipt.originalFailure ?? "UI_RECEIPT_WRITE_FAILED");
  return receipt;
}

export interface SmtpSink {
  child: Child;
  port: string;
  capture: string;
}

/** The maintained plaintext sink, an allocation of this scope bound to port 0.
 * Its port file is accepted only when this child's own socket listens there. */
export async function startSmtpSink(
  scope: Scope,
  manifest: Pick<Manifest, "bun">,
  directory: string,
): Promise<SmtpSink> {
  const capture = join(directory, "smtp-capture.private.jsonl");
  const portFile = join(directory, "smtp-port.private");
  const log = openSync(join(directory, "smtp-sink.private.log"), "wx", 0o600);
  let child: Child | null = null;
  try {
    child = scope.spawn(
      [
        manifest.bun.path,
        ...bunIsolation(),
        join(checkout, "tools/web-e2e/smtp-sink.ts"),
        "--capture",
        capture,
        "--port-file",
        portFile,
      ],
      "smtp-sink",
      { env: cleanEnv(), stdin: "ignore", stdout: log, stderr: log },
    );
    const deadline = now() + 10;
    let port = "";
    // Bounded read-only observation of the sink's own readiness file.
    while (!/^[1-9][0-9]{0,4}$/.test(port)) {
      require(poll(child) === null && now() < deadline, "UI_SMTP_SINK_START_FAILED");
      port = existsSync(portFile) ? readFileSync(portFile, "utf8") : "";
      if (!/^[1-9][0-9]{0,4}$/.test(port)) await pause(20);
    }
    let listening: string;
    try {
      listening = await ownedListener(child);
    } catch {
      throw new UiError("UI_SMTP_SINK_START_FAILED");
    }
    require(listening === "http://127.0.0.1:" + port, "UI_SMTP_SINK_START_FAILED");
    return { child, port, capture };
  } catch (error) {
    if (child !== null) {
      try {
        await scope.finish(child, true);
      } catch {
        scope.io.output.err(
          diagnosticJson({
            originalFailure: failureCode(error),
            cleanupErrors: ["UI_SMTP_SINK_CLOSURE_FAILED"],
          }),
        );
      }
    }
    throw error;
  } finally {
    closeSync(log);
  }
}

/** The sink has no signal handler: a normal stop is its SIGTERM exit. */
/** The sink has no signal handler: its normal stop is the SIGTERM exit, and its
 * closure is read from the kernel listener tables right after that exit. */
export async function stopSmtpSink(
  scope: Scope,
  sink: SmtpSink,
  absent: (port: number) => boolean = listenerAbsent,
): Promise<void> {
  const code = await scope.finish(sink.child, true);
  require(code === -15 && absent(Number(sink.port)), "UI_SMTP_SINK_CLOSURE_FAILED");
}

export interface AuthSteps extends FlowSteps {
  smtp: typeof startSmtpSink;
  stopSmtp: typeof stopSmtpSink;
}
export const defaultAuthSteps: AuthSteps = {
  ...defaultSteps,
  smtp: startSmtpSink,
  stopSmtp: stopSmtpSink,
};

export interface AuthReceipt {
  source: string;
  tree: string;
  backend: "libsql-remote";
  counts: { on: number; off: number };
  // The local official sqld run never connects the hosted acceptance DB.
  hosted: "NOT_CONNECTED" | "THIS_RUN";
  cleanupErrors: string[];
  originalFailure: string | null;
  preservation: Record_;
  receiptWrite: string;
  uiResult?: "PASS" | "FAIL";
  auditErrors?: string[];
}

/** One recorded witness value shared by all three cases. */
function sharedWitness(witnesses: Record_[], key: string): string {
  const values = new Set(witnesses.map((witness) => witness[key]));
  const [value] = values;
  require(values.size === 1 && typeof value === "string", "UI_AUTH_WITNESS_REFUSED");
  return value;
}

/**
 * Dedicated auth allocations, one per realtime mode: a fresh namespace, owner
 * and member, an owned SMTP sink and normal server, the three auth cases, the
 * native auth event audit around them and the whole-primary preservation audit.
 * The original ON1/restart1/OFF8 consumer and its counts are separate.
 */
export async function executeAuth(
  scope: Scope,
  manifest: Manifest,
  baseline: Record_,
  environment: Record<string, string>,
  steps: AuthSteps = defaultAuthSteps,
  heavy?: string,
  localSource?: {
    sourceAllocation: number;
    sourceIdentity: { pid: number; startTicks: string };
    processErrors: readonly string[];
  },
): Promise<AuthReceipt> {
  const sourceAllocation = localSource && scope.allocations[localSource.sourceAllocation];
  const sourceEntry = sourceAllocation?.key ? scope.entries.get(sourceAllocation.key) : undefined;
  if (localSource)
    require(executionMode() === "orca-local" &&
      Number.isSafeInteger(localSource.sourceAllocation) &&
      localSource.sourceAllocation >= 0 &&
      localSource.processErrors.length === 0 &&
      sourceAllocation !== undefined &&
      sourceAllocation.label === "official-sqld" &&
      !sourceAllocation.closed &&
      !sourceAllocation.forced &&
      poll(sourceAllocation.process) === null &&
      sourceEntry !== undefined &&
      sourceEntry.label === "official-sqld" &&
      !scope.retired(sourceEntry.identity) &&
      sourceEntry.allocation === localSource.sourceAllocation &&
      sourceEntry.identity.pid === sourceAllocation.process.pid &&
      sourceEntry.identity.pid === localSource.sourceIdentity.pid &&
      sourceEntry.identity.startTicks ===
        localSource.sourceIdentity.startTicks, "UI_LOCAL_SOURCE_REFUSED");
  const source = manifest.sourceInputs;
  require(startupBlockers(baseline).length === 0, "UI_EXISTING_BACKGROUND_WORK_REFUSED");
  const audit: Audit = {
    namespaces: [],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: digest(get(environment, "FVOCI_LIBSQL_URL") as string),
    serverStarts: 0,
    observedFences: [],
    authActors: [],
  };
  const receipt: AuthReceipt = {
    source: source.source,
    tree: source.tree,
    backend: "libsql-remote",
    counts: { on: 0, off: 0 },
    hosted: executionMode() === "orca-local" ? "NOT_CONNECTED" : "THIS_RUN",
    cleanupErrors: [],
    originalFailure: null,
    preservation: { result: "NOTRUN" },
    receiptWrite: "not-attempted",
  };
  let lastEnvironment = environment;
  const serverAllocations: [string, { pid: number; startTicks: string }][] = [];
  let failure: unknown = null;
  try {
    for (const flow of ["on", "off"] as const) {
      const namespace = "tui-" + steps.token(10);
      audit.namespaces.push(namespace);
      const directory = join(steps.root(), "auth-" + flow);
      mkdirSync(directory, 0o700);
      const storage = join(directory, "storage");
      mkdirSync(storage, 0o700);
      let sink: SmtpSink | null = null,
        server: ServerHandle | null = null,
        base: string | null = null,
        log: number | null = null;
      try {
        sink = await steps.smtp(scope, manifest, directory);
        const env: Record<string, string> = {
          ...allocationEnvironment(manifest, environment, namespace, flow, storage),
          // Three case documents keep their rooms within one allocation.
          FVOCI_COLLAB_MAX_ROOMS: "3",
          SMTP_HOST: "127.0.0.1",
          SMTP_PORT: sink.port,
          SMTP_FROM: "auth-session@example.invalid",
        };
        lastEnvironment = env;
        const setupNeeded = flow === "on" && Boolean(get(baseline, "setupNeeded"));
        const { owner, bindingPath, capsule, wrapper } = await allocateActors(
          scope,
          manifest,
          baseline,
          env,
          { audit, namespace, directory, setupNeeded },
          steps,
        );
        ({ server, base, log } = await steps.start(scope, manifest, env, directory, setupNeeded));
        audit.serverStarts += 1;
        serverAllocations.push([directory, steps.serverIdentity(server)]);
        const before = await steps.fixture(scope, manifest, "baseline", env);
        const report = await steps.browser(
          scope,
          manifest,
          directory,
          {
            PLAYWRIGHT_BASE_URL: base,
            FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
            FVOCI_E2E_SELECTED_FLOW: flow,
            FVOCI_E2E_TURSO_NAMESPACE: namespace,
            FVOCI_E2E_TURSO_SOURCE: source.source,
            FVOCI_E2E_TURSO_TREE: source.tree,
            FVOCI_E2E_SELECTED_FIXTURE_BIN: wrapper,
            FVOCI_E2E_TURSO_PRIVATE_INPUT: capsule,
            FVOCI_E2E_TURSO_ACTOR_BINDING: bindingPath,
            FVOCI_E2E_AUTH_SOURCE: source.source,
            FVOCI_E2E_AUTH_TREE: source.tree,
            FVOCI_E2E_SMTP_CAPTURE: sink.capture,
          },
          AUTH_SPEC,
          undefined,
          checkout,
          "auth-session-playwright.config.ts",
          heavy,
        );
        const witnesses = authSessionWitnesses(report, {
          backend: "libsql-remote",
          flow,
          source: source.source,
          tree: source.tree,
        });
        receipt.counts[flow] = witnesses.length;
        const memberId = authSessionActor(witnesses);
        const ownerId = sharedWitness(witnesses, "owner"),
          workspaceId = sharedWitness(witnesses, "workspaceId");
        if (owner === null) {
          audit.actors[uuidHex(ownerId)] = namespace + "-owner@example.invalid";
          audit.workspaces[uuidHex(workspaceId)] = namespace;
        }
        require((owner === null || get(owner, "userId") === ownerId) &&
          (owner === null || get(owner, "workspaceId") === workspaceId), "UI_AUTH_WITNESS_REFUSED");
        const members = readdirSync(directory).filter((n) => /^member-.*\.json$/.test(n));
        require(members.length === 1, "UI_AUTH_MEMBER_RECEIPT_REFUSED");
        const member = privateRead(join(directory, members[0] as string));
        rememberActor(audit, member, namespace);
        require(get(member, "userId") === memberId &&
          get(member, "workspaceId") === workspaceId, "UI_AUTH_MEMBER_RECEIPT_REFUSED");
        (audit.authActors as string[]).push(uuidHex(memberId));
        const documentIds = witnesses.map((witness) => witness.targetId as string);
        for (const id of documentIds)
          (audit.documents as Record<string, string>)[uuidHex(id)] = uuidHex(workspaceId);
        // Every room owner of the three ON case documents is observed for the
        // fence audit; OFF starts no room, which the fence counter audit checks.
        const observed =
          flow === "on"
            ? await steps.fixture(scope, manifest, "observe", env, { workspaceId, documentIds })
            : null;
        for (const id of observed === null ? [] : documentIds)
          audit.observedFences.push(
            ...(list(record(observed, "rows", id), "roomFences") as unknown[][]),
          );
        const after = await steps.fixture(scope, manifest, "baseline", env);
        authSessionEventEffects(before, after, memberId);
        steps.write(join(directory, "auth-events.private.json"), { before, after, observed });
        await steps.stop(scope, server, base, directory);
        server = null;
      } catch (error) {
        receipt.originalFailure ??= failureCode(error);
        throw error;
      } finally {
        if (server !== null) {
          try {
            await steps.stop(scope, server, base as string, directory);
          } catch {
            receipt.cleanupErrors.push("UI_SERVER_CLOSURE_FAILED");
            try {
              await scope.finish(server.child, true);
            } catch {
              receipt.cleanupErrors.push("UI_SERVER_FORCE_RETIRE_FAILED");
            }
          }
        }
        if (log !== null) {
          try {
            closeSync(log);
          } catch {
            receipt.cleanupErrors.push("UI_SERVER_LOG_CLOSE_FAILED");
          }
        }
        if (sink !== null) {
          try {
            await steps.stopSmtp(scope, sink);
          } catch {
            receipt.cleanupErrors.push("UI_SMTP_SINK_CLOSURE_FAILED");
          }
        }
      }
      require(receipt.cleanupErrors.length === 0, "UI_RESOURCE_CLOSURE_FAILED");
    }
    require(deepEquals(receipt.counts, { on: 3, off: 3 }), "UI_AUTH_ACTUAL_COUNTS_FAILED");
    receipt.uiResult = "PASS";
  } catch (error) {
    receipt.uiResult = "FAIL";
    receipt.originalFailure ??= failureCode(error);
    failure = error;
  }
  const auditErrors: string[] = [];
  for (const [directory, serverIdentity] of serverAllocations) {
    try {
      audit.servers.push(
        steps.maintenanceReceipts(
          join(directory, "server.private.log"),
          serverIdentity,
          audit.targetSha256,
        ),
      );
    } catch (error) {
      auditErrors.push(failureCode(error));
    }
  }
  receipt.auditErrors = auditErrors;
  try {
    // Refresh the process observation before checking the local source's
    // exact exception. Its primary must still serve the preservation read;
    // localAuth retires it before withProcesses requires full closure.
    const closed = scope.closure();
    require(localSource
      ? localSource.processErrors.length === 0 &&
          sourceAllocation !== undefined &&
          !sourceAllocation.closed &&
          !sourceAllocation.forced &&
          poll(sourceAllocation.process) === null &&
          sourceEntry !== undefined &&
          !scope.retired(sourceEntry.identity) &&
          scope.allocations.every(
            (allocation) =>
              allocation === sourceAllocation || (allocation.closed && !allocation.forced),
          ) &&
          [...scope.entries.values()].every(
            (entry) => entry === sourceEntry || scope.retired(entry.identity),
          )
      : closed, "UI_RESOURCE_CLOSURE_FAILED");
    steps.currentBuild();
    const final = await steps.fixture(scope, manifest, "baseline", lastEnvironment);
    steps.write(join(steps.root(), "auth-preservation.private.json"), {
      before: baseline,
      after: final,
      audit,
    });
    receipt.preservation = { result: "observed", afterSha256: valueDigest(final) };
    require(auditErrors.length === 0, "UI_OWNERSHIP_AUDIT_FAILED");
    assertPreserved(baseline, final, audit);
    receipt.preservation.result = "PASS";
  } catch (error) {
    receipt.preservation.result = receipt.preservation.afterSha256 ? "FAIL" : "NOTRUN";
    receipt.preservation.failure = failureCode(error);
    receipt.uiResult = "FAIL";
    receipt.originalFailure ??= failureCode(error);
  }
  receipt.receiptWrite = "attempted";
  try {
    steps.write(join(steps.root(), "auth-result.private.json"), receipt);
  } catch {
    receipt.receiptWrite = "failed";
    steps.output.err(
      diagnosticJson({
        originalFailure: receipt.originalFailure,
        receiptWrite: "failed",
        preservation: receipt.preservation,
        cleanupErrors: receipt.cleanupErrors,
      }),
    );
    receipt.originalFailure ??= "UI_RECEIPT_WRITE_FAILED";
  }
  if (failure !== null) throw failure as Error;
  require(receipt.uiResult === "PASS" &&
    receipt.receiptWrite !== "failed", receipt.originalFailure ?? "UI_RECEIPT_WRITE_FAILED");
  return receipt;
}

export interface ConsumeSteps {
  currentBuild: () => Manifest;
  fixture: typeof fixture;
  executeUi: typeof executeUi;
  executeRestore?: typeof executeRestore;
  executeAuth?: (
    scope: Scope,
    manifest: Manifest,
    baseline: Record_,
    environment: Record<string, string>,
  ) => Promise<AuthReceipt>;
  write: typeof write;
  root: () => string;
  output: Output;
}
const consumeSteps: ConsumeSteps = {
  currentBuild: () => currentBuild(lease.load),
  fixture,
  executeUi,
  executeRestore: async (...args) => (await import("./ui-restore.ts")).executeRestore(...args),
  executeAuth: (scope, manifest, baseline, environment) =>
    executeAuth(scope, manifest, baseline, environment),
  write,
  root,
  output: stdio,
};

export async function consumeIn(
  scope: Scope,
  phase: string,
  inputs: Record<string, unknown>,
  steps: ConsumeSteps = consumeSteps,
): Promise<void> {
  require(["ui-baseline", "ui-ack", "restore", "ui-auth"].includes(phase), "UI_PHASE_REFUSED");
  const manifest = steps.currentBuild();
  require(inputs.ui_source_sha === manifest.sourceInputs.source, "UI_REVIEWED_SOURCE_REQUIRED");
  let encryptionKeys: string | undefined;
  if (phase === "ui-ack" || phase === "restore" || phase === "ui-auth") {
    const raw = process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS;
    require(typeof raw === "string" && raw.length > 0, "UI_ENCRYPTION_KEYRING_REQUIRED");
    let keys: unknown;
    try {
      keys = JSON.parse(raw) as unknown;
    } catch {
      require(false, "UI_ENCRYPTION_KEYRING_REFUSED");
    }
    require(isRecord(keys) &&
      Object.keys(keys).length === 1 &&
      Object.hasOwn(keys, "turso-test") &&
      typeof keys["turso-test"] === "string" &&
      keys["turso-test"].length === 64 &&
      /^[a-fA-F0-9]{64}$/.test(keys["turso-test"]), "UI_ENCRYPTION_KEYRING_REFUSED");
    encryptionKeys = JSON.stringify(keys);
  }
  const environment: Record<string, string> = {
    FVOCI_LIBSQL_URL: get(process.env, "FVOCI_LIBSQL_URL") as string,
    FVOCI_LIBSQL_AUTH_TOKEN: get(process.env, "FVOCI_LIBSQL_AUTH_TOKEN") as string,
  };
  const baseline = await steps.fixture(scope, manifest, "baseline", environment);
  steps.write(join(steps.root(), "baseline.private.json"), baseline);
  const baselineSha = valueDigest(baseline);
  const targetSha = digest(environment.FVOCI_LIBSQL_URL as string);
  if (phase === "ui-baseline") {
    steps.output.out(
      "TURSO_UI_BASELINE_PASS source=" +
        manifest.sourceInputs.source +
        " baseline_sha256=" +
        baselineSha +
        " target_sha256=" +
        targetSha +
        " rows=" +
        String(get(baseline, "rows")) +
        " setup_needed=" +
        String(get(baseline, "setupNeeded")).toLowerCase() +
        " startup_admissible=" +
        String(startupBlockers(baseline).length === 0),
    );
    return;
  }
  require((phase === "ui-ack" || phase === "restore" || phase === "ui-auth") &&
    inputs.ui_baseline_sha256 === baselineSha, "UI_CURRENT_DATASET_BINDING_REQUIRED");
  require(inputs.ui_target_sha256 === targetSha, "UI_CURRENT_TARGET_BINDING_REQUIRED");
  Object.assign(environment, {
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
    PASSWORD_PEPPER_KEYS: diagnosticJson({ fixture: token(32) }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: encryptionKeys,
    ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
  });
  if (phase === "restore") {
    require(steps.executeRestore !== undefined, "RESTORE_CONSUMER_REQUIRED");
    const result = await steps.executeRestore(scope, manifest, baseline, environment);
    require(result.result === "PASS" &&
      result.originalFailure === null &&
      result.cleanupErrors.length === 0 &&
      result.auditErrors.length === 0 &&
      deepEquals(result.counts, [1, 1, 1, 1, 1, 1]) &&
      result.preservation.result === "PASS" &&
      result.sourceUnchangedDuringDestination === "PASS", "RESTORE_ACCEPTANCE_FAILED");
    steps.output.out(
      "TURSO_RESTORE_PASS u6=1 source_restart=1 export=1 destination=1 destination_restart=1 compare=1 retries=0 ignored=0 source=libsql-remote destination=sqlite cost=NOTRUN",
    );
    return;
  }
  if (phase === "ui-auth") {
    require(steps.executeAuth !== undefined, "UI_AUTH_CONSUMER_REQUIRED");
    const result = await steps.executeAuth(scope, manifest, baseline, environment);
    // executeAuth returns only after counts, event effects, cleanup and
    // preservation pass; each is checked again before the pass line.
    require(result.uiResult === "PASS" &&
      result.originalFailure === null &&
      result.cleanupErrors.length === 0 &&
      deepEquals(result.auditErrors, []) &&
      deepEquals(result.counts, { on: 3, off: 3 }) &&
      result.preservation.result === "PASS", "UI_AUTH_ACCEPTANCE_FAILED");
    steps.output.out(
      "TURSO_UI_AUTH_PASS on=3 off=3 retries=0 ignored=0 events=PASS preservation=PASS source=libsql-remote cost=NOTRUN",
    );
    return;
  }
  const result = await steps.executeUi(scope, manifest, baseline, environment);
  steps.output.out(
    "TURSO_UI_ACK_PASS on=1 restart=1 off=8 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN",
  );
  require(result.cleanupErrors.length === 0, "UI_RESOURCE_CLOSURE_FAILED");
}

/** The guard's entry: one owned process scope around the whole consumer. */
export async function consume(phase: string, inputs: Record<string, unknown>): Promise<void> {
  await withProcesses((scope) => consumeIn(scope, phase, inputs));
}

export async function actor(scope: Scope, output: Output = stdio): Promise<void> {
  if (executionMode() === "orca-local") loadLocalLease("actor", lease.load);
  const inputPath = get(process.env, "FVOCI_E2E_TURSO_PRIVATE_INPUT") as string;
  const capsule = privateRead(inputPath);
  const namespace = process.env.FVOCI_E2E_TURSO_NAMESPACE ?? "";
  require(namespace === get(capsule, "namespace") &&
    /^tui-[a-f0-9]{20}$/.test(namespace), "UI_ACTOR_NAMESPACE_REFUSED");
  const expected: Record<string, string> = {
    E2E_USER_EMAIL: namespace + "-member@example.invalid",
    E2E_USER_PASSWORD: "memberpass1",
    E2E_USER_GIVEN_NAME: "협업",
    E2E_USER_FAMILY_NAME: "멤버",
    E2E_WORKSPACE_SLUG: namespace,
    E2E_MEMBERSHIP_ROLE: "member",
    E2E_DATABASE_BACKEND: "libsql-remote",
  };
  require(Object.entries(expected).every(
    ([key, value]) => process.env[key] === value,
  ), "UI_ACTOR_INPUT_REFUSED");
  const manifest = get(capsule, "manifest") as Manifest;
  const binary = record(manifest, "binaries", "fvoci-e2e-fixture");
  require(sha(binary.path as string) === binary.sha256, "UI_ACTOR_BINARY_CHANGED");
  const workspaceId = get(capsule, "workspaceId");
  const result = await fixture(
    scope,
    manifest,
    "member",
    get(capsule, "environment") as Record<string, string>,
  );
  require(get(result, "namespace") === namespace &&
    (workspaceId === null || get(result, "workspaceId") === workspaceId) &&
    get(result, "commit") === "confirmed" &&
    Boolean(get(result, "freshPrimaryReadback")), "UI_ACTOR_RECEIPT_FAILED");
  write(join(dirname(inputPath), "member-" + String(get(result, "userId")) + ".json"), result);
  output.out(String(get(result, "userId")));
}

/**
 * Local evidence only: the dedicated auth allocations against one owned
 * official sqld behind the standard TLS endpoint. Hosted auth is not
 * connected; the hosted guard and target allowlist remain separate.
 */
export async function localAuth(): Promise<AuthReceipt> {
  // Only a grant whose exact runtime argv is --local-auth runs this consumer.
  const grant = localTursoUiRun("--local-auth");
  const manifest = currentBuild(lease.load);
  const selected = grant.localLibsql;
  require(selected !== undefined, "UI_LOCAL_ALLOCATION_REFUSED");
  mkdirSync(selected.directory, { recursive: true, mode: 0o700 });
  require(realpathSync(selected.directory) === selected.directory &&
    lstatSync(selected.directory).isDirectory() &&
    (lstatSync(selected.directory).mode & 0o777) === 0o700, "UI_LOCAL_ROOT_REFUSED");
  return withProcesses(async (scope) => {
    const before = scope.allocations.length;
    const directory = join(selected.directory, selected.label + "-" + token(6));
    const local = await startLocalLibsql(scope, selected.binary.path, directory);
    let original: unknown = null,
      result: AuthReceipt | null = null;
    try {
      const sourceAllocation = scope.allocations.findIndex(
        (a, index) => index >= before && a.label === "official-sqld",
      );
      const child = scope.allocations[sourceAllocation]?.process;
      require(child !== undefined &&
        scope.allocations.slice(before).filter((a) => a.label === "official-sqld").length ===
          1, "UI_LOCAL_SOURCE_REFUSED");
      const upstream = await ownedListener(child);
      const url = new URL(local.environment.FVOCI_LIBSQL_URL as string);
      const ca = local.environment.SSL_CERT_FILE as string;
      const sourceIdentity = identity(child.pid);
      require(url.protocol === "https:" &&
        url.hostname === "127.0.0.1" &&
        url.port !== "" &&
        url.port !== "0" &&
        url.pathname === "/" &&
        url.search === "" &&
        url.hash === "" &&
        url.username === "" &&
        url.password === "", "UI_LOCAL_TARGET_REFUSED");
      require(ca === join(directory, "tls.crt") &&
        realpathSync(ca) === ca &&
        (lstatSync(ca).mode & 0o777) === 0o600 &&
        sha(ca) === local.certificateSha256, "UI_LOCAL_CA_REFUSED");
      // The official-pinned child and its own socket precede publication of
      // the private input and any native/browser consumption.
      write(join(root(), "local-auth-source-binding.private.json"), {
        source: grant.source,
        tree: grant.tree,
        inputSha256: grant.inputSha256,
        manifestSha256: sha(join(root(), "current-build.json")),
        physicalSha256: manifest.physicalInputs.sha256,
        child: sourceIdentity,
        targetSha256: digest(url.origin),
        upstreamSha256: digest(upstream),
        certificateSha256: local.certificateSha256,
        sqldSha256: sha(selected.binary.path),
        loopbackOnly: true,
        certificateSan: "IP-loopback",
        browserCredentials: false,
      });
      const environment: Record<string, string> = {
        ...local.environment,
        FVOCI_DATABASE_BACKEND: "libsql-remote",
        FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
        FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
        PASSWORD_PEPPER_KEYS: diagnosticJson({ fixture: token(32) }),
        PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
        ENCRYPTION_KEYS: diagnosticJson({ "turso-test": token(32) }),
        ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
      };
      // Migration only, as the hosted migration phase leaves the primary:
      // a server start would register relay consumers before the baseline.
      const bootstrap = join(root(), "local-auth-bootstrap");
      mkdirSync(bootstrap, 0o700);
      const storage = join(bootstrap, "storage");
      mkdirSync(storage, 0o700);
      const log = openSync(join(bootstrap, "migrate.private.log"), "wx", 0o600);
      try {
        const migrate = scope.spawn(
          [get(manifest, "binaries", "fvoci-migrate", "path") as string],
          "local-auth-migrate",
          {
            env: { ...cleanEnv(), ...local.environment, FVOCI_DATABASE_BACKEND: "libsql-remote" },
            stdin: "ignore",
            stdout: log,
            stderr: log,
          },
        );
        require((await wait(migrate, 120)) === 0, "UI_LOCAL_MIGRATION_FAILED");
        await scope.finish(migrate);
      } finally {
        closeSync(log);
      }
      // The local fixture runs in the same container shape and needs a storage
      // mount; the final preservation observation reuses this environment.
      const observed = { ...environment, FVOCI_STORAGE_DIR: storage };
      const baseline = await fixture(scope, manifest, "baseline", observed);
      write(join(root(), "local-auth-baseline.private.json"), {
        baseline,
        source: grant.source,
        tree: grant.tree,
        baselineSha256: valueDigest(baseline),
        targetSha256: digest(environment.FVOCI_LIBSQL_URL as string),
      });
      result = await executeAuth(
        scope,
        manifest,
        baseline,
        observed,
        defaultAuthSteps,
        grant.heavy.path,
        { sourceAllocation, sourceIdentity, processErrors: scope.errors },
      );
    } catch (error) {
      original = error;
    }
    try {
      await local.close();
    } catch {
      if (original === null) original = new UiError("UI_LOCAL_SOURCE_CLOSURE_FAILED");
      else
        scope.io.output.err(
          diagnosticJson({
            originalFailure: failureCode(original),
            cleanupErrors: ["UI_LOCAL_SOURCE_CLOSURE_FAILED"],
          }),
        );
    }
    if (original !== null) throw original as Error;
    return result as AuthReceipt;
  });
}
