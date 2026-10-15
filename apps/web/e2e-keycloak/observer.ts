// Fixed read-only observations. Prepared runs keep native credentials and full
// datasets in the parent; fresh local runs retain their existing native child.
import { execFileSync } from "node:child_process";
import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  openSync,
  readFileSync,
  readSync,
  writeSync,
} from "node:fs";
import { fileURLToPath } from "node:url";
import { deepEquals } from "bun";
import { bunIsolation } from "../../../tools/turso/ui-common.ts";
import { now, pause, procIdentity, readable } from "../../../tools/turso/ui-processes.ts";
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
export type Values = {
  users: number;
  states: number;
  links: LinkRow[];
  user: string | null;
  sessions: number;
  invitation: boolean | null;
  membership: string | null;
};
export type PreparedRead =
  | Exclude<Observation, { operation: "membership" }>
  | { operation: "before-unlink"; email: string; identityEmail: string; subject: string };
export type PreparedReadRequest = {
  binding: PreparedBinding;
  ordinal: number;
  actor: string;
  input: PreparedRead;
  peer: { pid: number; startTicks: string };
};
export const PREPARED_REQUEST_LIMIT = 4096;
export const PREPARED_REPLY_LIMIT = 1024 * 1024;
const fixedKeys = (value: object, keys: string[]) =>
  deepEquals(Object.keys(value).sort(), keys.sort(), true);
const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown, maximum: number): value is string =>
  typeof value === "string" && value.length > 0 && value.length <= maximum;
const count = (value: unknown) => Number.isSafeInteger(value) && Number(value) >= 0;

/** The only data allowed back to Playwright: existing no-secret projections or ACK. */
export function preparedReadValue(input: PreparedRead, result: unknown): unknown {
  if (input.operation === "before-unlink") {
    if (!record(result) || !fixedKeys(result, ["ack"]) || result.ack !== true)
      throw new Error("Keycloak prepared checkpoint failed");
    return null;
  }
  if (
    !record(result) ||
    !fixedKeys(result, ["value", "rollback", "lifecycleDrain", "leases"]) ||
    result.rollback !== "confirmed" ||
    result.lifecycleDrain !== "confirmed" ||
    result.leases !== 0
  )
    throw new Error("Keycloak prepared native closure differs");
  const value = result.value;
  let valid = false;
  switch (input.operation) {
    case "users":
    case "states":
    case "sessions":
      valid = count(value);
      break;
    case "user":
      valid = value === null || (typeof value === "string" && /^[a-f0-9-]{36}$/.test(value));
      break;
    case "invitation":
      valid = value === null || typeof value === "boolean";
      break;
    case "links":
      valid =
        Array.isArray(value) &&
        value.length <= 10000 &&
        value.every(
          (row: unknown) =>
            record(row) &&
            fixedKeys(row, ["user", "provider", "subject", "issuer", "email"]) &&
            text(row.user, 254) &&
            text(row.provider, 64) &&
            text(row.subject, 512) &&
            (row.issuer === null || text(row.issuer, 2048)) &&
            (row.email === null || text(row.email, 254)),
        );
      break;
  }
  if (!valid || Buffer.byteLength(JSON.stringify(value)) > PREPARED_REPLY_LIMIT - 128)
    throw new Error("Keycloak prepared native projection differs");
  return value;
}

let ordinal = 0;
function preparedRead(input: PreparedRead): unknown {
  try {
    if (process.env.FVOCI_KC_OBSERVER_BIN || process.env.FVOCI_KC_OBSERVER_ENV) throw new Error();
    const binding = JSON.parse(process.env.FVOCI_KC_READ_BINDING ?? "") as PreparedBinding;
    const actor = "email" in input ? input.email : process.env.FVOCI_KC_READ_OWNER;
    const bytes = execFileSync(
      process.execPath,
      [...bunIsolation(), fileURLToPath(import.meta.url), "--prepared-read"],
      {
        env: Object.fromEntries(
          ["PATH", "LANG", "TZ", "TMPDIR", "FVOCI_KC_READ_REQUEST", "FVOCI_KC_READ_REPLY"].flatMap(
            (key) => (process.env[key] === undefined ? [] : [[key, process.env[key]]]),
          ),
        ),
        input: JSON.stringify({ binding, ordinal: ++ordinal, actor, input }),
        encoding: "utf8",
        stdio: ["pipe", "pipe", "ignore"],
        timeout: 30_000,
        maxBuffer: PREPARED_REPLY_LIMIT,
      },
    );
    const reply = JSON.parse(bytes) as { ordinal: number; value: unknown };
    if (!record(reply) || !fixedKeys(reply, ["ordinal", "value"]) || reply.ordinal !== ordinal)
      throw new Error();
    return preparedReadValue(
      input,
      input.operation === "before-unlink"
        ? { ack: reply.value === null }
        : { value: reply.value, rollback: "confirmed", lifecycleDrain: "confirmed", leases: 0 },
    );
  } catch {
    throw new Error("Keycloak prepared parent read failed");
  }
}

