// Bun / Node compatibility APIs, with only the existing FVOCI file policy.
import { spawn, spawnSync, which } from "bun";
import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import {
  constants,
  accessSync,
  closeSync,
  fchmodSync,
  openSync,
  readSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  lstatSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { constants as osConstants } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import process from "node:process";

export type Environment = Record<string, string | undefined>;
export const root = resolve(import.meta.dir, "../..");
export const uid = () => {
  assert.ok(process.getuid);
  return process.getuid();
};
export const gid = () => {
  assert.ok(process.getgid);
  return process.getgid();
};
export const groups = () => {
  assert.ok(process.getgroups);
  return process.getgroups().sort((a, b) => a - b);
};
// `expected` is chosen by the caller. Production entry points pass the fixed
// [1000, 1000] literal. Root is refused even when it owns the directory and
// even when `expected` names that same root.
export function assertHandoffActor(output: string, expected: readonly [number, number]): void {
  const actorUid = uid(),
    actorGid = gid(),
    handed = statSync(output);
  assert.notEqual(actorUid, 0, "refusing root");
  assert.ok(
    actorUid === expected[0] && actorGid === expected[1],
    "selected runtime actor must be the fixed handoff uid and gid",
  );
  assert.equal(handed.uid, actorUid);
  assert.equal(handed.gid, actorGid);
}
export function env(name: string, source: Environment = process.env): string {
  const value = source[name];
  assert.notEqual(value, undefined, "missing required environment name");
  return value as string;
}
export function digest(value: string | Uint8Array): string {
  return createHash("sha256").update(value).digest("hex");
}
// Same canonical digest the runner and the lane drivers apply to a recorded
// failure.
export function failureDigest(value: unknown): string {
  const keys = new Set<string>();
  JSON.stringify(value, (key, child: unknown) => {
    if (key) keys.add(key);
    return child;
  });
  return digest(JSON.stringify(value, [...keys].sort()));
}
export function sha(path: string): string {
  const fd = openSync(path, "r"),
    hash = createHash("sha256"),
    buffer = new Uint8Array(1048576);
  try {
    for (;;) {
      const n = readSync(fd, buffer);
      if (!n) break;
      hash.update(buffer.subarray(0, n));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest("hex");
}
const numberTokens = new WeakMap<object, Map<string, string>>();
export function read(path: string): unknown {
  return parseJson(readFileSync(path, "utf8"));
}
// Standard JSON.parse; the reviver keeps each number's source token for jsonInteger.
export function parseJson(text: string): unknown {
  const value: unknown = JSON.parse(
    text,
    function (this: object, key: string, item: unknown, context?: { source?: string }): unknown {
      if (typeof item === "number") {
        assert.ok(context?.source, "JSON number source is required");
        let tokens = numberTokens.get(this);
        if (!tokens) {
          tokens = new Map();
          numberTokens.set(this, tokens);
        }
        tokens.set(key, context.source);
      }
      return item;
    },
  );
  return value;
}
export function jsonInteger(record: object, key: string): boolean {
  const value: unknown = Reflect.get(record, key);
  const token = numberTokens.get(record)?.get(key);
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    token !== undefined &&
    /^-?(?:0|[1-9][0-9]*)$/.test(token)
  );
}
// Existing selected command policy; spawn, cancellation and reaping stay in Bun.
export function spawnSelectedCommand(
  command: string[],
  environment: Environment,
  stdout: number | "pipe",
  stderr: number | "pipe",
  signal: AbortSignal,
) {
  signal.throwIfAborted();
  return spawn(command, {
    cwd: root,
    env: environment,
    stdout,
    stderr,
    stdin: "inherit",
    signal,
    killSignal: "SIGKILL",
  });
}
// Long enough for a nested lane driver to finish its own cleanup and for the
// wrapper's EXIT trap to docker rm its container. A group ^C starts every
// nesting level's timer at once, so an outer wrapper is given a longer grace
// than the wrappers nested inside it, and the runner a longer one than any
// lane driver (runtime.ts driverInterruptGrace).
export const wrapperInterruptGrace = 30_000;
// Re-exec of a lane driver under Bun: the same executable, no .env autoload.
export const bunDriver = (driver: string) => [process.execPath, "--no-env-file", driver];
// Public preparation checkpoints of the SQLite lane driver: one fixed name per
// owned preparation step. The runner publishes only `checkpointPrefix + <one of
// these>`; never a stack frame, path, argv or message.
export const checkpointPrefix = "tools/selected-backend-ci/drivers/sqlite.ts#";
export const preparationSteps = [
  "container-create",
  "container-start",
  "runtime-abi",
  "copy-inputs",
  "own-inputs",
  "mode-executables",
  "runtime-ldd",
  "runtime-ldd-log",
  "runtime-ldd-dependencies",
  "mode-environment",
  "copy-static",
  "copied-owned-files",
  "copied-hashes",
  "copied-hashes-log",
  "copied-hashes-match",
  "network-mode",
  "network-attachments",
  "network-attachment-count",
  "network-attachment-id",
  "network-driver",
  "network-driver-log",
  "network-driver-host",
] as const;
export type PreparationStep = (typeof preparationSteps)[number];
export function publicCheckpoint(value: unknown): string | null {
  return typeof value === "string" &&
    value.startsWith(checkpointPrefix) &&
    (preparationSteps as readonly string[]).includes(value.slice(checkpointPrefix.length))
    ? value
    : null;
}
export const jsonText = (value: unknown) => JSON.stringify(value, null, 2) + "\n";
// This byte form is a consumer contract: the runner's restart grant and
// drivers/binding.ts hash it, and the drivers write their receipts in it.
// Two-space indent, ASCII escapes and a final LF.
export const sourceInputText = (value: unknown) =>
  jsonText(value).replace(
    /[\u007f-\uffff]/g,
    (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"),
  );
export function write(path: string, value: unknown): void {
  const fd = openSync(path, "wx", 0o600);
  try {
    fchmodSync(fd, 0o600);
    writeFileSync(fd, jsonText(value));
  } finally {
    closeSync(fd);
  }
}
export function below(path: string, parent: string): boolean {
  const part = relative(parent, path);
  return part === "" || (part !== ".." && !part.startsWith(".." + sep) && !isAbsolute(part));
}
// Python Path.resolve() (posixpath.realpath, strict=False, CPython 3.14):
// components in order; an existing symlink is replaced by its readlink
// target (absolute resets to "/", relative continues from the link's
// directory) and ".." applies to the resolved location. A missing component
// is kept as is (CARGO_TARGET_DIR before the first build, CARGO_HOME -> a
// not-yet-created directory), and a symlink loop keeps the looping link
// path, as non-strict realpath does. Python ignores every OSError there;
// this port ignores only absent() errors, so EACCES and the rest still fail
// closed (intentionally stricter).
export function resolved(path: string): string {
  const rest: (string | null)[] = path.split(sep).reverse();
  let count = rest.length;
  let current = path.startsWith(sep) ? sep : process.cwd();
  // Link path -> fully resolved target; null while its target is resolving.
  const seen = new Map<string, string | null>();
  while (count > 0) {
    const name = rest.pop() as string | null;
    if (name === null) {
      seen.set(rest.pop() as string, current);
      continue;
    }
    count -= 1;
    if (name === "" || name === ".") continue;
    if (name === "..") {
      current = current.slice(0, current.lastIndexOf(sep)) || sep;
      continue;
    }
    const next = current === sep ? sep + name : current + sep + name;
    let target: string;
    try {
      if (!lstatSync(next).isSymbolicLink()) {
        current = next;
        continue;
      }
      if (seen.has(next)) {
        const cached = seen.get(next);
        // A cached target is reused; an unresolved one is a loop.
        current = cached ?? next;
        continue;
      }
      target = readlinkSync(next);
    } catch (error) {
      if (!absent(error)) throw error;
      current = next;
      continue;
    }
    if (target.startsWith(sep)) current = sep;
    seen.set(next, null);
    rest.push(next, null);
    const parts = target.split(sep).reverse();
    rest.push(...parts);
    count += parts.length;
  }
  return current;
}
// The path (or its symlink target) does not exist as a file system object:
// missing, a non-directory used as one, or a symlink loop.
function absent(error: unknown): boolean {
  const code = (error as NodeJS.ErrnoException).code;
  return code === "ENOENT" || code === "ENOTDIR" || code === "ELOOP";
}
// Python Path.is_file(): false for a missing or dangling path. Python
// ignores every OSError; EACCES and other errors still fail closed here.
export function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch (error) {
    if (absent(error)) return false;
    throw error;
  }
}
export function physical(path: string): string {
  assert.ok(isAbsolute(path) && realpathSync(path) === path, "nonphysical path");
  return path;
}
export function accessible(path: string, mask = constants.R_OK): boolean {
  try {
    accessSync(path, mask);
    return true;
  } catch {
    return false;
  }
}
export function tool(name: string): string {
  const path = which(name);
  assert.ok(path, "missing required tool");
  return path;
}
// skipDangling mirrors Python rglob('*') + is_file() (build inputs); the
// default throws on a dangling link, as browser_inventory refuses one.
export function files(directory: string, skipDangling = false): string[] {
  const result: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) result.push(...files(path, skipDangling));
    else if (skipDangling ? isFile(path) : statSync(path).isFile()) result.push(path);
  }
  return result;
}
export function inventory(directory: string): Record<string, string> {
  return Object.fromEntries(files(directory).map((path) => [relative(directory, path), sha(path)]));
}
/** No TCP listener on `port` in this network namespace, from the kernel's own
 * socket tables. A loopback connect probe is not this proof: on WSL mirrored
 * networking it still reaches the mirrored listener for a few milliseconds
 * after the owned one closed. */
