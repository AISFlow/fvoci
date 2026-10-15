// Pure verification of native observations, browser reports and the primary
// preservation audit. No process, network or file system access except the
// bounded server log read in maintenanceReceipts.
import { deepEquals } from "bun";
import { readFileSync, statSync } from "node:fs";
import { basename } from "node:path";
import { jsonInteger } from "../selected-backend-ci/io.ts";
import { pySplitlines } from "../web-e2e/compat.ts";
import { digest } from "../selected-backend-ci/io.ts";
import {
  canonical,
  decodeUtf8,
  get,
  isRecord,
  list,
  record,
  require,
  text,
  type Record_,
  valueDigest,
} from "./ui-common.ts";

export const ON = "workspace-wiki-selected-backend.spec.ts";
export const OFF = "workspace-off-selected-backend.spec.ts";
export const RESTORE = "v060-turso-restore.acceptance.ts";

export const AUDITED_COUNTERS = new Set([
  "event_sequence",
  "collab_fence_counter",
  "maintenance_job_claims",
]);
// Admission is intentionally narrower than general product support. Normal
// startup must not consume preexisting jobs or deliver preexisting events.
export const BACKGROUND_TABLES = [
  "documents",
  "tasks",
  "attachments",
  "attachment_object_cleanups",
  "revisions",
  "events",
  "outbox_consumers",
  "outbox_failures",
  "processed_events",
  "notifications",
  "notification_prefs",
  "ics_tokens",
  "magic_tokens",
  "github_deliveries",
  "github_install_states",
  "github_installations",
  "github_issue_links",
  "import_jobs",
  "import_deferred_events",
  "push_deliveries",
  "push_subscriptions",
  "webhook_deliveries",
  "webhooks",
] as const;
const RELAYS = ["notifications", "mail", "push", "webhooks", "github"];

const same = (a: unknown, b: unknown) => deepEquals(a, b, true);

/** Persist barrier request IDs and append operation IDs are independent. */
export function nativeReceiptsRetained(before: unknown[], after: unknown[]): boolean {
  return (
    before.length >= 2 && before.every((receipt) => after.some((fresh) => same(receipt, fresh)))
  );
}

/** One native witness bound to each complete singleton row fingerprint. */
function vapidInitialization(before: unknown, after: unknown): boolean {
  if (!isRecord(before) || !isRecord(after)) return false;
  const previous = before.instanceConfig,
    current = after.instanceConfig;
  if (
    !Array.isArray(previous) ||
    previous.length !== 1 ||
    !Array.isArray(current) ||
    current.length !== 1
  )
    return false;
  const old: unknown = previous[0],
    now: unknown = current[0];
  if (!isRecord(old) || !isRecord(now)) return false;
  const hash = (value: unknown): value is string =>
    typeof value === "string" && value.length === 64 && /^[a-f0-9]{64}$/.test(value);
  return (
    old.singleton === true &&
    now.singleton === true &&
    old.vapidBothNull === true &&
    old.vapidPublicPresent === false &&
    old.vapidPrivateSealed === false &&
    now.vapidBothNull === false &&
    now.vapidPublicPresent === true &&
    now.vapidPrivateSealed === true &&
    hash(old.rowSha256) &&
    hash(now.rowSha256) &&
    old.rowSha256 !== now.rowSha256 &&
    hash(old.stableSha256) &&
    old.stableSha256 === now.stableSha256 &&
    same(get(before, "fingerprints", "instance_config"), { [old.rowSha256]: 1 }) &&
    same(get(after, "fingerprints", "instance_config"), { [now.rowSha256]: 1 })
  );
}
const truthy = (value: unknown) =>
  Array.isArray(value)
    ? value.length > 0
    : isRecord(value)
      ? Object.keys(value).length > 0
      : Boolean(value);
const subset = (items: unknown[], set: Set<string>) =>
  items.every((item) => typeof item === "string" && set.has(item));

export interface Audit {
  namespaces: string[];
  actors: Record<string, string>;
  workspaces: Record<string, string>;
  documents?: Record<string, string>;
  servers: Record_[];
  targetSha256: string;
  serverStarts: number;
  observedFences: unknown[][];
  /** Members of dedicated auth allocations; only they may revoke sessions. */
  authActors?: string[];
  /** Exact phase actors and parent-held native snapshots, never browser claims. */
  keycloak?: { actors: string[]; beforeUnlink: SerialObservation[] };
  /** Private observations collected by the final caller, never CLI proof input. */
  serial?: {
    before: SerialObservation;
    after: SerialObservation;
    policy: SerialPolicy;
    keycloakGroups?: 1 | 2 | 3;
  };
}

export interface SerialPolicy {
  revisionKeep: number;
  revisionSnapshotIntervalHours: number;
}
export interface SerialObservation {
  dataset: Record_;
  checks: Record_;
}

/** Native SQL clock resolution is milliseconds; compare without JS rounding. */
export function assertNativeClock(
  observation: SerialObservation,
  before: bigint,
  after: bigint,
): void {
  const clock = decimal(observation.checks.clockMicros);
  require(before > 0n &&
    after >= before &&
    clock >= before - 999n &&
    clock <= after + 999n, "UI_FINAL_NATIVE_CLOCK_RELATION_REQUIRED");
}

const SERIAL_BACKGROUND = [
  "magic-expiry",
  "mfa-expiry",
  "oidc-expiry",
  "ics-expiry",
  "github-install-expiry",
  "withdrawn-users",
  "deleted-workspaces",
  "trashed-documents",
  "unfinished-uploads",
  "object-cleanup",
  "imports",
  "deferred-import-events",
  "outbox-failures",
  "push-deliveries",
  "pending-webhooks",
  "enabled-digests",
  "notification-read-retention",
  "notification-archived-retention",
  "processed-retention",
  "unknown-processed-consumer",
  "webhook-retention",
  "github-retention",
  "maintenance-owners",
  "document-room-owners",
  "task-room-owners",
  "automatic-revision-retention",
];
const decimal = (value: unknown): bigint => integer(["integer", value]);
const rowHash = (cells: unknown) => digest(JSON.stringify(cells));
const fingerprints = (rows: unknown[]) => {
  const result: Record<string, number> = {};
  for (const row of rows) {
    const hash = rowHash(row);
    result[hash] = (result[hash] ?? 0) + 1;
  }
  return result;
};

