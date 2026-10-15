// Fixed native read-only observations. Database credentials reach only this
// native child, never a browser or the normal FVOCI server's environment.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { deepEquals } from "bun";
import type { PreparedBinding, PreparedUserRow } from "./config";

/** Private native facts: complete rows are hashed, never exported as secret columns. */
export type PreparedSnapshot = {
  binding: PreparedBinding;
  users: PreparedUserRow[];
  links: LinkRow[];
  memberships: Array<{ workspaceId: string; userId: string; role: string; rowFingerprint: string }>;
  states: number;
  /** The maintained native whole-primary observation, passed intact to its auditor. */
  wholePrimary: Record<string, unknown>;
};
export type PreparedDelta = { emails: string[]; links: LinkRow[] };

/** Exact global equality; an allocated email never exempts another row from preservation. */
export function assertPreparedDelta(
  before: PreparedSnapshot,
  after: PreparedSnapshot,
  delta: PreparedDelta,
): void {
  const fail = () => {
    throw new Error("Keycloak prepared global delta differs");
  };
  if (
    !deepEquals(before.binding, after.binding, true) ||
    !Number.isSafeInteger(before.states) ||
    before.states < 0 ||
    after.states !== before.states ||
    new Set(delta.emails).size !== delta.emails.length
  )
    fail();
  for (const snapshot of [before, after]) {
    const rows = snapshot.users;
    const whole: unknown = snapshot.wholePrimary;
    if (
      !Array.isArray(rows) ||
      rows.length > 10000 ||
      new Set(rows.map((row) => row.id)).size !== rows.length ||
      new Set(rows.map((row) => row.email)).size !== rows.length ||
      rows.some(
        (row) =>
          !/^[a-f0-9-]{36}$/.test(row.id) ||
          typeof row.email !== "string" ||
          !/^[a-f0-9]{64}$/.test(row.rowFingerprint),
      )
    )
      fail();
    if (
      whole === null ||
      typeof whole !== "object" ||
      Array.isArray(whole) ||
      !Array.isArray(snapshot.memberships) ||
      snapshot.memberships.length > 10000 ||
      new Set(snapshot.memberships.map((row) => `${row.workspaceId}/${row.userId}`)).size !==
        snapshot.memberships.length ||
      snapshot.memberships.some(
        (row) =>
          !/^[a-f0-9-]{36}$/.test(row.workspaceId) ||
          !rows.some((user) => user.id === row.userId) ||
          typeof row.role !== "string" ||
          !row.role ||
          !/^[a-f0-9]{64}$/.test(row.rowFingerprint),
      ) ||
      !Array.isArray(snapshot.links) ||
      snapshot.links.length > 10000
    )
      fail();
  }
  if (after.users.length !== before.users.length + delta.emails.length) fail();
  for (const row of before.users) {
    if (!after.users.some((fresh) => deepEquals(row, fresh, true))) fail();
  }
  for (const row of before.memberships) {
    if (!after.memberships.some((fresh) => deepEquals(row, fresh, true))) fail();
  }
  const oldIds = new Set(before.users.map((row) => row.id));
  const added = after.users.filter((row) => !oldIds.has(row.id));
  if (
    !deepEquals(added.map((row) => row.email).sort(), [...delta.emails].sort(), true) ||
    delta.emails.some((email) => before.users.some((row) => row.email === email))
  )
    fail();
  const sorted = (rows: LinkRow[]) =>
    rows
      .map((row) => JSON.stringify([row.user, row.provider, row.subject, row.issuer, row.email]))
      .sort();
  if (!deepEquals(sorted(after.links), sorted([...before.links, ...delta.links]), true)) fail();
}

export type Observation =
  | { operation: "users" | "states" | "links" }
  | { operation: "user" | "sessions" | "invitation"; email: string }
  | { operation: "membership"; email: string; slug: string };

export type LinkRow = {
  user: string;
  provider: string;
  subject: string;
  issuer: string | null;
  email: string | null;
};
type Values = {
  users: number;
  states: number;
  links: LinkRow[];
  user: string | null;
  sessions: number;
  invitation: boolean | null;
  membership: string | null;
};
export function observeDatabase<K extends keyof Values>(
  input: Observation & { operation: K },
): Values[K] {
  try {
    const binary = process.env.FVOCI_KC_OBSERVER_BIN;
    const capsule = process.env.FVOCI_KC_OBSERVER_ENV;
    if (!binary || !capsule) throw new Error();
    const environment = JSON.parse(readFileSync(capsule, "utf8")) as Record<string, string>;
    const bytes = execFileSync(binary, ["oidc-observe"], {
      env: environment,
      input: JSON.stringify(input),
      encoding: "utf8",
      stdio: ["pipe", "pipe", "ignore"],
      timeout: 30_000,
      maxBuffer: 1024 * 1024,
    });
    const result = JSON.parse(bytes) as {
      value: Values[K];
      rollback: string;
      lifecycleDrain: string;
      leases: number;
    };
    if (
      result.rollback !== "confirmed" ||
      result.lifecycleDrain !== "confirmed" ||
      result.leases !== 0
    )
      throw new Error();
    return result.value;
  } catch {
    // exec errors quote argv, output and environment; none are diagnostic text.
    throw new Error("Keycloak native read-only observer failed");
  }
}
