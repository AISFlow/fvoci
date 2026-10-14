// Standalone privileged reader. ROOT copies this exact module and pinned Bun
// to a private directory; stdin is a fixed DTO, stdout contains booleans only.
// Never accepts a path, command, SQL, environment value or output destination.
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readlinkSync, realpathSync, statSync, writeSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";

/** Fixed system entry only; root-owned alternatives are followed without PATH. */
export function systemSudo(): string {
  let path = "/usr/bin/sudo";
  for (let depth = 0; depth < 8; depth++) {
    for (let parent = dirname(path); ; parent = dirname(parent)) {
      const facts = lstatSync(parent);
      if (
        !facts.isDirectory() ||
        realpathSync(parent) !== parent ||
        facts.uid !== 0 ||
        facts.gid !== 0 ||
        (facts.mode & 0o022) !== 0
      )
        throw new Error("WITNESS_SYSTEM_LAUNCHER_REFUSED");
      if (parent === "/") break;
    }
    const facts = lstatSync(path);
    if (facts.uid !== 0 || facts.gid !== 0) throw new Error("WITNESS_SYSTEM_LAUNCHER_REFUSED");
    if (facts.isSymbolicLink()) {
      path = resolve(dirname(path), readlinkSync(path));
      continue;
    }
    if (!facts.isFile() || (facts.mode & 0o022) !== 0 || realpathSync(path) !== path)
      throw new Error("WITNESS_SYSTEM_LAUNCHER_REFUSED");
    return path;
  }
  throw new Error("WITNESS_SYSTEM_LAUNCHER_REFUSED");
}

const ALLOWED = [
  "DATABASE_APP_URL",
  "FVOCI_DATABASE_BACKEND",
  "FVOCI_SQLITE_PATH",
  "FVOCI_LIBSQL_URL",
  "FVOCI_LIBSQL_AUTH_TOKEN",
  "SSL_CERT_FILE",
  "OIDC_GENERIC_ISSUER",
  "OIDC_GENERIC_CLIENT_ID",
  "OIDC_GENERIC_CLIENT_SECRET",
] as const;
const FORBIDDEN = [
  "DATABASE_URL",
  "FVOCI_MIGRATION_URL",
  "POSTGRES_PASSWORD",
  "FVOCI_APP_PASSWORD",
  "MEILI_MASTER_KEY",
  "FVOCI_MEILI_MASTER_KEY",
  "TEST_DATABASE_URL",
  "FVOCI_TEST_DATABASE_URL",
  "FVOCI_KC_OBSERVER_DATABASE_URL",
  "FVOCI_KC_OBSERVER_ENV",
  "KC_BOOTSTRAP_ADMIN_PASSWORD",
];
export interface WitnessInput {
  pid: number;
  startTicks: string;
  parentPid: number;
  parentStartTicks: string;
  executableSha256: string;
  environmentDigests: Record<string, string>;
}
interface Identity {
  pid: number;
  parentPid: number;
  startTicks: string;
}
export interface WitnessView {
  before: Identity;
  after: Identity;
  parentBefore: Identity;
  parentAfter: Identity;
  callerBound: boolean;
  uids: number[];
  gids: number[];
  executableName: string;
  executableSha256: string;
  executableAfterName: string;
  executableAfterSha256: string;
  environment: Record<string, string>;
}
export const digest = (value: string | Uint8Array) =>
  createHash("sha256").update(value).digest("hex");
const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const integer = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > 1;
const ticks = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9]{1,24}$/.test(value);
const hash = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
export function parseInput(value: unknown): WitnessInput {
  if (
    !isObject(value) ||
    Object.keys(value).sort().join(",") !==
      "environmentDigests,executableSha256,parentPid,parentStartTicks,pid,startTicks" ||
    !integer(value.pid) ||
    !integer(value.parentPid) ||
    value.pid === value.parentPid ||
    !ticks(value.startTicks) ||
    !ticks(value.parentStartTicks) ||
    !hash(value.executableSha256) ||
    !isObject(value.environmentDigests)
  )
    throw new Error("WITNESS_INPUT_REFUSED");
  const environment = value.environmentDigests;
  if (
    !Object.hasOwn(environment, "OIDC_GENERIC_CLIENT_SECRET") ||
    !Object.hasOwn(environment, "FVOCI_DATABASE_BACKEND") ||
    !Object.keys(environment).every(
      (key) => (ALLOWED as readonly string[]).includes(key) && hash(environment[key]),
    )
  )
    throw new Error("WITNESS_INPUT_REFUSED");
  const backend = environment.FVOCI_DATABASE_BACKEND;
  const needed =
    backend === digest("postgres")
      ? ["DATABASE_APP_URL"]
      : backend === digest("sqlite")
        ? ["FVOCI_SQLITE_PATH"]
        : backend === digest("libsql-remote")
          ? ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN", "SSL_CERT_FILE"]
          : [];
  if (needed.length === 0 || !needed.every((key) => Object.hasOwn(environment, key)))
    throw new Error("WITNESS_INPUT_REFUSED");
  return value as unknown as WitnessInput;
}