/** Match all current counter columns to their complete native row hashes. */
function serialCounterRows(dataset: Record_): void {
  for (const table of AUDITED_COUNTERS) {
    const rows = list(dataset, "operations", table);
    require(same(
      record(dataset, "fingerprints", table),
      fingerprints(rows),
    ), "UI_SERIAL_COUNTER_FINGERPRINT_CHANGED");
  }
}

/** The initial read may have no registered relays; later reads require all five. */
export function validateSerialObservation(
  value: unknown,
  policy: SerialPolicy,
  initial = false,
): SerialObservation {
  const dataset = record(value, "dataset"),
    checks = record(value, "checks");
  require(dataset.schema === 1 &&
    dataset.backend === "libsql-remote" &&
    dataset.schemaCurrent === true &&
    dataset.lineage === "fvoci-sqlite-060" &&
    list(dataset, "ledger").length === 12 &&
    dataset.startupHazards === 0 &&
    dataset.liveOutboxLeases === 0, "UI_SERIAL_CURRENT_SCHEMA_REQUIRED");
  let total = 0;
  const tables = record(dataset, "fingerprints");
  require(same(
    Object.keys(tables).sort(),
    Object.keys(record(dataset, "allocations")).sort(),
  ), "UI_SERIAL_TABLE_WITNESS_MISSING");
  for (const [table, hashes] of Object.entries(tables)) {
    let count = 0;
    for (const [hash, multiplicity] of Object.entries(record(hashes))) {
      require(/^[a-f0-9]{64}$/.test(hash) &&
        typeof multiplicity === "number" &&
        Number.isSafeInteger(multiplicity) &&
        multiplicity > 0 &&
        Object.hasOwn(
          record(dataset, "allocations", table),
          hash,
        ), "UI_SERIAL_ROW_WITNESS_REFUSED");
      count += multiplicity;
    }
    require(count <= 10000 &&
      same(
        Object.keys(record(hashes)).sort(),
        Object.keys(record(dataset, "allocations", table)).sort(),
      ), "UI_SERIAL_ROW_CAP_REFUSED");
    total += count;
  }
  require(total <= 100000 && dataset.rows === total, "UI_SERIAL_ROW_CAP_REFUSED");
  require(checks.schema === 1 &&
    checks.policyBinding === "required-from-sealed-server" &&
    same(checks.policy, policy) &&
    checks.policySha256 ===
      rowHash({
        revisionKeep: policy.revisionKeep,
        revisionSnapshotIntervalHours: policy.revisionSnapshotIntervalHours,
      }) &&
    Number.isSafeInteger(policy.revisionKeep) &&
    policy.revisionKeep > 0 &&
    policy.revisionKeep <= 4294967295 &&
    Number.isSafeInteger(policy.revisionSnapshotIntervalHours) &&
    policy.revisionSnapshotIntervalHours >= 0 &&
    policy.revisionSnapshotIntervalHours <= 4294967295, "UI_SERIAL_SERVER_POLICY_REQUIRED");
  const clock = decimal(checks.clockMicros),
    end = decimal(checks.horizonEndMicros);
  require(clock > 0n &&
    decimal(checks.coverageMicros) === 2400000000n &&
    end - clock === 2400000000n, "UI_SERIAL_CLOCK_HORIZON_REQUIRED");
  serialCounterRows(dataset);
  const head = decimal(checks.eventHead);
  const events = list(dataset, "operations", "events");
  require(head === singleton(get(dataset, "operations", "event_sequence")) &&
    head === (events.length ? integer(get(events.at(-1), 0)) : 0n), "UI_SERIAL_EVENT_HEAD_CHANGED");
  const facts = list(checks, "background");
  require(same(
    facts.map((fact) => get(fact, "kind")).sort(),
    [...SERIAL_BACKGROUND].sort(),
  ), "UI_SERIAL_BACKGROUND_COVERAGE_MISSING");
  for (const fact of facts)
    require(get(fact, "eligibility") === "clear" &&
      get(fact, "now") === 0 &&
      get(fact, "throughHorizon") === 0 &&
      [
        "eligible",
        "pending",
        "owner",
        "unknown",
        "conservative-blocker",
        "eligible-conditional-on-policy",
      ].includes(text(fact, "interpretation")), "UI_SERIAL_BACKGROUND_WORK_REFUSED");
  for (const candidate of list(checks, "revisionCandidates"))
    require(get(candidate, "dueNow") === false &&
      get(candidate, "dueThroughHorizon") === false, "UI_SERIAL_REVISION_DUE");
  for (const row of list(checks, "automaticRevisionRanks"))
    require(list(row).length === 7 &&
      integer(get(row, 6)) <= BigInt(policy.revisionKeep), "UI_SERIAL_REVISION_RETENTION_DUE");
  const owners = record(checks, "owners");
  require(same(
    Object.keys(owners).sort(),
    ["maintenance_job_claims", "collab_room_fences", "task_collab_room_fences"].sort(),
  ), "UI_SERIAL_OWNER_COVERAGE_MISSING");
  for (const [table, rows] of Object.entries(owners))
    require(same(rows, get(dataset, "operations", table)), "UI_SERIAL_OWNER_ROWS_CHANGED");
  for (const row of list(owners, "maintenance_job_claims"))
    require(same(get(row, 1), ["null"]) && same(get(row, 3), ["null"]), "UI_SERIAL_OWNER_REMAINS");
  require(list(owners, "collab_room_fences").length === 0 &&
    list(owners, "task_collab_room_fences").length === 0, "UI_SERIAL_OWNER_REMAINS");
  const relays = list(checks, "outbox");
  if (initial && relays.length === 0) {
    require(head === 0n &&
      startupBlockers(dataset).length === 0 &&
      checks.relayCompletion === "unknown-registry" &&
      same(get(dataset, "operations", "outbox_consumers"), []) &&
      same(
        get(dataset, "fingerprints", "outbox_consumers"),
        {},
      ), "UI_SERIAL_INITIAL_RELAY_REFUSED");
  } else {
    require(checks.relayCompletion === "complete" &&
      same(
        relays.map((relay) => text(relay, "consumer")).sort(),
        [...RELAYS].sort(),
      ), "UI_SERIAL_RELAY_REGISTRY_CHANGED");
    const rows = relays.map((value) => {
      const relay = record(value);
      const name = text(relay, "consumer"),
        seq = text(relay, "lastSeq"),
        updated = text(relay, "updatedAtMicros");
      const row = [["text", name], ["integer", seq], ["null"], ["null"], ["integer", updated]];
      require(decimal(seq) === head &&
        relay.leaseOwner === null &&
        relay.leaseUntilMicros === null &&
        decimal(updated) <= clock &&
        relay.immutableSha256 === rowHash([["text", name]]) &&
        relay.rowSha256 === rowHash(row), "UI_SERIAL_RELAY_FINISH_UNCONFIRMED");
      return row;
    });
    require(same(record(dataset, "fingerprints", "outbox_consumers"), fingerprints(rows)) &&
      same(
        list(dataset, "operations", "outbox_consumers"),
        rows.map((row) => row.slice(0, 4)),
      ), "UI_SERIAL_RELAY_FINGERPRINT_CHANGED");
  }
  for (const [field, table] of [
    ["users", "users"],
    ["memberships", "memberships"],
  ] as const) {
    const rows = list(checks, field);
    const hashes = Object.fromEntries(rows.map((row) => [text(row, "rowFingerprint"), 1]));
    require(rows.length === Object.keys(hashes).length &&
      same(hashes, record(dataset, "fingerprints", table)), "UI_SERIAL_ACTOR_FINGERPRINT_CHANGED");
  }
  const users = list(checks, "users");
  require(new Set(users.map((row) => text(row, "id"))).size === users.length &&
    users.length ===
      list(dataset, "operations", "users").length, "UI_SERIAL_ACTOR_IDENTITY_CHANGED");
  for (const user of users) {
    const id = uuidHex(get(user, "id")),
      email = text(user, "email");
    require(list(dataset, "operations", "users").filter(
      (row) => same(get(row, 0), ["blob", id]) && same(get(row, 1), ["text", email]),
    ).length === 1, "UI_SERIAL_ACTOR_IDENTITY_CHANGED");
  }
  const memberships = list(checks, "memberships");
  require(new Set(
    memberships.map((row) => canonical([get(row, "workspaceId"), get(row, "userId")])),
  ).size === memberships.length, "UI_SERIAL_MEMBERSHIP_IDENTITY_CHANGED");
  for (const membership of memberships) {
    const workspace = uuidHex(get(membership, "workspaceId")),
      user = uuidHex(get(membership, "userId"));
    const allocation = record(
      dataset,
      "allocations",
      "memberships",
      text(membership, "rowFingerprint"),
    );
    require(users.some((row) => uuidHex(get(row, "id")) === user) &&
      list(dataset, "operations", "workspaces").some((row) =>
        same(get(row, 0), ["blob", workspace]),
      ) &&
      same(get(allocation, "workspaces"), [workspace]) &&
      same(get(allocation, "actors"), [user]) &&
      ["owner", "admin", "member", "guest"].includes(
        text(membership, "role"),
      ), "UI_SERIAL_MEMBERSHIP_IDENTITY_CHANGED");
  }
  return { dataset, checks };
}

