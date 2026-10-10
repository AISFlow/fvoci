// Bun 1.4.2 / Node compatibility APIs, with only the existing FVOCI file policy.
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
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { constants as osConstants } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import process from "node:process";

export type Environment = Record<string, string | undefined>;
export const root = resolve(import.meta.dir, "../..");
export const templates = join(root, "scripts/selected-backend-ci");
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
export const jsonText = (value: unknown) => JSON.stringify(value, null, 2) + "\n";
// This byte form is a consumer contract: restart_checkpoint/current_binding hash
// json.dumps(source_written, indent=2), including ASCII escapes and final LF.
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
export function files(directory: string): string[] {
  const result: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) result.push(...files(path));
    else if (statSync(path).isFile()) result.push(path);
  }
  return result;
}
export function inventory(directory: string): Record<string, string> {
  return Object.fromEntries(files(directory).map((path) => [relative(directory, path), sha(path)]));
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