const correctActor = (view: Pick<WitnessView, "uids" | "gids">) =>
  view.uids.length === 4 &&
  view.gids.length === 4 &&
  [...view.uids, ...view.gids].every((id) => id === 1000);
export function beforeReadAllowed(
  input: WitnessInput,
  view: Pick<WitnessView, "uids" | "gids" | "executableName" | "executableSha256">,
): boolean {
  return (
    correctActor(view) &&
    view.executableName === "fvoci-server" &&
    view.executableSha256 === input.executableSha256
  );
}
export function evaluate(input: WitnessInput, view: WitnessView) {
  const same = (a: Identity, b: Identity) =>
    a.pid === b.pid && a.parentPid === b.parentPid && a.startTicks === b.startTicks;
  const identityStable =
    same(view.before, view.after) &&
    same(view.parentBefore, view.parentAfter) &&
    view.before.pid === input.pid &&
    view.before.startTicks === input.startTicks &&
    view.before.parentPid === input.parentPid &&
    view.parentBefore.pid === input.parentPid &&
    view.parentBefore.startTicks === input.parentStartTicks;
  const actorCorrect = correctActor(view);
  const executableMatches =
    view.executableName === "fvoci-server" &&
    view.executableAfterName === view.executableName &&
    view.executableSha256 === input.executableSha256 &&
    view.executableAfterSha256 === view.executableSha256;
  const forbiddenAbsent = !Object.keys(view.environment).some(
    (key) => FORBIDDEN.includes(key) || /^KC_E2E_|^FVOCI_TEST_|^FVOCI_KC_OBSERVER_/.test(key),
  );
  const appMatches = Object.entries(input.environmentDigests).every(
    ([key, expected]) =>
      Object.hasOwn(view.environment, key) && digest(view.environment[key] as string) === expected,
  );
  const facts = {
    callerBound: view.callerBound,
    identityStable,
    actorCorrect,
    executableMatches,
    forbiddenAbsent,
    appMatches,
  };
  return { accepted: Object.values(facts).every(Boolean), ...facts };
}

function identity(pid: number): Identity {
  const raw = readFileSync(`/proc/${String(pid)}/stat`, "utf8");
  const parts = raw
    .slice(raw.lastIndexOf(")") + 2)
    .trim()
    .split(/\s+/);
  const parentPid = Number(parts[1]),
    startTicks = parts[19];
  if (!Number.isSafeInteger(parentPid) || parentPid < 0 || !ticks(startTicks))
    throw new Error("WITNESS_PROCESS_REFUSED");
  return { pid, parentPid, startTicks };
}
const idsOf = (status: string, key: "Uid" | "Gid") =>
  (new RegExp(`^${key}:\\s*(.+)$`, "m").exec(status)?.[1] ?? "").trim().split(/\s+/).map(Number);
type CallerFrame = Identity &
  Pick<WitnessView, "uids" | "gids" | "executableName" | "executableSha256">;