function serialProgress(before: unknown, after: unknown, audit: Audit): void {
  const serial = audit.serial;
  require(serial !== undefined, "UI_SERIAL_OBSERVATION_REQUIRED");
  validateSerialObservation(
    serial.before,
    serial.policy,
    list(serial.before.checks, "outbox").length === 0,
  );
  validateSerialObservation(serial.after, serial.policy);
  require(valueDigest(before) === valueDigest(serial.before.dataset) &&
    valueDigest(after) === valueDigest(serial.after.dataset), "UI_SERIAL_DATASET_CHANGED");
  require(decimal(serial.after.checks.clockMicros) >=
    decimal(serial.before.checks.clockMicros), "UI_SERIAL_CLOCK_REVERSED");
  for (const old of list(serial.before.checks, "outbox")) {
    const fresh = list(serial.after.checks, "outbox").find(
      (row) => get(row, "consumer") === get(old, "consumer"),
    );
    require(get(fresh, "immutableSha256") === get(old, "immutableSha256") &&
      decimal(get(fresh, "updatedAtMicros")) >=
        decimal(get(old, "updatedAtMicros")), "UI_SERIAL_RELAY_IMMUTABLE_CHANGED");
  }
}

export function startupBlockers(baseline: unknown): string[] {
  const blocked: string[] = BACKGROUND_TABLES.filter((table) =>
    truthy(get(baseline, "fingerprints", table)),
  );
  if (get(baseline, "startupHazards") !== 0 || get(baseline, "liveOutboxLeases") !== 0)
    blocked.push("existing-owner-or-deletion");
  return blocked;
}

export function integer(cell: unknown): bigint {
  require(Array.isArray(cell) &&
    cell.length === 2 &&
    cell[0] === "integer" &&
    typeof cell[1] === "string" &&
    /^(?:0|[1-9][0-9]*)$/.test(cell[1]), "UI_COUNTER_TYPE_REFUSED");
  const value = BigInt(cell[1]);
  require(value <= 9223372036854775807n, "UI_COUNTER_RANGE_REFUSED");
  return value;
}

export function singleton(rows: unknown): bigint {
  require(Array.isArray(rows) &&
    rows.length === 1 &&
    Array.isArray(rows[0]) &&
    rows[0].length === 2 &&
    integer(rows[0][0]) === 1n, "UI_COUNTER_KEY_CHANGED");
  return integer((rows[0] as unknown[])[1]);
}

const canonicalUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export function uuidHex(value: unknown): string {
  require(typeof value === "string", "UI_ALLOCATION_ID_REFUSED");
  // Any accepted UUID spelling that is not the canonical lowercase form is refused.
  const bare = value
    .replaceAll("urn:", "")
    .replaceAll("uuid:", "")
    .replace(/^\{+|\}+$/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(bare.replaceAll("-", ""))) throw new TypeError("not a UUID");
  require(canonicalUuid.test(value), "UI_ALLOCATION_ID_REFUSED");
  return value.replaceAll("-", "");
}

export function rememberActor(audit: Audit, actor: unknown, namespace: string): void {
  require(get(actor, "namespace") === namespace &&
    get(actor, "commit") === "confirmed" &&
    get(actor, "freshPrimaryReadback") === true &&
    get(actor, "lifecycleDrain") === "confirmed" &&
    get(actor, "leases") === 0, "UI_ACTOR_RECEIPT_FAILED");
  const actorId = uuidHex(get(actor, "userId")),
    workspaceId = uuidHex(get(actor, "workspaceId"));
  const email = get(actor, "email");
  require(email === namespace + "-owner@example.invalid" ||
    email === namespace + "-member@example.invalid", "UI_ACTOR_EMAIL_REFUSED");
  require((audit.actors[actorId] ?? email) === email &&
    (audit.workspaces[workspaceId] ?? namespace) === namespace, "UI_ALLOCATION_CHANGED");
  audit.actors[actorId] = email;
  audit.workspaces[workspaceId] = namespace;
}

