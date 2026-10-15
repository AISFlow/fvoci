// The final consumer owns the fixed serial call path and private whole-primary
// observations. Standalone consumers retain their original admission rules.
import { deepEquals } from "bun";
import { closeSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { randomUUID } from "node:crypto";
import { digest, root as checkout, sha } from "../selected-backend-ci/io.ts";
import {
  validateSerialObservation,
  assertNativeClock,
  startupBlockers,
  type SerialObservation,
  type SerialPolicy,
  assertPreserved,
  rememberActor,
  maintenanceReceipts,
  uuidHex,
  type Audit,
  reportCases,
  attachment,
  assertSerialGroupPreserved,
  roomRetirement,
  retainRoomHistory,
  assertRoomClosure,
} from "./ui-audit.ts";
import {
  diagnosticJson,
  failureCode,
  get,
  isRecord,
  list,
  record,
  cleanEnv,
  require,
  root,
  token,
  valueDigest,
  stdio,
  type Record_,
} from "./ui-common.ts";
import { docker, hostedFinalServerStart, lease } from "./ui-container.ts";
import {
  defaultSteps,
  defaultAuthSteps,
  executeUi,
  executeAuth,
  type SerialExecution,
  allocationEnvironment,
  allocationIdentity,
} from "./ui-flow.ts";
import { browser, fixture, start, stop } from "./ui-native.ts";
import { withProcesses, type Scope, type UiProcesses } from "./ui-processes.ts";
import { currentBuild, recheckFinalBrowsers, type Manifest } from "./ui-record.ts";
import { executeRestore } from "./ui-restore.ts";
import type {
  KcMode,
  PreparedActors,
  PreparedBinding,
} from "../../apps/web/e2e-keycloak/config.ts";
import {
  preparedReadValue,
  type Observation,
  type LinkRow,
  type PreparedSnapshot,
} from "../../apps/web/e2e-keycloak/observer.ts";
import { consumeKeycloak, runCommand, MODES } from "../keycloak/run.ts";
import {
  FINAL_SPEC,
  finalTitles,
  type FinalBinding,
} from "../../apps/web/e2e-pending/selected-final-contract.ts";
import {
  auditSelectedFinalNative,
  type SelectedSyncWrite,
  type SelectedNativeReceipt,
} from "../../apps/web/e2e-pending/selected-final-native.ts";
import { write } from "../selected-backend-ci/io.ts";
import type { BrowserEngine } from "./ui-record.ts";
import {
  measureHosted,
  compareHostedPair,
  type HostedCase,
  type HostedInputs,
  type HostedSeed,
} from "./ui-equal-work.ts";
import { workload, workloadHash } from "../selected-backend-ci/drivers/equal-work-policy.ts";
import type { Command } from "../selected-backend-ci/drivers/common.ts";

/** Public requests are confined to this actual owned normal server. */
async function requestPublic(base: string, path: string, body?: Record_, cookie?: string) {
  require(/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/.test(base) &&
    path.startsWith("/api/v1/") &&
    !path.includes(".."), "UI_FINAL_PUBLIC_TARGET_REFUSED");
  const response = await fetch(base + path, {
    method: body ? "POST" : "GET",
    redirect: "manual",
    signal: AbortSignal.timeout(10000),
    headers: {
      Origin: base,
      ...(body ? { "Content-Type": "application/json" } : {}),
      ...(cookie ? { Cookie: cookie } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  require(response.ok, "UI_FINAL_PUBLIC_PREPARATION_FAILED");
  const value: unknown = await response.json();
  return {
    value,
    cookie: response.headers
      .getSetCookie()
      .map((value) => value.split(";", 1)[0])
      .join("; "),
  };
}

async function inviteFinalActor(
  base: string,
  workspace: string,
  ownerCookie: string,
  email: string,
  password: string,
  role: "owner" | "member",
  name: string,
) {
  const invitation = await requestPublic(
    base,
    "/api/v1/workspaces/" + workspace + "/invitations",
    { email, role },
    ownerCookie,
  );
  const accepted = new URL(String(get(invitation.value, "acceptUrl")), base);
  require(accepted.origin === base &&
    /^\/invite\/[a-zA-Z0-9_-]+$/.test(accepted.pathname), "UI_FINAL_INVITATION_TARGET_REFUSED");
  const inviteToken = accepted.pathname.slice("/invite/".length);
  const preview = await requestPublic(base, "/api/v1/invitations/" + inviteToken);
  const consents = list(preview.value, "requiredLegal").map((item) => ({
    kind: get(item, "kind"),
    version: get(item, "version"),
  }));
  const issued = await requestPublic(base, "/api/v1/invitations/" + inviteToken + "/accept", {
    email,
    password,
    givenName: "Selected",
    familyName: name,
    consents,
  });
  require(issued.cookie.length > 0, "UI_FINAL_OWNER_INVITATION_FAILED");
  const owner = await requestPublic(base, "/api/v1/auth/login", { email, password });
  require(owner.cookie.length > 0 &&
    get(owner.value, "userId") ===
      get(issued.value, "userId"), "UI_FINAL_PREPARED_OWNER_LOGIN_FAILED");
  const userId = String(get(owner.value, "userId"));
  uuidHex(userId);
  return { userId, email, password, cookie: owner.cookie };
}

const canonicalId = (value: unknown): string => {
  require(typeof value === "string" && /^[a-f0-9]{32}$/.test(value), "UI_ALLOCATION_ID_REFUSED");
  return (
    value.slice(0, 8) +
    "-" +
    value.slice(8, 12) +
    "-" +
    value.slice(12, 16) +
    "-" +
    value.slice(16, 20) +
    "-" +
    value.slice(20)
  );
};

function equalConfig(environment: Record<string, string>, policy: SerialPolicy): string {
  const keys = [
    "FVOCI_COLLAB_FAMILY_LEASE_MS",
    "FVOCI_COLLAB_FAMILY_RENEW_MS",
    "FVOCI_COLLAB_MAX_ROOMS",
    "FVOCI_MAINTENANCE_TICK_SECS",
    "FVOCI_MAINTENANCE_INTERVAL_SECS",
    "FVOCI_UPLOAD_GC_INTERVAL_SECS",
    "FVOCI_REVISION_SWEEP_INTERVAL_SECS",
    "METRICS_ALLOW_IPS",
    "REVISION_KEEP",
    "REVISION_SNAPSHOT_INTERVAL_HOURS",
  ];
  return valueDigest({
    policy,
    settings: Object.fromEntries(keys.map((key) => [key, get(environment, key)])),
  });
}

function oidcEnvironment(environment: Record<string, string>): Record<string, string> {
  return {
    ...cleanEnv(),
    FVOCI_KC_OBSERVER_SELECTED: "1",
    E2E_DATABASE_BACKEND: "libsql-remote",
    FVOCI_LIBSQL_URL: environment.FVOCI_LIBSQL_URL as string,
    FVOCI_LIBSQL_AUTH_TOKEN: environment.FVOCI_LIBSQL_AUTH_TOKEN as string,
  };
}

async function readOidc(
  scope: UiProcesses,
  manifest: Manifest,
  environment: Record<string, string>,
  input: Exclude<Observation, { operation: "membership" }>,
): Promise<Record_> {
  currentBuild(lease.load);
  const result = await runCommand(
    scope,
    [manifest.binaries["fvoci-e2e-fixture"]?.path ?? "", "oidc-observe"],
    { env: oidcEnvironment(environment), input: JSON.stringify(input), seconds: 30 },
  );
  require(result.code === 0 &&
    Buffer.byteLength(result.stdout) <= 1024 * 1024, "UI_FINAL_KEYCLOAK_NATIVE_READ_FAILED");
  const parsed = record(JSON.parse(result.stdout));
  preparedReadValue(input, parsed);
  return parsed;
}

async function executeFinalKeycloak(
  manifest: Manifest,
  environment: Record<string, string>,
  policy: SerialPolicy,
  origin: SerialObservation,
): Promise<{ observation: SerialObservation; audit: Audit }> {
  const prepared = await prepareKeycloakOwners(manifest, environment, policy, origin);
  const directory = join(root(), "final-keycloak");
  mkdirSync(directory, { mode: 0o700 });
  require(manifest.finalProcessObserver !== undefined, "UI_FINAL_PROCESS_OBSERVER_REQUIRED");
  const target = digest(environment.FVOCI_LIBSQL_URL as string),
    bindings: Partial<Record<KcMode, PreparedBinding>> = {};
  const ordinals: Partial<Record<KcMode, number>> = {};
  const beforeUnlink: SerialObservation[] = [];
  const audit = prepared.audit;
  const keycloakActors = new Set(
    Object.values(prepared.groups).map((actors) => uuidHex(actors.owner.id)),
  );
  let accepted = prepared.observation,
    completed = 0;
  await withProcesses(async (scope) => {
    const checkBinding = (binding: PreparedBinding) => {
      currentBuild(lease.load);
      require(binding.source === manifest.sourceInputs.source &&
        binding.tree === manifest.sourceInputs.tree &&
        binding.target === target &&
        deepEquals(bindings[binding.group], binding, true), "UI_FINAL_KEYCLOAK_BINDING_CHANGED");
    };
    const observe = async (binding: PreparedBinding): Promise<PreparedSnapshot> => {
      checkBinding(binding);
      const observation = await readFinalPrimary(scope, manifest, environment, policy);
      const links = await readOidc(scope, manifest, environment, { operation: "links" });
      const states = await readOidc(scope, manifest, environment, { operation: "states" });
      return {
        binding: structuredClone(binding),
        users: list(observation.checks, "users") as PreparedSnapshot["users"],
        memberships: list(observation.checks, "memberships") as PreparedSnapshot["memberships"],
        links: links.value as LinkRow[],
        states: states.value as number,
        wholePrimary: { dataset: observation.dataset, checks: observation.checks },
      };
    };
    await consumeKeycloak(scope, {
      manifest,
      directory,
      processObserver: manifest.finalProcessObserver as NonNullable<
        Manifest["finalProcessObserver"]
      >,
      checkInputs: () => {
        require(deepEquals(currentBuild(lease.load), manifest, true), "UI_FINAL_INPUTS_CHANGED");
      },
      prepare: (mode, group, binding) => {
        require(MODES.includes(mode as KcMode) &&
          binding !== undefined &&
          binding.group === mode &&
          bindings[mode] === undefined &&
          binding.source === manifest.sourceInputs.source &&
          binding.tree === manifest.sourceInputs.tree &&
          binding.target === target, "UI_FINAL_KEYCLOAK_GROUP_CHANGED");
        const actors = prepared.groups[mode];
        bindings[mode] = structuredClone(binding);
        const storage = join(group, "retained-storage");
        mkdirSync(storage, { mode: 0o700 });
        return Promise.resolve({
          backend: "libsql-remote" as const,
          prepared: binding,
          environment: allocationEnvironment(
            manifest,
            environment,
            actors.workspace.slug,
            "on",
            storage,
          ),
          observer: oidcEnvironment(environment),
          close: async () => {},
        });
      },
      prepared: {
        source: manifest.sourceInputs.source,
        tree: manifest.sourceInputs.tree,
        target,
        groups: prepared.groups,
        observe,
        observeOperation: async (request) => {
          checkBinding(request.binding);
          scope.snapshot();
          const peer = scope.entries.get(String(request.peer.pid) + ":" + request.peer.startTicks);
          require(peer !== undefined &&
            !scope.retired(peer.identity) &&
            request.ordinal ===
              (ordinals[request.binding.group] ?? 0) + 1, "UI_FINAL_KEYCLOAK_PEER_CHANGED");
          let result: Record_;
          if (request.input.operation === "before-unlink") {
            const input = request.input;
            const actors = prepared.groups.failures;
            require(request.binding.group === "failures" &&
              beforeUnlink.length === 0 &&
              request.actor === actors.member.email &&
              input.email === actors.member.email &&
              input.identityEmail === actors.users.bob.email, "UI_FINAL_KEYCLOAK_UNLINK_REFUSED");
            const links = await readOidc(scope, manifest, environment, { operation: "links" });
            const matches = (links.value as LinkRow[]).filter(
              (link) =>
                link.user === input.email &&
                link.provider === "generic" &&
                link.subject === input.subject &&
                link.email === input.identityEmail &&
                link.issuer === request.binding.issuer,
            );
            require(matches.length === 1, "UI_FINAL_KEYCLOAK_UNLINK_IDENTITY_CHANGED");
            const beforeClock = BigInt(Date.now()) * 1000n;
            const native = await fixture(scope, manifest, "serial-observe", environment, {
              ...policy,
            });
            const snapshot = {
              dataset: record(native, "dataset"),
              checks: record(native, "checks"),
            };
            assertNativeClock(snapshot, beforeClock, BigInt(Date.now()) * 1000n);
            const users = list(snapshot.checks, "users").filter(
              (user) => get(user, "email") === input.email,
            );
            require(users.length === 1, "UI_FINAL_KEYCLOAK_UNLINK_ACTOR_CHANGED");
            const actor = uuidHex(get(users[0], "id"));
            const rows = Object.entries(
              record(snapshot.dataset, "allocations", "identity_links"),
            ).filter(([, row]) => deepEquals(get(row, "actors"), [actor], true));
            require(rows.length === 1 &&
              get(snapshot.dataset, "fingerprints", "identity_links", rows[0]?.[0] ?? "") ===
                1, "UI_FINAL_KEYCLOAK_UNLINK_ROW_CHANGED");
            canonicalId(get(rows[0]?.[1], "self"));
            beforeUnlink.push(snapshot);
            result = { ack: true };
          } else result = await readOidc(scope, manifest, environment, request.input);
          ordinals[request.binding.group] = request.ordinal;
          return result;
        },
        assertWholePrimary: (before, after, delta, witness) => {
          checkBinding(before.binding);
          require(before.binding.group === MODES[completed] &&
            deepEquals(before.binding, after.binding, true) &&
            deepEquals(witness.binding, before.binding, true), "UI_FINAL_KEYCLOAK_ORDER_CHANGED");
          const previous = retainRoomHistory(
            {
              dataset: record(before.wholePrimary, "dataset"),
              checks: record(before.wholePrimary, "checks"),
            },
            accepted,
          );
          const current = {
            dataset: record(after.wholePrimary, "dataset"),
            checks: record(after.wholePrimary, "checks"),
          };
          require(deepEquals(
            previous.dataset,
            accepted.dataset,
            true,
          ), "UI_FINAL_PREDECESSOR_CHANGED");
          require(witness.server.executable.path === manifest.binaries["fvoci-server"]?.path &&
            witness.server.executable.sha256 === manifest.binaries["fvoci-server"].sha256 &&
            witness.retirement.exitCode === 0 &&
            witness.retirement.allocationClosed &&
            !witness.retirement.forced &&
            witness.retirement.portClosed &&
            witness.retirement.errors.length === 0 &&
            witness.retirement.identities.length > 0 &&
            witness.retirement.identities.every((row) => row.retired && scope.retired(row)) &&
            sha(witness.log.path) ===
              witness.log.sha256, "UI_FINAL_KEYCLOAK_NORMAL_RETIREMENT_REQUIRED");
          const maintenance = maintenanceReceipts(
            witness.log.path,
            witness.server.identity,
            target,
          );
          retainRoomHistory(
            current,
            accepted,
            roomRetirement(scope, witness.log.path, witness.server.identity, target),
          );
          require(deepEquals(
            maintenance,
            witness.maintenance,
            true,
          ), "UI_FINAL_KEYCLOAK_MAINTENANCE_CHANGED");
          audit.servers.push(maintenance);
          completed++;
          audit.serverStarts = 3 + completed;
          const actors = prepared.groups[before.binding.group];
          const expected =
            before.binding.group === "flows"
              ? [actors.users.carol.email, actors.mismatchEmail, actors.users.erin.email]
              : before.binding.group === "failures"
                ? [actors.member.email]
                : [];
          require(deepEquals(
            [...delta.emails].sort(),
            expected.sort(),
            true,
          ), "UI_FINAL_KEYCLOAK_USER_DELTA_CHANGED");
          for (const email of delta.emails) {
            const users = list(current.checks, "users").filter(
              (user) => get(user, "email") === email,
            );
            require(users.length === 1, "UI_FINAL_KEYCLOAK_USER_NATIVE_REQUIRED");
            const actor = uuidHex(get(users[0], "id"));
            audit.actors[actor] = email;
            keycloakActors.add(actor);
          }
          require(beforeUnlink.length ===
            (completed >= 2 ? 1 : 0), "UI_FINAL_KEYCLOAK_UNLINK_WITNESS_REQUIRED");
          audit.keycloak = { actors: [...keycloakActors], beforeUnlink };
          assertSerialGroupPreserved(
            origin,
            previous,
            current,
            audit,
            policy,
            completed as 1 | 2 | 3,
          );
          accepted = current;
          return Promise.resolve();
        },
      },
    });
    require(completed === 3 && beforeUnlink.length === 1, "UI_FINAL_KEYCLOAK_PUBLIC16_REQUIRED");
  });
  return { observation: accepted, audit };
}

async function executeEqualCase(
  manifest: Manifest,
  environment: Record<string, string>,
  policy: SerialPolicy,
  before: SerialObservation,
  flow: "on" | "off",
): Promise<{ observation: SerialObservation; value: HostedCase }> {
  require(manifest.finalRuntime !== undefined &&
    manifest.finalBrowsers !== undefined, "UI_FINAL_EQUAL_INPUTS_REQUIRED");
  const namespace = "tui-" + token(10),
    directory = join(root(), "final-equal-" + flow);
  mkdirSync(directory, { mode: 0o700 });
  const storage = join(directory, "storage");
  mkdirSync(storage, { mode: 0o700 });
  const env = {
    ...allocationEnvironment(manifest, environment, namespace, flow, storage),
    METRICS_ALLOW_IPS: "127.0.0.1",
  };
  const logical: HostedSeed["logical"] = {
    users: workload.users,
    documents: workload.documents,
    roles: ["owner", "member"],
    initialBody: "",
    preparation: workload.preparation,
  };
  const inputs: HostedInputs = {
    source: manifest.sourceInputs.source,
    tree: manifest.sourceInputs.tree,
    workloadHash,
    sourceInputsHash: valueDigest({
      source: manifest.sourceInputs,
      physical: manifest.physicalInputs,
      sqlite: manifest.sqliteInputs,
      runtime: manifest.finalRuntime,
    }),
    image: manifest.finalRuntime.imageId,
    native: Object.fromEntries(
      Object.entries(manifest.binaries).map(([name, binary]) => [name, binary.sha256]),
    ),
    assets: manifest.assets,
    browser: {
      bun: { path: manifest.bun.path, sha256: manifest.bun.sha256 },
      chromium: manifest.finalBrowsers.chromium,
      chromium_directory_files: manifest.browserFiles,
    },
    targetSha256: digest(environment.FVOCI_LIBSQL_URL as string),
    configSha256: equalConfig(env, policy),
    logical,
  };
  const audit: Audit = {
    namespaces: [namespace],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    serverStarts: 1,
    observedFences: [],
    targetSha256: inputs.targetSha256,
  };
  return withProcesses(async (scope) => {
    const fresh = retainRoomHistory(
      await readFinalPrimary(scope, manifest, environment, policy),
      before,
    );
    assertRoomClosure(fresh, audit.targetSha256);
    require(deepEquals(before.dataset, fresh.dataset, true), "UI_FINAL_PREDECESSOR_CHANGED");
    const owner = await fixture(scope, manifest, "owner", env),
      member = await fixture(scope, manifest, "member", env);
    rememberActor(audit, owner, namespace);
    rememberActor(audit, member, namespace);
    const running = await hostedFinalServerStart(scope, manifest, env, directory);
    const serverIdentity = allocationIdentity(running.server);
    let original: unknown = null;
    const cleanup: string[] = [];
    let measurement: HostedCase["measurement"] | undefined, closure: Record_ | undefined;
    try {
      const logged = await requestPublic(running.base, "/api/v1/auth/login", {
        email: get(owner, "email"),
        password: "supersecret1",
      });
      require(logged.cookie.length > 0 &&
        get(logged.value, "userId") === get(owner, "userId"), "UI_FINAL_BOOTSTRAP_LOGIN_FAILED");
      const workspaceId = String(get(owner, "workspaceId"));
      const created = await requestPublic(
        running.base,
        "/api/v1/workspaces/" + workspaceId + "/documents",
        { commandId: randomUUID(), parentId: null, title: "Equal work" },
        logged.cookie,
      );
      const documentId = String(get(created.value, "id"));
      require(get(created.value, "workspaceId") === workspaceId &&
        get(created.value, "createdBy") ===
          get(owner, "userId"), "UI_FINAL_EQUAL_DOCUMENT_ALLOCATION_FAILED");
      (audit.documents as Record<string, string>)[uuidHex(documentId)] = uuidHex(workspaceId);
      const seed: HostedSeed = {
        namespace,
        owner: String(get(owner, "userId")),
        member: String(get(member, "userId")),
        workspace: workspaceId,
        document: documentId,
        logical,
      };
      const initialDocument = await fixture(scope, manifest, "observe", env, {
        workspaceId,
        documentIds: [documentId],
      });
      require(get(initialDocument, "rows", documentId, "text") === "" &&
        get(initialDocument, "rows", documentId, "tail") === 0 &&
        list(initialDocument, "rows", documentId, "receipts").length ===
          0, "UI_FINAL_EQUAL_INITIAL_BODY_CHANGED");
      const bindingPath = join(directory, "actor-binding.private.json");
      write(bindingPath, {
        schema: 1,
        backend: "libsql-remote",
        setupNeeded: false,
        namespace,
        ownerEmail: get(owner, "email"),
        memberEmail: get(member, "email"),
        workspaceSlug: namespace,
        source: inputs.source,
        tree: inputs.tree,
        schemaCurrent: true,
        commit: "confirmed",
        lifecycleDrain: "confirmed",
        leases: 0,
        baselineSha256: valueDigest(fresh.dataset),
        owner,
        equalWork: {
          schema: 1,
          flow,
          workloadHash,
          targetSha256: inputs.targetSha256,
          ownerId: seed.owner,
          memberId: seed.member,
          workspaceId,
          documentId,
          documentDisplayId: get(created.value, "displayId"),
          initialBody: "",
        },
      });
      const samples = join(directory, "samples");
      const run: Command = async (args, options) => {
        require(options === undefined, "UI_FINAL_EQUAL_COMMAND_OPTIONS_REFUSED");
        const bytes = await docker.client(scope, args, 10);
        return {
          returncode: 0,
          stdout: new TextDecoder("utf-8", { fatal: true }).decode(bytes),
          stderr: "",
        };
      };
      measurement = await measureHosted(
        { server: running.server, base: running.base, flow, inputs, seed },
        samples,
        async () => {
          const report = await browser(
            scope,
            manifest,
            directory,
            {
              PLAYWRIGHT_BASE_URL: running.base,
              FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
              FVOCI_E2E_SELECTED_FLOW: flow,
              FVOCI_E2E_TURSO_NAMESPACE: namespace,
              FVOCI_E2E_TURSO_SOURCE: inputs.source,
              FVOCI_E2E_TURSO_TREE: inputs.tree,
              FVOCI_E2E_TURSO_TARGET_SHA256: inputs.targetSha256,
              FVOCI_E2E_TURSO_ACTOR_BINDING: bindingPath,
              FVOCI_E2E_EQUAL_WORK_PREPARED: "1",
              FVOCI_E2E_EQUAL_WORK_DIR: samples,
            },
            "v060-equal-work.spec.ts",
            undefined,
            checkout,
            "equal-work-playwright.config.ts",
          );
          return { result: { returncode: 0, stdout: "", stderr: "" }, report, allocation: seed };
        },
        run,
      );
      const native = await fixture(scope, manifest, "observe", env, {
        workspaceId,
        documentIds: [documentId],
      });
      require(get(native, "rows", documentId, "text") ===
        workload.edits.at(-1), "UI_FINAL_EQUAL_NATIVE_BODY_CHANGED");
      audit.observedFences.push(...(list(native, "rows", documentId, "roomFences") as unknown[][]));
    } catch (error) {
      original = error;
    }
    try {
      closure = await stop(scope, running.server, running.base, directory);
    } catch (error) {
      cleanup.push(failureCode(error));
    }
    try {
      closeSync(running.log);
    } catch {
      cleanup.push("UI_SERVER_LOG_CLOSE_FAILED");
    }
    if (original !== null || cleanup.length) {
      scope.io.output.err(
        diagnosticJson({
          originalFailure: original === null ? null : failureCode(original),
          cleanupErrors: cleanup,
        }),
      );
      if (original !== null) throw original as Error;
      require(false, "UI_FINAL_EQUAL_CLOSURE_FAILED");
    }
    require(measurement !== undefined &&
      closure !== undefined &&
      closure.containerAbsent === true &&
      closure.forced === false &&
      closure.serverExit === 0, "UI_FINAL_EQUAL_NORMAL_CLOSURE_REQUIRED");
    audit.servers.push(
      maintenanceReceipts(
        join(directory, "server.private.log"),
        serverIdentity,
        audit.targetSha256,
      ),
    );
    const after = await readFinalPrimary(scope, manifest, environment, policy);
    retainRoomHistory(
      after,
      fresh,
      roomRetirement(
        scope,
        join(directory, "server.private.log"),
        serverIdentity,
        audit.targetSha256,
        running.server.hostIdentity,
      ),
    );
    const documentRows = Object.values(record(after.dataset, "allocations", "documents")).filter(
      (row) => canonicalId(get(row, "self")) === measurement.seed.document,
    );
    require(documentRows.length === 1 &&
      deepEquals(
        get(documentRows[0], "workspaces"),
        [uuidHex(measurement.seed.workspace)],
        true,
      ), "UI_FINAL_EQUAL_NATIVE_ALLOCATION_CHANGED");
    audit.serial = { before: fresh, after, policy };
    assertPreserved(fresh.dataset, after.dataset, audit);
    return {
      observation: after,
      value: {
        measurement,
        evidence: {
          before: fresh.dataset,
          after: after.dataset,
          serialChecksBefore: fresh.checks,
          serialChecksAfter: after.checks,
          operationWitnesses: audit,
        },
        closure: {
          containerId: String(running.server.containerId),
          serverPid: running.server.child.pid,
          serverExit: closure.serverExit,
          forced: false,
          containerAbsent: true,
          portClosed: closure.portClosed === true,
          recordedIdentitiesRetired: closure.recordedIdentitiesRetired === true,
          cleanupErrors: cleanup,
        },
      },
    };
  });
}

/** The actual additional product4 or one explicitly selected boundary case. */
async function executeSelectedStage(
  manifest: Manifest,
  environment: Record<string, string>,
  policy: SerialPolicy,
  before: SerialObservation,
  suite: "products" | "boundary",
  engine: BrowserEngine,
  flow: "on" | "off",
): Promise<{ observation: SerialObservation; count: number; audit: Audit }> {
  const namespace = "tui-" + token(10),
    directory = join(root(), "final-" + suite + "-" + engine + "-" + flow);
  mkdirSync(directory, { mode: 0o700 });
  const storage = join(directory, "storage");
  mkdirSync(storage, { mode: 0o700 });
  const env = allocationEnvironment(manifest, environment, namespace, flow, storage);
  const audit: Audit = {
    namespaces: [namespace],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: digest(environment.FVOCI_LIBSQL_URL as string),
    serverStarts: 1,
    observedFences: [],
  };
  const observed = await withProcesses(async (scope) => {
    const fresh = retainRoomHistory(
      await readFinalPrimary(scope, manifest, environment, policy),
      before,
    );
    assertRoomClosure(fresh, audit.targetSha256);
    require(deepEquals(fresh.dataset, before.dataset, true), "UI_FINAL_PREDECESSOR_CHANGED");
    const owner = await fixture(scope, manifest, "owner", env),
      member = await fixture(scope, manifest, "member", env);
    rememberActor(audit, owner, namespace);
    rememberActor(audit, member, namespace);
    const running = await start(scope, manifest, env, directory, false),
      serverIdentity = allocationIdentity(running.server);
    let original: unknown = null;
    const cleanup: string[] = [];
    let cases: ReturnType<typeof reportCases> | undefined;
    const writes: SelectedSyncWrite[] = [];
    const targets: {
      workspaceId: string;
      documentId: string;
      receipts: SelectedNativeReceipt[];
    }[] = [];
    try {
      const logged = await requestPublic(running.base, "/api/v1/auth/login", {
        email: get(owner, "email"),
        password: "supersecret1",
      });
      require(get(logged.value, "userId") === get(owner, "userId") &&
        logged.cookie.length > 0, "UI_FINAL_BOOTSTRAP_LOGIN_FAILED");
      const outsider = await inviteFinalActor(
        running.base,
        String(get(owner, "workspaceId")),
        logged.cookie,
        namespace + "-outsider@example.invalid",
        "Aa1!" + token(16),
        "member",
        "outsider",
      );
      audit.actors[uuidHex(outsider.userId)] = outsider.email;
      const binding: FinalBinding = {
        schema: 1,
        backend: "libsql-remote",
        source: manifest.sourceInputs.source,
        tree: manifest.sourceInputs.tree,
        browser: engine,
        flow,
        suite,
        namespace,
        workspaceId: String(get(owner, "workspaceId")),
        workspaceSlug: namespace,
        setupNeeded: false,
        schemaCurrent: true,
        commit: "confirmed",
        lifecycleDrain: "confirmed",
        leases: 0,
        owner: {
          userId: String(get(owner, "userId")),
          email: String(get(owner, "email")),
          password: "supersecret1",
        },
        member: {
          userId: String(get(member, "userId")),
          email: String(get(member, "email")),
          password: "memberpass1",
        },
        outsider: { userId: outsider.userId, email: outsider.email, password: outsider.password },
      };
      const bindingPath = join(directory, "selected-actors.private.json");
      write(bindingPath, binding);
      const report = await browser(
        scope,
        manifest,
        directory,
        {
          PLAYWRIGHT_BASE_URL: running.base,
          FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
          FVOCI_E2E_SELECTED_FLOW: flow,
          FVOCI_E2E_SELECTED_SOURCE: binding.source,
          FVOCI_E2E_SELECTED_TREE: binding.tree,
          FVOCI_E2E_FINAL_BROWSER: engine,
          FVOCI_E2E_FINAL_SUITE: suite,
          FVOCI_E2E_TURSO_NAMESPACE: namespace,
          FVOCI_E2E_FINAL_ACTOR_BINDING: bindingPath,
        },
        FINAL_SPEC,
        undefined,
        checkout,
        "selected-final-playwright.config.ts",
        undefined,
        engine,
      );
      cases = reportCases(report, FINAL_SPEC, finalTitles(binding));
      require(get(report, "config", "metadata", "actorBindingSha256") === sha(bindingPath) &&
        get(report, "config", "metadata", "source") === binding.source &&
        get(report, "config", "metadata", "tree") === binding.tree &&
        get(report, "config", "metadata", "browser") === engine &&
        get(report, "config", "metadata", "selectedFlow") ===
          flow, "UI_FINAL_BROWSER_METADATA_CHANGED");
      const expected = cases.map((item) => {
        const wire = attachment([item], "selected-final-writes.json"),
          read = attachment([item], "selected-final-native-readback.json");
        require(get(wire, "source") === binding.source &&
          get(wire, "tree") === binding.tree &&
          get(wire, "browser") === engine &&
          get(wire, "flow") === flow &&
          get(read, "source") === binding.source &&
          get(read, "tree") === binding.tree &&
          get(read, "browser") === engine &&
          get(read, "flow") === flow, "UI_FINAL_BROWSER_EVIDENCE_CHANGED");
        for (const actor of list(wire, "actors")) {
          require(Object.hasOwn(
            audit.actors,
            uuidHex(get(actor, "actorId")),
          ), "UI_FINAL_FOREIGN_BROWSER_ACTOR");
          writes.push(...(list(actor, "writes") as SelectedSyncWrite[]));
        }
        return read;
      });
      const documents = new Map<string, Set<string>>();
      for (const read of expected) {
        const workspace = String(get(read, "workspaceId")),
          document = String(get(read, "targetId"));
        uuidHex(workspace);
        uuidHex(document);
        if (!documents.has(workspace)) documents.set(workspace, new Set());
        documents.get(workspace)?.add(document);
      }
      for (const write of writes) {
        if (!documents.has(write.workspaceId)) documents.set(write.workspaceId, new Set());
        documents.get(write.workspaceId)?.add(write.documentId);
      }
      for (const [workspaceId, ids] of documents) {
        const native = await fixture(scope, manifest, "observe", env, {
          workspaceId,
          documentIds: [...ids],
        });
        for (const documentId of ids) {
          const row = get(native, "rows", documentId);
          (audit.documents as Record<string, string>)[uuidHex(documentId)] = uuidHex(workspaceId);
          audit.observedFences.push(...(list(row, "roomFences") as unknown[][]));
          targets.push({
            workspaceId,
            documentId,
            receipts: list(row, "receipts") as SelectedNativeReceipt[],
          });
          for (const read of expected.filter(
            (r) => get(r, "workspaceId") === workspaceId && get(r, "targetId") === documentId,
          ))
            require(deepEquals(get(row, "content"), get(read, "content"), true) &&
              get(row, "text") === get(read, "text"), "UI_FINAL_NATIVE_BODY_CHANGED");
        }
      }
    } catch (error) {
      original = error;
    }
    try {
      await stop(scope, running.server, running.base, directory);
    } catch (error) {
      cleanup.push(failureCode(error));
    }
    try {
      closeSync(running.log);
    } catch {
      cleanup.push("UI_SERVER_LOG_CLOSE_FAILED");
    }
    if (original !== null || cleanup.length) {
      scope.io.output.err(
        diagnosticJson({
          originalFailure: original === null ? null : failureCode(original),
          cleanupErrors: cleanup,
        }),
      );
      if (original !== null) throw original as Error;
      require(false, "UI_FINAL_SELECTED_CLOSURE_FAILED");
    }
    audit.servers.push(
      maintenanceReceipts(
        join(directory, "server.private.log"),
        serverIdentity,
        audit.targetSha256,
      ),
    );
    const after = await readFinalPrimary(scope, manifest, environment, policy);
    retainRoomHistory(
      after,
      fresh,
      roomRetirement(
        scope,
        join(directory, "server.private.log"),
        serverIdentity,
        audit.targetSha256,
        running.server.hostIdentity,
      ),
    );
    audit.serial = { before: fresh, after, policy };
    if (flow === "on")
      await auditSelectedFinalNative(fresh.dataset, after.dataset, audit, targets, writes);
    else {
      require(writes.length === 0, "UI_FINAL_OFF_COLLAB_WRITE_REFUSED");
      assertPreserved(fresh.dataset, after.dataset, audit);
    }
    require(cases !== undefined, "UI_FINAL_SELECTED_REPORT_REQUIRED");
    return { observation: after, count: cases.length, audit };
  });
  return observed;
}

/** Three eager, distinct prepared owners; no SQL or fixture email relaxation. */
export async function prepareKeycloakOwners(
  manifest: Manifest,
  environment: Record<string, string>,
  policy: SerialPolicy,
  origin: SerialObservation,
): Promise<{
  groups: Record<KcMode, PreparedActors>;
  audit: Audit;
  observation: SerialObservation;
}> {
  const groups: Partial<Record<KcMode, PreparedActors>> = {};
  const audit: Audit = {
    namespaces: [],
    actors: {},
    workspaces: {},
    documents: {},
    servers: [],
    targetSha256: digest(environment.FVOCI_LIBSQL_URL as string),
    serverStarts: 0,
    observedFences: [],
  };
  let predecessor = origin;
  for (const mode of ["flows", "failures", "wrong-secret"] as const) {
    const namespace = "tui-" + token(10),
      directory = join(root(), "keycloak-prepare-" + mode);
    mkdirSync(directory, { mode: 0o700 });
    const storage = join(directory, "storage");
    mkdirSync(storage, { mode: 0o700 });
    const env = allocationEnvironment(manifest, environment, namespace, "off", storage);
    const phaseAudit: Audit = {
      namespaces: [namespace],
      actors: {},
      workspaces: {},
      documents: {},
      servers: [],
      targetSha256: audit.targetSha256,
      serverStarts: 1,
      observedFences: [],
    };
    const before = predecessor;
    const prepared = await withProcesses(async (scope) => {
      const fresh = retainRoomHistory(
        await readFinalPrimary(scope, manifest, environment, policy),
        before,
      );
      assertRoomClosure(fresh, audit.targetSha256);
      require(deepEquals(fresh.dataset, before.dataset, true), "UI_FINAL_PREDECESSOR_CHANGED");
      const bootstrap = await fixture(scope, manifest, "owner", env);
      rememberActor(phaseAudit, bootstrap, namespace);
      const running = await start(scope, manifest, env, directory, false);
      const serverIdentity = allocationIdentity(running.server);
      let actors: PreparedActors | undefined;
      let original: unknown = null;
      const cleanup: string[] = [];
      try {
        const logged = await requestPublic(running.base, "/api/v1/auth/login", {
          email: namespace + "-owner@example.invalid",
          password: "supersecret1",
        });
        require(logged.cookie.length > 0 &&
          get(logged.value, "userId") ===
            get(bootstrap, "userId"), "UI_FINAL_BOOTSTRAP_LOGIN_FAILED");
        const email = namespace + "-kc-owner@example.com",
          password = "Aa1!" + token(16);
        const workspaceId = String(get(bootstrap, "workspaceId"));
        const owner = await inviteFinalActor(
          running.base,
          workspaceId,
          logged.cookie,
          email,
          password,
          "owner",
          mode,
        );
        const ownerId = owner.userId;
        const workspaces = await requestPublic(
          running.base,
          "/api/v1/me/workspaces",
          undefined,
          owner.cookie,
        );
        const selected = list(workspaces.value, "items").filter(
          (row) => get(row, "id") === workspaceId,
        );
        require(selected.length === 1 &&
          get(selected[0], "role") === "owner" &&
          get(selected[0], "slug") === namespace, "UI_FINAL_PREPARED_OWNER_MEMBERSHIP_FAILED");
        phaseAudit.actors[uuidHex(ownerId)] = email;
        actors = {
          owner: { id: ownerId, email, password },
          member: { email: namespace + "-kc-member@example.com", password: "Aa1!" + token(16) },
          workspace: { id: workspaceId, slug: namespace, name: String(get(selected[0], "name")) },
          users: Object.fromEntries(
            ["alice", "bob", "carol", "mallory", "erin", "tina"].map((key) => [
              key,
              { username: namespace + "-" + key, email: namespace + "-" + key + "@example.com" },
            ]),
          ) as PreparedActors["users"],
          mismatchEmail: namespace + "-mismatch@example.com",
          crossOriginEmail: namespace + "-cross@example.com",
        };
      } catch (error) {
        original = error;
      }
      try {
        await stop(scope, running.server, running.base, directory);
      } catch (error) {
        cleanup.push(failureCode(error));
      }
      try {
        closeSync(running.log);
      } catch {
        cleanup.push("UI_SERVER_LOG_CLOSE_FAILED");
      }
      if (original !== null || cleanup.length) {
        scope.io.output.err(
          diagnosticJson({
            originalFailure: original === null ? null : failureCode(original),
            cleanupErrors: cleanup,
          }),
        );
        if (original !== null) throw original as Error;
        require(false, "UI_FINAL_PREPARATION_CLOSURE_FAILED");
      }
      phaseAudit.servers.push(
        maintenanceReceipts(
          join(directory, "server.private.log"),
          serverIdentity,
          phaseAudit.targetSha256,
        ),
      );
      const after = await readFinalPrimary(scope, manifest, environment, policy);
      retainRoomHistory(
        after,
        fresh,
        roomRetirement(
          scope,
          join(directory, "server.private.log"),
          serverIdentity,
          phaseAudit.targetSha256,
          running.server.hostIdentity,
        ),
      );
      phaseAudit.serial = { before: fresh, after, policy };
      assertPreserved(fresh.dataset, after.dataset, phaseAudit);
      require(actors !== undefined &&
        list(after.checks, "users").some(
          (row) => get(row, "id") === actors.owner.id && get(row, "email") === actors.owner.email,
        ), "UI_FINAL_PREPARED_OWNER_NATIVE_REQUIRED");
      return { actors, after };
    });
    groups[mode] = prepared.actors;
    predecessor = prepared.after;
    audit.namespaces.push(namespace);
    Object.assign(audit.actors, phaseAudit.actors);
    Object.assign(audit.workspaces, phaseAudit.workspaces);
    audit.servers.push(...phaseAudit.servers);
    audit.serverStarts++;
  }
  return { groups: groups as Record<KcMode, PreparedActors>, audit, observation: predecessor };
}

/** Read the compiled source defaults, then explicitly use those same settings. */
export function finalPolicy(manifest: Manifest): SerialPolicy {
  const path = join(checkout, "src/config.rs");
  require(manifest.sourceInputs.files["src/config.rs"] ===
    sha(path), "UI_FINAL_POLICY_SOURCE_CHANGED");
  const source = readFileSync(path, "utf8");
  const read = (name: string) => {
    const match = new RegExp("pub const " + name + ": u32 = ([0-9]+);").exec(source);
    require(match !== null, "UI_FINAL_POLICY_SOURCE_REQUIRED");
    return Number(match[1]);
  };
  return {
    revisionKeep: read("DEFAULT_REVISION_KEEP"),
    revisionSnapshotIntervalHours: read("DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS"),
  };
}

/** One read-only native transaction, bound to the actual sealed helper and target. */
export async function readFinalPrimary(
  scope: Scope,
  manifest: Manifest,
  environment: Record<string, string>,
  policy: SerialPolicy,
  initial = false,
): Promise<SerialObservation> {
  currentBuild(lease.load);
  require(environment.REVISION_KEEP === String(policy.revisionKeep) &&
    environment.REVISION_SNAPSHOT_INTERVAL_HOURS ===
      String(policy.revisionSnapshotIntervalHours), "UI_FINAL_SERVER_POLICY_CHANGED");
  const before = BigInt(Date.now()) * 1000n;
  const native = await fixture(scope, manifest, "serial-observe", environment, { ...policy });
  const after = BigInt(Date.now()) * 1000n;
  const observation = validateSerialObservation(native, policy, initial);
  assertNativeClock(observation, before, after);
  return observation;
}

function finalEnvironment(policy: SerialPolicy): Record<string, string> {
  require(process.env.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE === "true" &&
    process.env.FVOCI_DATABASE_BACKEND === "libsql-remote", "UI_FINAL_HUMAN_GATE_REQUIRED");
  let keys: unknown;
  try {
    keys = JSON.parse(process.env.FVOCI_TEST_TURSO_ENCRYPTION_KEYS ?? "");
  } catch {
    require(false, "UI_ENCRYPTION_KEYRING_REFUSED");
  }
  require(isRecord(keys) &&
    Object.keys(keys).length === 1 &&
    typeof keys["turso-test"] === "string" &&
    /^[a-fA-F0-9]{64}$/.test(keys["turso-test"]), "UI_ENCRYPTION_KEYRING_REFUSED");
  return {
    FVOCI_LIBSQL_URL: get(process.env, "FVOCI_LIBSQL_URL") as string,
    FVOCI_LIBSQL_AUTH_TOKEN: get(process.env, "FVOCI_LIBSQL_AUTH_TOKEN") as string,
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
    PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: token(32) }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: JSON.stringify(keys),
    ENCRYPTION_ACTIVE_KEY_ID: "turso-test",
    REVISION_KEEP: String(policy.revisionKeep),
    REVISION_SNAPSHOT_INTERVAL_HOURS: String(policy.revisionSnapshotIntervalHours),
  };
}

/** A scope must finish normally before its accepted snapshot can be continued. */
function acceptedPhase(
  serial: SerialExecution,
  expected: SerialObservation,
  proofs: Audit[],
): SerialObservation {
  require(serial.accepted !== undefined &&
    deepEquals(serial.before, expected, true), "UI_FINAL_PHASE_NATIVE_AUDIT_REQUIRED");
  proofs.push(serial.accepted.audit);
  return serial.accepted.observation;
}

/** Guard-only entry: no CLI/JSON continuation, alternate grant or phase array. */
export async function consumeFinal(inputs: Record<string, unknown>): Promise<void> {
  require(inputs.phase === "ui-final" &&
    inputs.destructive === true &&
    process.env.GITHUB_REF === "refs/heads/main", "UI_FINAL_MAIN_DISPATCH_REQUIRED");
  const manifest = currentBuild(lease.load);
  require(inputs.ui_source_sha === manifest.sourceInputs.source, "UI_REVIEWED_SOURCE_REQUIRED");
  recheckFinalBrowsers(manifest);
  const policy = finalPolicy(manifest),
    environment = finalEnvironment(policy);
  const initial = await withProcesses((scope) =>
    readFinalPrimary(scope, manifest, environment, policy, true),
  );
  require(startupBlockers(initial.dataset).length === 0 &&
    inputs.ui_baseline_sha256 ===
      valueDigest(initial.dataset), "UI_CURRENT_DATASET_BINDING_REQUIRED");
  require(inputs.ui_target_sha256 ===
    digest(environment.FVOCI_LIBSQL_URL as string), "UI_CURRENT_TARGET_BINDING_REQUIRED");
  let predecessor = initial;
  const proofs: Audit[] = [];
  const fresh = async () => {
    const observed = await withProcesses((scope) =>
      readFinalPrimary(scope, manifest, environment, policy, predecessor === initial),
    );
    require(deepEquals(
      observed.dataset,
      predecessor.dataset,
      true,
    ), "UI_FINAL_PREDECESSOR_CHANGED");
    retainRoomHistory(observed, predecessor);
    assertRoomClosure(observed, digest(environment.FVOCI_LIBSQL_URL as string));
    return observed;
  };
  let serial: SerialExecution = { before: await fresh(), policy };
  const ui = await withProcesses((scope) =>
    executeUi(scope, manifest, serial.before.dataset, environment, { ...defaultSteps, serial }),
  );
  require(ui.uiResult === "PASS" &&
    ui.originalFailure === null &&
    ui.cleanupErrors.length === 0 &&
    deepEquals(ui.counts, { on: 1, restart: 1, off: 8 }), "UI_FINAL_UI_ACK_FAILED");
  predecessor = acceptedPhase(serial, serial.before, proofs);
  serial = { before: await fresh(), policy };
  const restored = await withProcesses((scope) =>
    executeRestore(scope, manifest, serial.before.dataset, environment, {
      ...defaultSteps,
      serial,
    }),
  );
  require(restored.result === "PASS" &&
    restored.originalFailure === null &&
    restored.cleanupErrors.length === 0 &&
    restored.auditErrors.length === 0 &&
    deepEquals(restored.counts, [1, 1, 1, 1, 1, 1]) &&
    restored.sourceUnchangedDuringDestination === "PASS", "UI_FINAL_RESTORE_FAILED");
  predecessor = acceptedPhase(serial, serial.before, proofs);
  serial = { before: await fresh(), policy };
  const auth = await withProcesses((scope) =>
    executeAuth(scope, manifest, serial.before.dataset, environment, {
      ...defaultAuthSteps,
      serial,
    }),
  );
  require(auth.uiResult === "PASS" &&
    auth.originalFailure === null &&
    auth.cleanupErrors.length === 0 &&
    deepEquals(auth.counts, { on: 3, off: 3 }) &&
    deepEquals(auth.auditErrors, []), "UI_FINAL_AUTH_FAILED");
  predecessor = acceptedPhase(serial, serial.before, proofs);
  const products = await executeSelectedStage(
    manifest,
    environment,
    policy,
    predecessor,
    "products",
    "chromium",
    "on",
  );
  require(products.count === 4, "UI_FINAL_PRODUCT_COUNTS_FAILED");
  predecessor = products.observation;
  proofs.push(products.audit);
  for (const engine of ["chromium", "firefox", "webkit"] as const)
    for (const flow of ["on", "off"] as const) {
      const boundary = await executeSelectedStage(
        manifest,
        environment,
        policy,
        predecessor,
        "boundary",
        engine,
        flow,
      );
      require(boundary.count === 1, "UI_FINAL_BOUNDARY_COUNTS_FAILED");
      predecessor = boundary.observation;
      proofs.push(boundary.audit);
    }
  const on = await executeEqualCase(manifest, environment, policy, predecessor, "on");
  const off = await executeEqualCase(manifest, environment, policy, on.observation, "off");
  const costs = await compareHostedPair(on.value, off.value, (context) => {
    currentBuild(lease.load);
    require(context.inputs.source === manifest.sourceInputs.source &&
      context.inputs.tree === manifest.sourceInputs.tree &&
      context.inputs.targetSha256 === digest(environment.FVOCI_LIBSQL_URL as string) &&
      context.inputs.image === manifest.finalRuntime?.imageId &&
      context.inputs.configSha256 === on.value.measurement.inputs.configSha256 &&
      deepEquals(
        context.inputs.native,
        Object.fromEntries(
          Object.entries(manifest.binaries).map(([name, binary]) => [name, binary.sha256]),
        ),
        true,
      ) &&
      deepEquals(context.inputs.assets, manifest.assets, true), "UI_FINAL_EQUAL_BINDING_CHANGED");
    const audit = context.operationWitnesses;
    require(audit.serial !== undefined &&
      deepEquals(audit.serial.policy, policy, true) &&
      deepEquals(audit.serial.before.dataset, context.before, true) &&
      deepEquals(audit.serial.before.checks, context.serialChecksBefore, true) &&
      deepEquals(audit.serial.after.dataset, context.after, true) &&
      deepEquals(
        audit.serial.after.checks,
        context.serialChecksAfter,
        true,
      ), "UI_FINAL_EQUAL_AUDIT_CHANGED");
    assertPreserved(context.before, context.after, audit);
  });
  write(join(root(), "final-equal.private.json"), costs);
  proofs.push(on.value.evidence.operationWitnesses, off.value.evidence.operationWitnesses);
  predecessor = off.observation;
  const keycloak = await executeFinalKeycloak(manifest, environment, policy, predecessor);
  proofs.push(keycloak.audit);
  predecessor = keycloak.observation;
  const final = await fresh();
  let previous = initial.dataset;
  for (const proof of proofs) {
    require(proof.serial !== undefined &&
      deepEquals(proof.serial.before.dataset, previous, true), "UI_FINAL_AUDIT_CHAIN_CHANGED");
    assertPreserved(proof.serial.before.dataset, proof.serial.after.dataset, proof);
    previous = proof.serial.after.dataset;
  }
  require(deepEquals(previous, final.dataset, true), "UI_FINAL_WHOLE_PRIMARY_CHANGED");
  require(deepEquals(currentBuild(lease.load), manifest, true), "UI_FINAL_INPUTS_CHANGED");
  const counts = {
    uiAck: 10,
    restore: 6,
    auth: 6,
    products: products.count,
    boundary: 6,
    equal: 2,
    keycloak: 16,
  };
  const total = Object.values(counts).reduce((sum, count) => sum + count, 0);
  require(total === 50 && proofs.length === 13, "UI_FINAL_ACTUAL50_REQUIRED");
  write(join(root(), "final-preservation.private.json"), {
    source: manifest.sourceInputs.source,
    tree: manifest.sourceInputs.tree,
    before: initial,
    after: final,
    proofs,
    counts,
    total,
    wholePrimary: "PASS",
    normalClosure: "PASS",
  });
  stdio.out(
    JSON.stringify({
      phase: "ui-final",
      result: "PASS",
      counts,
      total,
      wholePrimary: "PASS",
      normalClosure: "PASS",
      afterSha256: valueDigest(final.dataset),
    }),
  );
}
