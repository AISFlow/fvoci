// The final consumer owns the fixed serial call path and private whole-primary
// observations. Standalone consumers retain their original admission rules.
import { deepEquals } from "bun";
import { closeSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
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
} from "./ui-audit.ts";
import {
  diagnosticJson,
  failureCode,
  get,
  isRecord,
  list,
  require,
  root,
  token,
  valueDigest,
  type Record_,
} from "./ui-common.ts";
import { lease } from "./ui-container.ts";
import {
  defaultSteps,
  defaultAuthSteps,
  executeUi,
  executeAuth,
  type SerialExecution,
  allocationEnvironment,
  allocationIdentity,
} from "./ui-flow.ts";
import { fixture, start, stop } from "./ui-native.ts";
import { withProcesses, type Scope } from "./ui-processes.ts";
import { currentBuild, recheckFinalBrowsers, type Manifest } from "./ui-record.ts";
import { executeRestore } from "./ui-restore.ts";
import type { KcMode, PreparedActors } from "../../apps/web/e2e-keycloak/config.ts";

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
      const fresh = await readFinalPrimary(scope, manifest, environment, policy);
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
        const invitation = await requestPublic(
          running.base,
          "/api/v1/workspaces/" + workspaceId + "/invitations",
          { email, role: "owner" },
          logged.cookie,
        );
        const accepted = new URL(String(get(invitation.value, "acceptUrl")), running.base);
        require(accepted.origin === running.base &&
          /^\/invite\/[a-zA-Z0-9_-]+$/.test(
            accepted.pathname,
          ), "UI_FINAL_INVITATION_TARGET_REFUSED");
        const inviteToken = accepted.pathname.slice("/invite/".length);
        const preview = await requestPublic(running.base, "/api/v1/invitations/" + inviteToken);
        const consents = list(preview.value, "requiredLegal").map((item) => ({
          kind: get(item, "kind"),
          version: get(item, "version"),
        }));
        const issued = await requestPublic(
          running.base,
          "/api/v1/invitations/" + inviteToken + "/accept",
          { email, password, givenName: "Keycloak", familyName: mode, consents },
        );
        require(issued.cookie.length > 0, "UI_FINAL_OWNER_INVITATION_FAILED");
        const owner = await requestPublic(running.base, "/api/v1/auth/login", { email, password });
        require(owner.cookie.length > 0 &&
          get(owner.value, "userId") ===
            get(issued.value, "userId"), "UI_FINAL_PREPARED_OWNER_LOGIN_FAILED");
        const ownerId = String(get(owner.value, "userId"));
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
function acceptedPhase(serial: SerialExecution, expected: SerialObservation): SerialObservation {
  require(serial.accepted !== undefined &&
    deepEquals(serial.before, expected, true), "UI_FINAL_PHASE_NATIVE_AUDIT_REQUIRED");
  return serial.accepted.observation;
}

/** Guard-only entry: no CLI/JSON continuation, alternate grant or phase array. */
export async function consumeFinal(inputs: Record<string, unknown>): Promise<void> {
  // Removed only when the ROOT-frozen consumer APIs are concretely connected.
  // Missing final calls must refuse before reading credentials or starting a child.
  require(false, "UI_FINAL_BROWSER_EQUAL_KEYCLOAK_CALLERS_REQUIRED");
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
  const fresh = async () => {
    const observed = await withProcesses((scope) =>
      readFinalPrimary(scope, manifest, environment, policy, predecessor === initial),
    );
    require(deepEquals(
      observed.dataset,
      predecessor.dataset,
      true,
    ), "UI_FINAL_PREDECESSOR_CHANGED");
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
  predecessor = acceptedPhase(serial, serial.before);
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
  predecessor = acceptedPhase(serial, serial.before);
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
  predecessor = acceptedPhase(serial, serial.before);
  // Remaining fixed consumers are connected only from ROOT-frozen APIs below.
  // This source must fail closed until all fixed calls are present.
  require(false, "UI_FINAL_BROWSER_EQUAL_KEYCLOAK_CALLERS_REQUIRED");
}