const ids = (rows: unknown[]) => new Set(rows.map((row) => get(row, 0, 1)));
const setEquals = (a: Set<unknown>, b: Set<unknown>) =>
  a.size === b.size && [...a].every((item) => b.has(item));

// The native witness binds the exact producer tuple/payload to the whole row
// fingerprint. An exception still needs the actor or resource allocated by
// this UI run; unknown globals and system writes remain refused.
type ProjectionTargets = {
  documents: Map<string, string>;
  tasks: Map<string, string>;
  identityLinks: Map<string, string>;
  removedIdentityLinks: Set<string>;
};
function witnessedEventOperation(
  refs: unknown,
  audit: Audit,
  workspaces: Set<string>,
  targets: ProjectionTargets,
): unknown[] | null {
  if (!isRecord(refs) || !isRecord(refs.eventWitness)) return null;
  const witness = refs.eventWitness;
  const spaces = list(refs, "workspaces"),
    people = list(refs, "actors");
  // Session revocation producers of a dedicated auth allocation member only.
  // Logout has no stored target; its witness binds the actual actor instead.
  if (
    witness.class === "actor-logout" ||
    witness.class === "actor-password-change" ||
    witness.class === "actor-password-reset"
  ) {
    const logout = witness.class === "actor-logout";
    const subject = logout ? witness.actor : witness.target;
    return typeof subject === "string" &&
      /^[a-f0-9]{32}$/.test(subject) &&
      same(
        Object.keys(witness).sort(),
        logout ? ["actor", "class", "sequence"] : ["class", "sequence", "target"],
      ) &&
      integer(witness.sequence) > 0n &&
      spaces.length === 0 &&
      people.length === 1 &&
      people[0] === subject &&
      ((audit.authActors ?? []).includes(subject) ||
        (audit.serial?.keycloakGroups !== undefined &&
          witness.class === "actor-logout" &&
          (audit.keycloak?.actors ?? []).includes(subject))) &&
      Object.hasOwn(audit.actors, subject)
      ? [witness.sequence, ["null"], ["blob", subject]]
      : null;
  }
  const target = witness.target;
  if (typeof target !== "string" || !/^[a-f0-9]{32}$/.test(target)) return null;
  const sequence = witness.sequence;
  if (integer(sequence) <= 0n) return null;
  if (
    witness.class === "actor-login" &&
    spaces.length === 0 &&
    people.length === 1 &&
    people[0] === target &&
    Object.hasOwn(audit.actors, target)
  )
    return [sequence, ["null"], ["blob", target]];
  if (audit.serial?.keycloakGroups !== undefined && audit.keycloak !== undefined) {
    if (
      witness.class === "actor-oidc-login" &&
      same(Object.keys(witness).sort(), ["class", "sequence", "target"]) &&
      spaces.length === 0 &&
      same(people, [target]) &&
      audit.keycloak.actors.includes(target) &&
      Object.hasOwn(audit.actors, target)
    )
      return [sequence, ["null"], ["blob", target]];
    if (
      (witness.class === "actor-identity-linked" || witness.class === "actor-identity-unlinked") &&
      same(Object.keys(witness).sort(), ["actor", "class", "sequence", "target"]) &&
      spaces.length === 0 &&
      typeof witness.actor === "string" &&
      same(people, [witness.actor]) &&
      audit.keycloak.actors.includes(witness.actor) &&
      Object.hasOwn(audit.actors, witness.actor) &&
      targets.identityLinks.get(target) === witness.actor &&
      (witness.class === "actor-identity-linked" || targets.removedIdentityLinks.has(target))
    )
      return [sequence, ["null"], ["blob", witness.actor]];
  }
  if (
    (witness.class === "document-projection" || witness.class === "task-projection") &&
    spaces.length === 1 &&
    people.length === 0 &&
    typeof spaces[0] === "string" &&
    workspaces.has(spaces[0]) &&
    targets[witness.class === "document-projection" ? "documents" : "tasks"].get(target) ===
      spaces[0]
  )
    return [sequence, ["blob", spaces[0]], ["null"]];
  return null;
}