export function callerFrameMatches(
  input: WitnessInput,
  frame: CallerFrame,
  pinnedBunSha256: string,
): boolean {
  return (
    frame.pid === input.parentPid &&
    frame.startTicks === input.parentStartTicks &&
    correctActor(frame) &&
    frame.executableName === "bun" &&
    frame.executableSha256 === pinnedBunSha256
  );
}
export function callerBound(input: WitnessInput): boolean {
  // Only our own bounded ancestor chain (sudo's monitor/exec processes), never
  // a scan of unrelated processes. The observed app must be this caller's child.
  const sudo = systemSudo(),
    sudoSha256 = digest(readFileSync(sudo));
  let pid = process.ppid;
  for (let depth = 0; depth < 8 && pid > 1; depth++) {
    const parent = identity(pid);
    const executable = `/proc/${String(pid)}/exe`;
    const path = readlinkSync(executable);
    const file = statSync(executable);
    if (
      path === sudo &&
      file.uid === 0 &&
      file.gid === 0 &&
      (file.mode & 0o022) === 0 &&
      digest(readFileSync(executable)) === sudoSha256
    ) {
      pid = parent.parentPid;
      continue;
    }
    // The first non-sudo frame must be the immediate unprivileged Bun caller.
    // Never continue to a higher ancestor that owns another worker's child.
    const status = readFileSync(`/proc/${String(pid)}/status`, "utf8");
    return callerFrameMatches(
      input,
      {
        ...parent,
        uids: idsOf(status, "Uid"),
        gids: idsOf(status, "Gid"),
        executableName: basename(path),
        executableSha256: digest(readFileSync(executable)),
      },
      digest(readFileSync(process.execPath)),
    );
  }
  return false;
}
function inspect(input: WitnessInput): WitnessView {
  if (!callerBound(input)) throw new Error("WITNESS_CALLER_REFUSED");
  const before = identity(input.pid),
    parentBefore = identity(input.parentPid);
  if (
    before.startTicks !== input.startTicks ||
    before.parentPid !== input.parentPid ||
    parentBefore.startTicks !== input.parentStartTicks ||
    !callerBound(input)
  )
    throw new Error("WITNESS_IDENTITY_REFUSED");
  const status = readFileSync(`/proc/${String(input.pid)}/status`, "utf8");
  const executableName = basename(readlinkSync(`/proc/${String(input.pid)}/exe`));
  const executableSha256 = digest(readFileSync(`/proc/${String(input.pid)}/exe`));
  const uids = idsOf(status, "Uid"),
    gids = idsOf(status, "Gid");
  if (!beforeReadAllowed(input, { uids, gids, executableName, executableSha256 }))
    throw new Error("WITNESS_ACTOR_OR_EXECUTABLE_REFUSED");
  const environment: Record<string, string> = {};
  const bytes = new TextDecoder("utf-8", { fatal: true }).decode(
    readFileSync(`/proc/${String(input.pid)}/environ`),
  );
  for (const line of bytes.split("\0").filter(Boolean)) {
    const at = line.indexOf("=");
    if (at <= 0 || Object.hasOwn(environment, line.slice(0, at)))
      throw new Error("WITNESS_ENVIRONMENT_REFUSED");
    environment[line.slice(0, at)] = line.slice(at + 1);
  }
  const executableAfterName = basename(readlinkSync(`/proc/${String(input.pid)}/exe`));
  const executableAfterSha256 = digest(readFileSync(`/proc/${String(input.pid)}/exe`));
  return {
    before,
    parentBefore,
    after: identity(input.pid),
    parentAfter: identity(input.parentPid),
    callerBound: callerBound(input),
    uids,
    gids,
    executableName,
    executableSha256,
    executableAfterName,
    executableAfterSha256,
    environment,
  };
}

export async function main(): Promise<number> {
  let accepted = false;
  let result: Record<string, boolean> = { accepted: false };
  try {
    if (process.geteuid?.() !== 0 || process.argv.length !== 2)
      throw new Error("WITNESS_CONTEXT_REFUSED");
    const reader = Bun.stdin.stream().getReader();
    const chunks: Uint8Array[] = [];
    let length = 0;
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      length += chunk.value.length;
      if (length > 8192) {
        await reader.cancel();
        throw new Error("WITNESS_INPUT_REFUSED");
      }
      chunks.push(chunk.value);
    }
    const input = parseInput(
      JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks))),
    );
    result = evaluate(input, inspect(input));
    accepted = result.accepted === true;
  } catch {
    /* Fixed boolean output only, including parse/read/permission failures. */
  }
  try {
    writeSync(1, JSON.stringify(result) + "\n");
  } catch {
    return 78;
  }
  return accepted ? 0 : 78;
}
if (import.meta.main) process.exitCode = await main();