export function beforePreparedUnlink(email: string, identityEmail: string, subject: string): void {
  preparedRead({ operation: "before-unlink", email, identityEmail, subject });
}

/** One fixed synchronous caller's Bun child; no native binary, env capsule or dataset. */
async function preparedReadClient(): Promise<void> {
  const descriptors: number[] = [];
  try {
    const payload = readFileSync(0);
    if (payload.length > PREPARED_REQUEST_LIMIT - 128) throw new Error();
    const input: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payload));
    if (!record(input) || !fixedKeys(input, ["binding", "ordinal", "actor", "input"]))
      throw new Error();
    const peer = procIdentity(process.pid);
    const request = Buffer.from(
      JSON.stringify({ ...input, peer: { pid: peer.pid, startTicks: peer.startTicks } }) + "\n",
    );
    if (request.length > PREPARED_REQUEST_LIMIT) throw new Error();
    const paths = [process.env.FVOCI_KC_READ_REQUEST, process.env.FVOCI_KC_READ_REPLY];
    const infos = paths.map((path) => {
      if (!path) throw new Error();
      const info = lstatSync(path);
      if (!info.isFIFO() || (info.mode & 0o777) !== 0o600 || info.uid !== process.getuid?.())
        throw new Error();
      return info;
    });
    // Reader exists before the parent opens its response writer. The request is
    // <= PIPE_BUF and is written once; a second frame is never accepted.
    const replyFd = openSync(paths[1] as string, constants.O_RDONLY | constants.O_NONBLOCK);
    descriptors.push(replyFd);
    const requestFd = openSync(paths[0] as string, constants.O_WRONLY | constants.O_NONBLOCK);
    descriptors.push(requestFd);
    for (const [index, fd] of [requestFd, replyFd].entries()) {
      const info = infos[index],
        opened = fstatSync(fd);
      if (
        !info ||
        !opened.isFIFO() ||
        opened.dev !== info.dev ||
        opened.ino !== info.ino ||
        (opened.mode & 0o777) !== 0o600 ||
        opened.uid !== process.getuid?.()
      )
        throw new Error();
    }
    if (writeSync(requestFd, request) !== request.length) throw new Error();
    const deadline = now() + 30;
    const chunks: Buffer[] = [];
    let length = 0;
    for (;;) {
      if (now() >= deadline) throw new Error();
      if (!readable(replyFd)) {
        await pause(20);
        continue;
      }
      const chunk = Buffer.alloc(65536);
      const size = readSync(replyFd, chunk, 0, chunk.length, null);
      if (size === 0) throw new Error();
      length += size;
      if (length > PREPARED_REPLY_LIMIT) throw new Error();
      chunks.push(chunk.subarray(0, size));
      const bytes = Buffer.concat(chunks);
      const newline = bytes.indexOf(10);
      if (newline < 0) continue;
      if (newline !== bytes.length - 1) throw new Error();
      writeSync(1, bytes);
      return;
    }
  } finally {
    for (const fd of descriptors.reverse()) closeSync(fd);
  }
}

if (import.meta.main) {
  try {
    if (process.argv.length !== 3 || process.argv[2] !== "--prepared-read") throw new Error();
    void preparedReadClient().catch(() => {
      process.exitCode = 1;
    });
  } catch {
    process.exitCode = 1;
  }
}

export function observeDatabase<K extends keyof Values>(
  input: Observation & { operation: K },
): Values[K] {
  if (process.env.FVOCI_KC_READ_BINDING !== undefined) {
    const observation: Observation = input;
    if (observation.operation === "membership")
      throw new Error("Keycloak prepared membership uses the public API");
    return preparedRead(observation) as Values[K];
  }
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