export function allocatedRows(
  before: unknown,
  after: unknown,
  audit: Audit,
): [Set<string>, Set<string>, ProjectionTargets] {
  const oldUsers = ids(list(before, "operations", "users"));
  const oldSpaces = ids(list(before, "operations", "workspaces"));
  const addedUsers = list(after, "operations", "users").filter(
    (row) => !oldUsers.has(get(row, 0, 1)),
  );
  require(setEquals(
    ids(addedUsers),
    new Set(Object.keys(audit.actors)),
  ), "UI_FOREIGN_USER_ADDITION");
  const personal = new Set<string>();
  for (const row of addedUsers) {
    require(Array.isArray(row) &&
      row.length === 3 &&
      get(row, 0, 0) === "blob" &&
      same(row[1], ["text", audit.actors[text(row, 0, 1)]]), "UI_ACTOR_ROWS_CHANGED");
    if (!same(row[2], ["null"])) {
      require(get(row, 2, 0) === "blob" &&
        /^[a-f0-9]{32}$/.test(text(row, 2, 1)), "UI_PERSONAL_ALLOCATION_REFUSED");
      personal.add(text(row, 2, 1));
    }
  }
  const workspaces = new Set([...Object.keys(audit.workspaces), ...personal]);
  require(![...workspaces].some((id) => oldSpaces.has(id)), "UI_PREEXISTING_WORKSPACE_ALLOCATION");
  const addedSpaces = list(after, "operations", "workspaces").filter(
    (row) => !oldSpaces.has(get(row, 0, 1)),
  );
  require(setEquals(ids(addedSpaces), workspaces), "UI_FOREIGN_WORKSPACE_ADDITION");
  for (const row of addedSpaces) {
    require(Array.isArray(row) &&
      row.length === 3 &&
      get(row, 0, 0) === "blob", "UI_WORKSPACE_ROWS_CHANGED");
    const id = text(row, 0, 1);
    if (personal.has(id))
      require(same(row[2], ["text", "personal"]), "UI_PERSONAL_ALLOCATION_REFUSED");
    else
      require(same(row[1], ["text", audit.workspaces[id]]) &&
        same(row[2], ["text", "team"]), "UI_WORKSPACE_ROWS_CHANGED");
  }
  const actors = new Set(Object.keys(audit.actors));
  const targets: ProjectionTargets = {
    documents: new Map(Object.entries(audit.documents ?? {})),
    tasks: new Map(),
    identityLinks: new Map(),
    removedIdentityLinks: new Set(),
  };
  if (audit.keycloak !== undefined) {
    require(audit.serial?.keycloakGroups !== undefined, "UI_KEYCLOAK_SERIAL_CONTEXT_REQUIRED");
    const oldLinks = record(before, "allocations", "identity_links");
    const oldIds = new Set(Object.values(oldLinks).map((row) => get(row, "self")));
    const snapshots = [after, ...audit.keycloak.beforeUnlink.map((snapshot) => snapshot.dataset)];
    const finalIds = new Set(
      Object.values(record(after, "allocations", "identity_links")).map((row) => get(row, "self")),
    );
    for (const snapshot of snapshots) {
      require(same(get(before, "ledger"), get(snapshot, "ledger")) &&
        same(
          get(before, "schemaSha256"),
          get(snapshot, "schemaSha256"),
        ), "UI_CURRENT_LEDGER_CHANGED");
      for (const [hash, row] of Object.entries(record(snapshot, "allocations", "identity_links"))) {
        if (Object.hasOwn(oldLinks, hash)) continue;
        const id = get(row, "self"),
          people = list(row, "actors");
        require(typeof id === "string" &&
          /^[a-f0-9]{32}$/.test(id) &&
          !oldIds.has(id) &&
          get(snapshot, "fingerprints", "identity_links", hash) === 1 &&
          people.length === 1 &&
          typeof people[0] === "string" &&
          actors.has(people[0]) &&
          audit.keycloak.actors.includes(people[0]), "UI_KEYCLOAK_LINK_ALLOCATION_REFUSED");
        const previous = targets.identityLinks.get(id);
        require(previous === undefined || previous === people[0], "UI_KEYCLOAK_LINK_ACTOR_CHANGED");
        targets.identityLinks.set(id, people[0]);
        if (snapshot !== after && !finalIds.has(id)) targets.removedIdentityLinks.add(id);
      }
    }
  }
  // A projection exception needs either the ON browser/native seed or a new
  // complete native resource row created by this run's allocated actor in its
  // allocated team/personal workspace. A preexisting or foreign resource is
  // never inferred from an event's workspace alone.
  for (const table of ["documents", "tasks"] as const) {
    const previous = record(before, "fingerprints", table);
    for (const [hash, count] of Object.entries(record(after, "fingerprints", table))) {
      if (Object.hasOwn(previous, hash)) continue;
      const refs = get(after, "allocations", table, hash);
      const id = get(refs, "self"),
        spaces = list(refs, "workspaces"),
        people = list(refs, "actors");
      require(count === 1 &&
        typeof id === "string" &&
        /^[a-f0-9]{32}$/.test(id) &&
        spaces.length === 1 &&
        typeof spaces[0] === "string" &&
        workspaces.has(spaces[0]) &&
        people.length > 0 &&
        subset(people, actors), "UI_FOREIGN_RESOURCE_ALLOCATION");
      const existing = targets[table].get(id);
      require(existing === undefined || existing === spaces[0], "UI_ALLOCATION_CHANGED");
      targets[table].set(id, spaces[0]);
    }
  }
  const events = new Set<string>();
  const beforeEvents = record(before, "fingerprints", "events");
  for (const [sha, refs] of Object.entries(record(after, "allocations", "events"))) {
    if (!Object.hasOwn(beforeEvents, sha)) {
      const spaces = list(refs, "workspaces"),
        people = list(refs, "actors");
      require((spaces.length > 0 &&
        subset(spaces, workspaces) &&
        people.length > 0 &&
        subset(people, actors)) ||
        witnessedEventOperation(refs, audit, workspaces, targets) !== null, "UI_FOREIGN_EVENT");
      events.add(get(refs, "self") as string);
    }
  }
  for (const [table, hashes] of Object.entries(record(after, "fingerprints"))) {
    if (AUDITED_COUNTERS.has(table)) continue;
    // serialProgress binds every current outbox column and both full row maps.
    if (audit.serial !== undefined && table === "outbox_consumers") continue;
    if (table === "instance_config" && vapidInitialization(before, after)) continue;
    const previous = record(before, "fingerprints", table);
    for (const [sha, count] of Object.entries(record(hashes))) {
      const old = Object.hasOwn(previous, sha) ? previous[sha] : 0;
      if (truthy(old)) {
        require(count === old, "UI_PREEXISTING_ROW_MULTIPLICITY_CHANGED");
        continue;
      }
      const refs = get(after, "allocations", table, sha);
      require(subset(list(refs, "workspaces"), workspaces) &&
        subset(list(refs, "actors"), actors) &&
        subset(list(refs, "events"), events), "UI_FOREIGN_ROW_ADDITION");
      if (table === "users")
        require(actors.has(get(refs, "self") as string), "UI_FOREIGN_USER_ADDITION");
      else if (table === "workspaces")
        require(workspaces.has(get(refs, "self") as string), "UI_FOREIGN_WORKSPACE_ADDITION");
      else if (table === "outbox_consumers")
        require(RELAYS.includes(get(refs, "consumer") as string), "UI_RELAY_REGISTRY_CHANGED");
      else
        require(truthy(get(refs, "workspaces")) ||
          truthy(get(refs, "actors")) ||
          truthy(get(refs, "events")), "UI_UNSCOPED_ROW_ADDITION");
    }
  }
  return [workspaces, actors, targets];
}

// One JSON object at the start of `value`; trailing text is ignored.
export function leadingObject(value: string): unknown {
  if (!value.startsWith("{")) throw new SyntaxError("expected a JSON object");
  let depth = 0,
    quoted = false,
    escaped = false;
  for (let index = 0; index < value.length; index++) {
    const c = value[index];
    if (quoted) {
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === '"') quoted = false;
    } else if (c === '"') quoted = true;
    else if (c === "{" || c === "[") depth++;
    else if (c === "}" || c === "]") {
      depth--;
      if (depth === 0) return JSON.parse(value.slice(0, index + 1));
    }
  }
  throw new SyntaxError("unterminated JSON object");
}

