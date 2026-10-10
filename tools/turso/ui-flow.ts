// The guarded ON, restart and OFF consumer against the current primary, its
// preservation audit, and the member actor the browser fixture calls back.
import { deepEquals } from "bun";
import {
  closeSync,
  existsSync,
  fchmodSync,
  mkdirSync,
  openSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { root as checkout, sha, write } from "../selected-backend-ci/io.ts";
import {
  assertPreserved,
  attachment,
  maintenanceReceipts,
  OFF,
  ON,
  rememberActor,
  reportCases,
  startupBlockers,
  uuidHex,
  type Audit,
} from "./ui-audit.ts";
import {
  cleanEnv,
  diagnosticJson,
  executionMode,
  failureCode,
  get,
  list,
  privateRead,
  record,
  require,
  root,
  stdio,
  textDigest,
  token,
  valueDigest,
  type Output,
  type Record_,
} from "./ui-common.ts";
import { lease, shellQuote, type ServerHandle } from "./ui-container.ts";
import { browser, fixture, registeredTitles, start, stop } from "./ui-native.ts";
import { identity, withProcesses, type Scope } from "./ui-processes.ts";
import { currentBuild, loadLocalLease, type Manifest } from "./ui-record.ts";

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

/** The executable the browser fixture runs to create the member actor. */
export function memberWrapper(): string {
  return "#!/bin/sh\nexec " + shellQuote(process.execPath) + " " + shellQuote(ENTRY) + " --actor\n";
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
    servers: [],
    targetSha256: textDigest(get(environment, "FVOCI_LIBSQL_URL") as string),
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
      lastEnvironment = env;
      const setupNeeded = flow === "on" && Boolean(get(baseline, "setupNeeded"));
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
      steps.write(capsule, {
        manifest,
        environment: env,
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
          flow === "on" ? "^selected normal main:" : undefined,
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
          const query = { workspaceId: get(seed, "workspaceId"), documentIds: [documentId] };
          const observed = await steps.fixture(scope, manifest, "observe", env, query);
          const row = record(observed, "rows", documentId);
          audit.observedFences.push(...(list(row, "roomFences") as unknown[][]));
          // Persist ACK IDs must name actual operation receipts; the observer
          // already checks payload lengths, digests and tail.
          const acked = (receipts: unknown[], ack: unknown) =>
            receipts.some((r) => get(r, "op") === ack);
          for (const ack of [get(seed, "firstAck"), get(seed, "finalAck")])
            require(acked(list(row, "receipts"), ack), "UI_DURABLE_ACK_RECEIPT_MISSING");
          require(deepEquals(get(row, "content"), get(seed, "persisted", "contentJson"), true) &&
            deepEquals(
              get(row, "text"),
              get(seed, "persisted", "text"),
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
              "^selected normal main restart:",
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
            [get(seed, "firstAck"), get(seed, "finalAck")].every((ack) =>
              acked(list(fresh, "receipts"), ack),
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

export interface ConsumeSteps {
  currentBuild: () => Manifest;
  fixture: typeof fixture;
  executeUi: typeof executeUi;
  write: typeof write;
  root: () => string;
  output: Output;
}
const consumeSteps: ConsumeSteps = {
  currentBuild: () => currentBuild(lease.load),
  fixture,
  executeUi,
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
  const manifest = steps.currentBuild();
  require(inputs.ui_source_sha === manifest.sourceInputs.source, "UI_REVIEWED_SOURCE_REQUIRED");
  const environment: Record<string, string> = {
    FVOCI_LIBSQL_URL: get(process.env, "FVOCI_LIBSQL_URL") as string,
    FVOCI_LIBSQL_AUTH_TOKEN: get(process.env, "FVOCI_LIBSQL_AUTH_TOKEN") as string,
  };
  const baseline = await steps.fixture(scope, manifest, "baseline", environment);
  steps.write(join(steps.root(), "baseline.private.json"), baseline);
  const baselineSha = valueDigest(baseline);
  const targetSha = textDigest(environment.FVOCI_LIBSQL_URL as string);
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
  require(phase === "ui-ack" &&
    inputs.ui_baseline_sha256 === baselineSha, "UI_CURRENT_DATASET_BINDING_REQUIRED");
  require(inputs.ui_target_sha256 === targetSha, "UI_CURRENT_TARGET_BINDING_REQUIRED");
  Object.assign(environment, {
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
    PASSWORD_PEPPER_KEYS: diagnosticJson({ fixture: token(32) }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
  });
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
