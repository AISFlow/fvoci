// Test doubles for the UI consumer's process scope and children. Only tests
// import this module; production code never constructs these.
import { parseJson, write } from "../selected-backend-ci/io.ts";
import type { Output } from "./ui-common.ts";
import type {
  Allocation,
  Child,
  Entry,
  ProcRow,
  Scope,
  ScopeIo,
  SpawnOptions,
} from "./ui-processes.ts";

export class Captured implements Output {
  stdout: string[] = [];
  stderr: string[] = [];
  events: string[] = [];
  failOut = false;
  failErr = false;
  out(line: string) {
    this.events.push("diagnostic");
    if (this.failOut) throw new Error("PRIVATE_STDOUT");
    this.stdout.push(line);
  }
  err(line: string) {
    this.events.push("stderr");
    if (this.failErr) throw new Error("PRIVATE_STDERR_WRITE");
    this.stderr.push(line);
  }
  get all() {
    return this.stdout.join("\n") + "\n" + this.stderr.join("\n");
  }
}

/** A child that already exited with `code` and printed `stdout`. */
export function exitedChild(
  code: number | null,
  stdout: string | Uint8Array = "",
  pid = 4242,
): Child {
  const bytes = typeof stdout === "string" ? new TextEncoder().encode(stdout) : stdout;
  return {
    pid,
    exitCode: code,
    signalCode: null,
    exited: code === null ? new Promise(() => {}) : Promise.resolve(code),
    stdin: { write: () => 0, end: () => 0 },
    stdout: new Response(bytes).body,
    kill: () => {},
  } as unknown as Child;
}

export interface FakeScopeOptions {
  spawn?: (args: string[], label: string, options: SpawnOptions) => Child;
  finish?: (child: Child, normal?: boolean) => Promise<number> | number;
  closure?: () => boolean;
  retired?: (row: Pick<ProcRow, "pid" | "startTicks">) => boolean;
  write?: ScopeIo["write"];
  root?: () => string;
  output?: Output;
}

export function fakeScope(options: FakeScopeOptions = {}) {
  const calls = {
    spawn: [] as [string[], string, SpawnOptions][],
    finish: [] as [Child, boolean | undefined][],
  };
  const allocations: Allocation[] = [];
  const entries = new Map<string, Entry>();
  const scope: Scope = {
    io: {
      output: options.output ?? new Captured(),
      write: options.write ?? write,
      root: options.root ?? (() => "/nonexistent-root"),
    },
    allocations,
    entries,
    spawn(args, label, spawnOptions) {
      calls.spawn.push([args, label, spawnOptions]);
      if (!options.spawn) throw new Error("unexpected spawn");
      const child = options.spawn(args, label, spawnOptions);
      allocations.push({ process: child, label, closed: false, forced: false });
      return child;
    },
    async finish(child, normal) {
      calls.finish.push([child, normal]);
      const result = options.finish ? await options.finish(child, normal) : 0;
      const allocation = allocations.find((a) => a.process === child);
      if (allocation) allocation.closed = true;
      return result;
    },
    closure: options.closure ?? (() => true),
    retired: options.retired ?? (() => true),
    send() {},
    capture(row, label, allocation = null) {
      const key = String(row.pid) + ":" + row.startTicks;
      entries.set(key, { identity: row, label, allocation, pidfd: null, reaped: false });
      return key;
    },
  };
  return { scope, calls };
}

/** Values whose integer fields carry JSON source tokens, as parsed child output does. */
export const parsed = <T>(value: T): T => parseJson(JSON.stringify(value)) as T;
export const clone = <T>(value: T): T => structuredClone(value);

/** The present value; a missing fixture element fails the test instead of reading undefined. */
export function must<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("missing fixture value");
  return value;
}

/** The failure message of a promise that must reject. */
export async function failureOf(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
  throw new Error("expected a rejection");
}