const RECEIPT_KEYS = ["generation", "key", "outcome", "ownerSha256", "pid", "schema"];
export function maintenanceReceipts(
  logpath: string,
  serverIdentity: { pid: number; startTicks: string },
  target: string,
) {
  const records: Record_[] = [];
  const marker = "FVOCI_E2E_MAINTENANCE_RECEIPT ";
  require(statSync(logpath).size <= 32 * 1024 * 1024, "UI_SERVER_LOG_CAP_REFUSED");
  for (const line of pySplitlines(decodeUtf8(readFileSync(logpath)))) {
    const at = line.indexOf(marker);
    if (at < 0) continue;
    const item = leadingObject(line.slice(at + marker.length));
    require(isRecord(item) &&
      same(Object.keys(item).sort(), RECEIPT_KEYS) &&
      item.schema === 1 &&
      item.pid === serverIdentity.pid &&
      (item.key === 1 || item.key === 8 || item.key === 9) &&
      typeof item.ownerSha256 === "string" &&
      /^[a-f0-9]{64}$/.test(item.ownerSha256), "UI_MAINTENANCE_RECEIPT_REFUSED");
    records.push(item);
  }
  require(records.length <= 30, "UI_MAINTENANCE_RECEIPT_CAP_REFUSED");
  return { identity: serverIdentity, targetSha256: target, receipts: records };
}

export function auditMaintenance(before: unknown[], after: unknown[], audit: Audit): void {
  const keys = [1n, 2n, 3n, 4n, 5n, 6n, 7n, 8n, 9n];
  require(same(
    before.map((row) => integer(get(row, 0))),
    keys,
  ) &&
    same(
      after.map((row) => integer(get(row, 0))),
      keys,
    ), "UI_MAINTENANCE_KEYS_CHANGED");
  const current = new Map(before.map((row) => [integer(get(row, 0)), integer(get(row, 2))]));
  const keycloakGroups = audit.serial?.keycloakGroups;
  require(audit.servers.length === audit.serverStarts &&
    (keycloakGroups === undefined
      ? [1, 2, 3].includes(audit.serverStarts)
      : [1, 2, 3].includes(keycloakGroups) &&
        audit.serverStarts === 3 + keycloakGroups), "UI_START_COUNT_REFUSED");
  const identitiesSeen = new Set<string>(),
    ownersSeen = new Set<unknown>();
  for (const server of audit.servers) {
    const pid = get(server, "identity", "pid"),
      ticks = text(server, "identity", "startTicks");
    const identityKey = JSON.stringify([pid, ticks]);
    require(!identitiesSeen.has(identityKey) &&
      /^[0-9]+$/.test(ticks) &&
      get(server, "targetSha256") === audit.targetSha256, "UI_FOREIGN_MAINTENANCE_PROCESS");
    identitiesSeen.add(identityKey);
    for (const key of [8, 9, 1]) {
      const records = list(server, "receipts").filter((r) => get(r, "key") === key);
      require(records.length === 3 &&
        same(
          records.map((r) => get(r, "outcome")),
          ["prepared", "acquired", "released"],
        ), "UI_MAINTENANCE_FINISH_UNCONFIRMED");
      const owner = get(records[0], "ownerSha256");
      require(!ownersSeen.has(owner) &&
        records.every((r) => get(r, "pid") === pid && get(r, "ownerSha256") === owner) &&
        get(records[0], "generation") === null, "UI_FOREIGN_MAINTENANCE_OWNER");
      ownersSeen.add(owner);
      const next = (current.get(BigInt(key)) as bigint) + 1n;
      const expected = String(next);
      require(get(records[1], "generation") === expected &&
        get(records[2], "generation") === expected, "UI_MAINTENANCE_GENERATION_UNEXPLAINED");
      current.set(BigInt(key), next);
    }
  }
  before.forEach((old, index) => {
    const updated = after[index];
    const key = integer(get(old, 0));
    require(Array.isArray(old) &&
      Array.isArray(updated) &&
      old.length === 4 &&
      updated.length === 4 &&
      same(old[1], ["null"]) &&
      same(updated[1], ["null"]) &&
      same(old[3], ["null"]) &&
      same(updated[3], ["null"]), "UI_MAINTENANCE_OWNER_REMAINS");
    require(integer(updated[2]) === current.get(key), "UI_MAINTENANCE_GENERATION_UNEXPLAINED");
  });
}

