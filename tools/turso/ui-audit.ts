// Pure verification of native observations, browser reports and the primary
// preservation audit. No process, network or file system access except the
// bounded server log read in maintenanceReceipts.
import { deepEquals } from "bun";
import { readFileSync, statSync } from "node:fs";
import { basename } from "node:path";
import { jsonInteger } from "../selected-backend-ci/io.ts";
import { pySplitlines } from "../web-e2e/compat.ts";
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
} from "./ui-common.ts";

export const ON = "workspace-wiki-selected-backend.spec.ts";
export const OFF = "workspace-off-selected-backend.spec.ts";

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
  servers: Record_[];
  targetSha256: string;
  serverStarts: number;
  observedFences: unknown[][];
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

export function allocatedRows(
  before: unknown,
  after: unknown,
  audit: Audit,
): [Set<string>, Set<string>] {
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
  const events = new Set<string>();
  const beforeEvents = record(before, "fingerprints", "events");
  for (const [sha, refs] of Object.entries(record(after, "allocations", "events"))) {
    if (!Object.hasOwn(beforeEvents, sha)) {
      const spaces = list(refs, "workspaces"),
        people = list(refs, "actors");
      require(spaces.length > 0 &&
        subset(spaces, workspaces) &&
        people.length > 0 &&
        subset(people, actors), "UI_FOREIGN_EVENT");
      events.add(get(refs, "self") as string);
    }
  }
  for (const [table, hashes] of Object.entries(record(after, "fingerprints"))) {
    if (AUDITED_COUNTERS.has(table)) continue;
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
  return [workspaces, actors];
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
  require(audit.servers.length === audit.serverStarts &&
    [1, 2, 3].includes(audit.serverStarts), "UI_START_COUNT_REFUSED");
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
  const [workspaces, actors] = allocatedRows(before, after, audit);
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
  for (const row of addedEvents)
    require(Array.isArray(row) &&
      row.length === 3 &&
      get(row, 1, 0) === "blob" &&
      workspaces.has(get(row, 1, 1) as string) &&
      get(row, 2, 0) === "blob" &&
      actors.has(get(row, 2, 1) as string), "UI_FOREIGN_EVENT");
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
  require(same(get(before, "ledger"), get(after, "ledger")) &&
    same(get(before, "schemaSha256"), get(after, "schemaSha256")) &&
    same(get(before, "lineage"), get(after, "lineage")), "UI_CURRENT_LEDGER_CHANGED");
  const old = record(before, "fingerprints"),
    current = record(after, "fingerprints");
  require(same(Object.keys(old).sort(), Object.keys(current).sort()), "UI_CURRENT_TABLES_CHANGED");
  for (const [table, rows] of Object.entries(old)) {
    if (audit !== undefined && AUDITED_COUNTERS.has(table)) continue;
    const now = record(current, table);
    for (const [sha, count] of Object.entries(record(rows)))
      require((Object.hasOwn(now, sha) ? now[sha] : 0) === count, "UI_PREEXISTING_ROW_CHANGED");
  }
  if (audit !== undefined) auditCounters(before, after, audit);
}

export type Case = [string, Record_];
export function reportCases(report: unknown, spec: string, titles: string[]): Case[] {
  require(get(report, "config", "workers") === 1 &&
    same(get(report, "errors"), []) &&
    get(report, "config", "metadata", "selectedBackend") === "libsql-remote" &&
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