export function listenerAbsent(port: number): boolean {
  assert.ok(Number.isInteger(port) && port > 0 && port <= 65535, "listener port refused");
  const local = ":" + port.toString(16).toUpperCase().padStart(4, "0");
  for (const table of ["/proc/net/tcp", "/proc/net/tcp6"]) {
    let text: string;
    try {
      text = readFileSync(table, "utf8");
    } catch (error) {
      // Without the IPv6 table no IPv6 listener exists; the IPv4 table is required.
      if (table.endsWith("6") && (error as NodeJS.ErrnoException).code === "ENOENT") continue;
      throw error;
    }
    for (const line of text.split("\n").slice(1)) {
      const fields = line.trim().split(/\s+/);
      if (fields[3] === "0A" && fields[1]?.endsWith(local)) return false;
    }
  }
  return true;
}
export function observedExit(result: { exitCode: number; signalCode?: string }): number {
  if (result.signalCode) {
    const number = osConstants.signals[result.signalCode as keyof typeof osConstants.signals];
    assert.ok(number, "unknown process termination signal");
    return -number;
  }
  return result.exitCode;
}
export function call(args: string[], cwd = root): string {
  const command =
    args[0] === "git" ? ["git", "-c", "safe.directory=" + cwd, ...args.slice(1)] : args;
  const result = spawnSync(command, { cwd, stdout: "pipe", stderr: "pipe" });
  assert.equal(observedExit(result), 0, "required command failed; output withheld");
  return result.stdout.toString().trim();
}
export function ancestors(path: string): string[] {
  const result: string[] = [];
  for (let parent = dirname(path); ; parent = dirname(parent)) {
    result.push(parent);
    if (parent === dirname(parent)) break;
  }
  return result;
}