export function auditCounters(before: unknown, after: unknown, audit: Audit): void {
  const b = record(before, "operations"),
    a = record(after, "operations");
  require(get(before, "startupHazards") === 0 &&
    get(after, "startupHazards") === 0 &&
    get(after, "liveOutboxLeases") === 0, "UI_FOREIGN_OR_UNRELEASED_OWNER");
  const [workspaces, actors, targets] = allocatedRows(before, after, audit);
  const first = singleton(get(b, "event_sequence")),
    last = singleton(get(a, "event_sequence"));
  require(first <= last && last <= first + 10000n, "UI_EVENT_COUNTER_RESET");
  const addedEvents = list(a, "events").filter((row) => integer(get(row, 0)) > first);
  const expectedEvents: bigint[] = [];
  for (let n = first + 1n; n <= last; n++) expectedEvents.push(n);
  require(same(
    addedEvents.map((row) => integer(get(row, 0))),
    expectedEvents,
  ), "UI_EVENT_COUNTER_UNEXPLAINED");
  const witnessed = new Set(
    Object.entries(record(after, "allocations", "events"))
      .filter(([hash]) => !Object.hasOwn(record(before, "fingerprints", "events"), hash))
      .flatMap(([, refs]) => {
        const row = witnessedEventOperation(refs, audit, workspaces, targets);
        return row === null ? [] : [canonical(row)];
      }),
  );
  for (const row of addedEvents)
    require((Array.isArray(row) &&
      row.length === 3 &&
      get(row, 1, 0) === "blob" &&
      workspaces.has(get(row, 1, 1) as string) &&
      get(row, 2, 0) === "blob" &&
      actors.has(get(row, 2, 1) as string)) ||
      witnessed.has(canonical(row)), "UI_FOREIGN_EVENT");
  const firstFence = singleton(get(b, "collab_fence_counter")),
    lastFence = singleton(get(a, "collab_fence_counter"));
  require(firstFence <= lastFence && lastFence <= firstFence + 10000n, "UI_FENCE_COUNTER_RESET");
  const fences = new Set<bigint>();
  const observed = new Set(audit.observedFences.map((row) => canonical(row.slice(0, 4))));
  for (const row of audit.observedFences) {
    require(row.length === 5 &&
      same(row[0], ["blob", get(row, 0, 1)]) &&
      workspaces.has(get(row, 0, 1) as string) &&
      get(row, 2, 0) === "blob" &&
      /^[a-f0-9]{32}$/.test(text(row, 2, 1)), "UI_FOREIGN_ROOM_FENCE");
    const value = integer(row[3]);
    require(firstFence <= value && value < lastFence, "UI_FOREIGN_ROOM_FENCE");
    fences.add(value);
  }
  for (const name of ["collab_room_fences", "task_collab_room_fences"]) {
    const previous = list(b, name);
    for (const row of list(a, name))
      if (!previous.some((old) => same(old, row)))
        require(observed.has(canonical(list(row).slice(0, 4))), "UI_UNOBSERVED_ROOM_OWNER");
  }
  let complete = fences.size === Number(lastFence - firstFence);
  for (let n = firstFence; complete && n < lastFence; n++) complete = fences.has(n);
  require(complete, "UI_FENCE_COUNTER_UNOBSERVED");
  auditMaintenance(list(b, "maintenance_job_claims"), list(a, "maintenance_job_claims"), audit);
  if (audit.serial !== undefined) {
    serialProgress(before, after, audit);
    return;
  }
  require(same(get(b, "outbox_consumers"), []), "UI_PREEXISTING_RELAY_REFUSED");
  const consumers = list(a, "outbox_consumers");
  const names = consumers.map((row) => canonical(get(row, 0))).sort();
  require(same(
    names,
    RELAYS.map((name) => canonical(["text", name])).sort(),
  ), "UI_RELAY_REGISTRY_CHANGED");
  for (const row of consumers) {
    const position = Array.isArray(row) && row.length === 4 ? integer(row[1]) : -1n;
    require(Array.isArray(row) &&
      row.length === 4 &&
      first <= position &&
      position <= last &&
      same(row[2], ["null"]) &&
      same(row[3], ["null"]), "UI_RELAY_FINISH_UNCONFIRMED");
  }
}

export function assertPreserved(before: unknown, after: unknown, audit?: Audit): void {
  if (audit?.serial !== undefined) serialProgress(before, after, audit);
  require(same(get(before, "ledger"), get(after, "ledger")) &&
    same(get(before, "schemaSha256"), get(after, "schemaSha256")) &&
    same(get(before, "lineage"), get(after, "lineage")), "UI_CURRENT_LEDGER_CHANGED");
  const old = record(before, "fingerprints"),
    current = record(after, "fingerprints");
  require(same(Object.keys(old).sort(), Object.keys(current).sort()), "UI_CURRENT_TABLES_CHANGED");
  for (const [table, rows] of Object.entries(old)) {
    if (audit !== undefined && AUDITED_COUNTERS.has(table)) continue;
    if (audit?.serial !== undefined && table === "outbox_consumers") continue;
    if (audit !== undefined && table === "instance_config" && vapidInitialization(before, after))
      continue;
    const now = record(current, table);
    for (const [sha, count] of Object.entries(record(rows)))
      require((Object.hasOwn(now, sha) ? now[sha] : 0) === count, "UI_PREEXISTING_ROW_CHANGED");
  }
  if (audit !== undefined) auditCounters(before, after, audit);
}

/** Eager prepared actors belong to the one Keycloak phase. Each completed group
 * must preserve all preceding business rows AND explain the entire phase's
 * operational progress using the actual cumulative server and actor witnesses. */
export function assertSerialGroupPreserved(
  origin: SerialObservation,
  previous: SerialObservation,
  after: SerialObservation,
  audit: Audit,
  policy: SerialPolicy,
  completedGroups: 1 | 2 | 3,
): void {
  audit.serial = { before: origin, after, policy, keycloakGroups: completedGroups };
  assertPreserved(origin.dataset, after.dataset, audit);
  serialProgress(previous.dataset, after.dataset, {
    ...audit,
    serial: { before: previous, after, policy },
  });
  require(same(get(previous.dataset, "ledger"), get(after.dataset, "ledger")) &&
    same(get(previous.dataset, "schemaSha256"), get(after.dataset, "schemaSha256")) &&
    same(
      Object.keys(record(previous.dataset, "fingerprints")).sort(),
      Object.keys(record(after.dataset, "fingerprints")).sort(),
    ), "UI_CURRENT_LEDGER_CHANGED");
  for (const [table, rows] of Object.entries(record(previous.dataset, "fingerprints"))) {
    // All current columns of these globals were checked above, with exact
    // cumulative maintenance/event/fence receipts. No business row is exempt.
    if (AUDITED_COUNTERS.has(table) || table === "outbox_consumers") continue;
    for (const [hash, count] of Object.entries(record(rows)))
      require(get(after.dataset, "fingerprints", table, hash) ===
        count, "UI_PREEXISTING_ROW_CHANGED");
  }
}

export type Case = [string, Record_];
export function reportCases(
  report: unknown,
  spec: string,
  titles: string[],
  backend: "libsql-remote" | "sqlite" = "libsql-remote",
): Case[] {
  require(backend === "libsql-remote" || spec === RESTORE, "UI_BROWSER_BACKEND_REFUSED");
  require(get(report, "config", "workers") === 1 &&
    same(get(report, "errors"), []) &&
    get(report, "config", "metadata", "selectedBackend") === backend &&
    get(report, "stats", "expected") === titles.length &&
    ["unexpected", "flaky", "skipped"].every(
      (k) => get(report, "stats", k) === 0,
    ), "UI_BROWSER_REPORT_FAILED");
  const cases: Case[] = [];
  const visit = (suites: unknown[]) => {
    for (const entry of suites) {
      const suite = record(entry);
      const specs = Object.hasOwn(suite, "specs") ? list(suite, "specs") : [];
      for (const item of specs) {
        require(basename(text(item, "file")) === spec &&
          get(item, "ok") === true &&
          list(item, "tests").length === 1, "UI_BROWSER_CASE_FAILED");
        const test = get(item, "tests", 0);
        require(get(test, "expectedStatus") === "passed" &&
          list(test, "results").length === 1, "UI_BROWSER_CASE_FAILED");
        const actual = record(test, "results", 0);
        require(get(actual, "status") === "passed" &&
          get(actual, "retry") === 0 &&
          same(get(actual, "errors"), []), "UI_BROWSER_CASE_FAILED");
        cases.push([get(item, "title") as string, actual]);
      }
      visit(Object.hasOwn(suite, "suites") ? list(suite, "suites") : []);
    }
  };
  visit(list(report, "suites"));
  require(same(
    cases.map(([title]) => title),
    titles,
  ), "UI_BROWSER_COUNT_MISMATCH");
  return cases;
}

export function attachment(cases: Case[], name: string): unknown {
  const entries = cases.flatMap(([, actual]) =>
    list(actual, "attachments").filter((a) => get(a, "name") === name),
  );
  require(entries.length === 1 &&
    get(entries[0], "contentType") === "application/json", "UI_ATTACHMENT_MISSING");
  const body = text(entries[0], "body");
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(body) || body.length % 4 !== 0)
    throw new TypeError("invalid base64 attachment");
  return JSON.parse(decodeUtf8(Buffer.from(body, "base64")));
}

const PHASES = new Set([
  "backend-contract",
  "schema-check",
  "schema-contract",
  "begin-read",
  "family-contract",
  "table-read",
  "table-conversion",
  "table-contract",
  "preservation-read",
  "row-limit",
  "row-hash",
  "row-allocation",
  "ui-read",
  "ui-conversion",
  "summary-read",
  "ledger-read",
  "ledger-conversion",
  "summary-conversion",
  "rollback",
]);
const CATEGORIES = new Set([
  "request",
  "database",
  "row-conversion",
  "protocol",
  "pool",
  "driver",
  "other",
  "libsql-hrana",
]);
const COMPARISON_KEYS = [
  "actualCount",
  "actualMismatchExpectedIndex",
  "actualOnlyCount",
  "actualOnlyUnderscoreCount",
  "expectedCount",
  "expectedOnlyCount",
  "firstMismatchIndex",
  "orderEqual",
  "setEqual",
];
const COUNT_KEYS = [
  "expectedCount",
  "actualCount",
  "actualOnlyCount",
  "expectedOnlyCount",
  "actualOnlyUnderscoreCount",
] as const;

// Native facts compare sorted ASCII-folded table identifiers; counts retain
// duplicate rows and indices refer to those folded lists. Integer fields need
// their JSON source token (io.parseJson) so 1.0 and true are not integers.
function comparisonQualified(c: unknown): c is Record<string, number | boolean | null> {
  if (!isRecord(c) || !same(Object.keys(c).sort(), COMPARISON_KEYS)) return false;
  if (
    COUNT_KEYS.some((k) => !jsonInteger(c, k) || (c[k] as number) < 0 || (c[k] as number) > 100001)
  )
    return false;
  const n = (k: string) => c[k] as number;
  if (typeof c.setEqual !== "boolean" || c.orderEqual !== false) return false;
  if (n("actualOnlyCount") > n("actualCount") || n("expectedOnlyCount") > n("expectedCount"))
    return false;
  if (n("actualOnlyUnderscoreCount") > n("actualOnlyCount")) return false;
  if (c.setEqual !== (n("actualOnlyCount") === 0 && n("expectedOnlyCount") === 0)) return false;
  const first = c.firstMismatchIndex,
    mismatch = c.actualMismatchExpectedIndex;
  if (first !== null) {
    if (!jsonInteger(c, "firstMismatchIndex")) return false;
    const f = first as number;
    if (f < 0 || f > Math.min(n("expectedCount"), n("actualCount"), 100000)) return false;
    if (
      n("expectedCount") === n("actualCount") &&
      n("actualCount") < 100001 &&
      f >= n("actualCount")
    )
      return false;
  }
  if (mismatch !== null) {
    if (!jsonInteger(c, "actualMismatchExpectedIndex")) return false;
    const m = mismatch as number;
    if (m < 0 || m >= Math.min(n("expectedCount"), 100001)) return false;
    if (first === null || (first as number) >= n("actualCount")) return false;
  }
  return true;
}

/** Project fixed native classifications only; never SDK text or row values. */
export function baselineFailureDiagnostic(value: Record_): Record_ {
  const outcome = isRecord(value.nativeOutcome) ? value.nativeOutcome : {};
  let status = "missing";
  const cause = (name: string, rollback = false) => {
    if (!Object.hasOwn(outcome, name) || outcome[name] === null) return null;
    const item = outcome[name];
    const keys = isRecord(item) ? Object.keys(item).sort() : [];
    if (
      !isRecord(item) ||
      !(
        same(keys, ["category", "phase"]) || same(keys, ["category", "phase", "tableComparison"])
      ) ||
      typeof item.phase !== "string" ||
      !PHASES.has(item.phase) ||
      (rollback && item.phase !== "rollback") ||
      typeof item.category !== "string" ||
      !CATEGORIES.has(item.category)
    ) {
      status = "refused";
      return null;
    }
    const result: Record_ = { phase: item.phase, category: item.category };
    if (Object.hasOwn(item, "tableComparison")) {
      const comparison = item.tableComparison;
      if (
        rollback ||
        item.phase !== "table-contract" ||
        item.category !== "protocol" ||
        !comparisonQualified(comparison)
      ) {
        status = "refused";
        return null;
      }
      result.tableComparison = { ...comparison };
    }
    if (status !== "refused") status = "qualified";
    return result;
  };
  const first = cause("baselineFailure");
  const rollback = cause("baselineRollbackFailure", true);
  const state = (container: Record_, name: string, allowed: string[]) => {
    const item = container[name];
    if (typeof item !== "string" || !allowed.includes(item)) {
      status = "refused";
      return null;
    }
    return item;
  };
  const native = {
    operation: state(outcome, "operation", ["failed", "confirmed"]),
    rollback: state(outcome, "rollback", ["not-attempted", "unknown", "confirmed"]),
    commit: state(outcome, "commit", ["not-attempted", "unknown", "confirmed"]),
  };
  const drain = state(value, "lifecycleDrain", ["confirmed", "unconfirmed"]);
  const drainOutcome = state(value, "drainOutcome", ["confirmed", "failed"]);
  return {
    originalFailure: "TURSO_UI_BASELINE_FAILED",
    diagnosticStatus: status,
    baselineFailure: first,
    baselineRollbackFailure: rollback,
    nativeOutcome: native,
    lifecycleDrain: drain,
    drainOutcome,
  };
}
