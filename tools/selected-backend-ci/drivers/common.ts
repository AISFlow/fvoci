// Lane-driver primitives shared by the selected backend drivers: owned
// commands, Docker process identities, the first-failure packet and the
// cleanup ledger. Receipts keep the field names the runner already reads.
import { deepEquals, sleepSync, spawn, which, type Subprocess } from "bun";
import { dlopen, ptr } from "bun:ffi";
import { strict as assert } from "node:assert";
import {
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  statSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { get } from "node:http";
import { connect } from "node:net";
import { join, relative, resolve } from "node:path";
import process from "node:process";
import { digest, files, jsonInteger, observedExit, root, sha, sourceInputText } from "../io.ts";
import type { Environment } from "../io.ts";
import type { Inputs } from "../types.ts";

export type Receipt = Record<string, unknown>;
export type Json = Record<string, unknown>;

// datetime.now(timezone.utc).isoformat() at millisecond precision.
export const now = () => new Date().toISOString().replace("Z", "+00:00");

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
export const decode = (bytes: Uint8Array) => utf8.decode(bytes);
export const readText = (path: string) => decode(readFileSync(path));

export function named(name: string, message: string): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}
// The runner publishes this type name for an owned command failure.
export const runtimeError = (message: string) => named("RuntimeError", message);
export function errorFacts(error: unknown): { type: string; message: string } {
  return error instanceof Error
    ? { type: error.name, message: error.message.slice(0, 4096) }
    : { type: "NonError", message: String(error).slice(0, 4096) };
}

// SIGINT reaches the driver as an interrupt of its body: the owned child in
// flight is killed and reaped, the first failure is recorded and every cleanup
// step still runs. A later SIGINT aborts only the cleanup command in flight,
// which records that step as a cleanup error.
// What each in-flight owned command does on SIGINT.
const inFlight = new Set<() => void>();
let interrupted = false,
  cleanupDepth = 0;
export const interruptError = () => named("KeyboardInterrupt", "selected driver interrupted");
export function trapInterrupts(): void {
  process.on("SIGINT", () => {
    interrupted = true;
    for (const interrupt of inFlight) interrupt();
  });
}
export function checkInterrupt(): void {
  if (interrupted && cleanupDepth === 0) throw interruptError();
}
export async function cleanupScope<T>(operation: () => Promise<T>): Promise<T> {
  cleanupDepth += 1;
  try {
    return await operation();
  } finally {
    cleanupDepth -= 1;
  }
}

export interface Completed {
  returncode: number;
  stdout: string;
  stderr: string;
}
export interface CommandOptions {
  log?: string;
  required?: boolean;
  env?: Environment;
  cwd?: string;
  input?: string;
  // The child is a fixture wrapper whose EXIT trap removes what it owns. On
  // SIGINT it is not killed at once: it gets SIGINT (as from a process-group
  // ^C) and is waited for, so its cleanup runs; the interrupt is then raised
  // as for any other owned command (Python: KeyboardInterrupt out of
  // subprocess.run). The wait is bounded by interruptGrace: a wrapper that
  // ignores SIGINT, or whose INT trap waits for a foreground command that only
  // a process-group ^C would interrupt (a SIGINT sent to this driver alone),
  // is then SIGKILLed with every descendant. Its EXIT trap does not run, so
  // the fixture-closure check reports what it left behind.
  waitOnInterrupt?: boolean;
  // Milliseconds; defaults to wrapperInterruptGrace.
  interruptGrace?: number;
}
// Long enough for a nested lane driver to finish its own cleanup and for the
// wrapper's EXIT trap to docker rm its container. A group ^C starts every
// nesting level's timer at once, so an outer wrapper is given a longer grace
// than the wrappers nested inside it.
export const wrapperInterruptGrace = 30_000;

// ESRCH and ENOENT: the process or thread has already gone.
const gone = (error: unknown) =>
  ["ESRCH", "ENOENT"].includes(String((error as NodeJS.ErrnoException).code));
const signalled = (pid: number, signal: NodeJS.Signals) => {
  try {
    process.kill(pid, signal);
  } catch (error) {
    if (!gone(error)) throw error;
  }
};
const listed = <T>(read: () => T[]): T[] => {
  try {
    return read();
  } catch (error) {
    if (!gone(error)) throw error;
    return [];
  }
};
const childPids = (pid: number) =>
  listed(() => readdirSync(`/proc/${String(pid)}/task`)).flatMap((task) =>
    listed(() =>
      readText(`/proc/${String(pid)}/task/${task}/children`)
        .split(" ")
        .filter(Boolean)
        .map(integer),
    ),
  );
// /proc/<pid>[/task/<tid>]/stat, or undefined once the process has gone.
const statText = (path: string): string | undefined => {
  try {
    return readFileSync(path, "latin1");
  } catch (error) {
    if (!gone(error)) throw error;
    return undefined;
  }
};
// The state letter follows "(comm) ".
const state = (stat: string) => stat.charAt(stat.lastIndexOf(")") + 2);
// The start time tells a process from a later one reusing its pid.
const startedAt = (pid: number) => {
  const stat = statText(`/proc/${String(pid)}/stat`);
  return stat === undefined ? undefined : startTicks(stat);
};
const until = (done: () => boolean, deadline: number) => {
  while (!done()) {
    if (performance.now() >= deadline) return false;
    sleepSync(1);
  }
  return true;
};
// How long the whole walk may wait for SIGSTOPped processes to stop, and the
// SIGKILLed ones to die, before that wait is given up. Bounds on a kernel
// state change, not timing allowances.
const stopBound = 500;
const deathBound = 1000;
// SIGKILL a process and all its descendants, and return once every one of
// them is dead. /proc children is only whole while its process cannot fork
// or reap, so each process is SIGSTOPped and the walk waits until all its
// threads have stopped before reading it; one that does not stop within the
// bound is read anyway (its pending signal already fails its forks) and then
// SIGKILLed. Passes repeat until one adds no pid. A stopped parent cannot
// reap, so a listed pid is not reused before its SIGKILL; the start time is
// still checked before each signal. A step that fails does not stop the
// others; the first failure is thrown after the rest, and a process left
// alive is a failure. A child that exits between being listed and being
// stopped hands its own children to the nearest child subreaper. The walk is
// whole only when the root is one, as a waited wrapper is (execSubreaper):
// the stopped root then holds those orphans for the next pass. Under any
// other root they go to init, out of reach of this walk.
export function killTree(pid: number): void {
  const tree = new Map([[pid, startedAt(pid)]]);
  const order = [pid];
  const stopped = new Set<number>();
  let failure: Error | undefined;
  const attempt = (step: () => void) => {
    try {
      step();
    } catch (error) {
      failure ??= error instanceof Error ? error : new Error(String(error));
    }
  };
  const same = (member: number) => {
    const ticks = tree.get(member);
    return ticks !== undefined && startedAt(member) === ticks;
  };
  const signal = (member: number, name: NodeJS.Signals) => {
    if (same(member)) signalled(member, name);
  };
  // Every thread has stopped (T, or t under a tracer) or died, or the
  // process has gone.
  const halted = (member: number) =>
    !same(member) ||
    listed(() => readdirSync(`/proc/${String(member)}/task`)).every((task) =>
      "TtZX".includes(state(statText(`/proc/${String(member)}/task/${task}/stat`) ?? "() X")),
    );
  const stopDeadline = performance.now() + stopBound;
  const stop = (member: number) => {
    stopped.add(member);
    let done = false;
    attempt(() => {
      signal(member, "SIGSTOP");
      done = until(() => halted(member), stopDeadline);
    });
    return done;
  };
  for (let grew = true; grew;) {
    grew = false;
    for (let index = 0; index < order.length; index += 1) {
      const member = order[index] as number;
      const late = !stopped.has(member) && !stop(member);
      attempt(() => {
        for (const child of childPids(member))
          if (!tree.has(child)) {
            tree.set(child, startedAt(child));
            order.push(child);
            grew = true;
          }
      });
      if (late)
        attempt(() => {
          signal(member, "SIGKILL");
        });
    }
  }
  // Descendants first: their stopped parents cannot reap them meanwhile.
  for (const member of [...order].reverse())
    attempt(() => {
      signal(member, "SIGKILL");
    });
  const dead = (member: number) => {
    const stat = statText(`/proc/${String(member)}/stat`);
    return (
      stat === undefined || startTicks(stat) !== tree.get(member) || "ZX".includes(state(stat))
    );
  };
  const deathDeadline = performance.now() + deathBound;
  attempt(() => {
    const alive = order.filter((member) => !until(() => dead(member), deathDeadline));
    if (alive.length > 0)
      throw new Error(`owned process tree outlived SIGKILL: ${alive.join(" ")}`);
  });
  if (failure) throw failure;
}

// libc calls Bun does not expose. prctl, fcntl and syscall are variadic: every
// argument is passed as an unsigned long, as the Linux x86-64/aarch64 ABI does.
let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("libc.so.6", {
    prctl: { args: ["i32", "u64", "u64", "u64", "u64"], returns: "i32" },
    fcntl: { args: ["i32", "i32", "u64"], returns: "i32" },
    syscall: { args: ["i64", "i64", "i64", "i64", "i64"], returns: "i64" },
    pipe2: { args: ["ptr", "i32"], returns: "i32" },
    signal: { args: ["i32", "u64"], returns: "u64" },
    sigprocmask: { args: ["i32", "ptr", "ptr"], returns: "i32" },
    execve: { args: ["ptr", "ptr", "ptr"], returns: "i32" },
  }).symbols;
}
const native = () => (libc ??= openLibc());
const PR_SET_CHILD_SUBREAPER = 36;
const PR_GET_CHILD_SUBREAPER = 37;
const O_CLOEXEC = 0o2000000;
const F_GETFD = 1;
const F_SETFD = 2;
const FD_CLOEXEC = 1;
const SYS_CLOSE_RANGE = 436n;
const CLOSE_RANGE_CLOEXEC = 4n;
const SIG_SETMASK = 2;
const SIG_ERR = 0xffffffffffffffffn;
// The launcher writes "ok" to this fd just before its exec, and the name of
// a step that failed after it.
const statusFd = 3;
// A waited wrapper is launched as `bun common.ts <cwd> <args...>`: the
// launcher makes itself a child subreaper and confirms the flag, so an orphan
// of the wrapper tree is reparented onto the wrapper instead of init, then
// execs the wrapper in its own place. The pid, the process group and the flag
// survive the exec. The status fd is close-on-exec, so an exec that succeeds
// leaves "ok" alone in the pipe; a launcher that dies before it writes "ok"
// leaves nothing. Bun.spawn resets a child's ignored signals and its mask, so
// the launcher does so too, last of all; the signals Bun handles return to
// their default at the exec. The launcher starts in its own directory, away
// from any bunfig.toml or tsconfig.json of the wrapper's cwd.
function execSubreaper([cwd, executable, ...rest]: string[]): never {
  const fail = (step: string): never => {
    writeSync(statusFd, step);
    process.exit(1);
  };
  if (cwd === undefined || executable === undefined) return fail("exec");
  try {
    process.chdir(cwd);
  } catch {
    fail("exec");
  }
  // Bun.spawn searches the child's PATH or, when that is unset or empty, the
  // libc default (getconf PATH), never the driver's own PATH.
  const path = which(executable, { PATH: process.env.PATH || "/bin:/usr/bin" });
  if (path === null) return fail("exec");
  const strings: Buffer[] = [];
  const vector = (values: string[]) => {
    const pointers = new BigUint64Array(values.length + 1);
    values.forEach((value, index) => {
      const bytes = Buffer.from(value + "\0");
      strings.push(bytes);
      pointers[index] = BigInt(ptr(bytes));
    });
    return pointers;
  };
  const file = Buffer.from(path + "\0");
  const argv = vector([executable, ...rest]);
  const envp = vector(Object.entries(process.env).map(([key, value]) => `${key}=${String(value)}`));
  const ignored = BigInt(
    "0x" + (/^SigIgn:\s*([0-9a-f]+)$/m.exec(readText("/proc/self/status"))?.[1] ?? ""),
  );
  const c = native();
  const flag = new Int32Array(1);
  if (
    c.prctl(PR_SET_CHILD_SUBREAPER, 1n, 0n, 0n, 0n) !== 0 ||
    c.prctl(PR_GET_CHILD_SUBREAPER, BigInt(ptr(flag)), 0n, 0n, 0n) !== 0 ||
    flag[0] !== 1
  )
    fail("subreaper");
  // Bun opens its own fds close-on-exec; close_range (Linux 5.11) makes sure
  // of every fd above 2. The status fd must be.
  c.syscall(SYS_CLOSE_RANGE, 3n, 0xffffffffn, CLOSE_RANGE_CLOEXEC, 0n);
  if (
    c.fcntl(statusFd, F_SETFD, BigInt(FD_CLOEXEC)) !== 0 ||
    (c.fcntl(statusFd, F_GETFD, 0n) & FD_CLOEXEC) === 0
  )
    fail("cloexec");
  for (let signal = 1; signal <= 64; signal += 1)
    if ((ignored >> BigInt(signal - 1)) & 1n && c.signal(signal, 0n) === SIG_ERR) fail("signals");
  if (c.sigprocmask(SIG_SETMASK, ptr(new Uint8Array(128)), null) !== 0) fail("signals");
  writeSync(statusFd, "ok");
  c.execve(ptr(file), ptr(argv), ptr(envp));
  return fail("exec");
}
// The read end of a close-on-exec pipe whose write end is the launcher's
// status fd. Read once the wrapper has exited, when no process holds the
// write end.
function statusPipe(): [number, number] {
  const ends = new Int32Array(2);
  if (native().pipe2(ptr(ends), O_CLOEXEC) !== 0)
    throw runtimeError("owned wrapper status pipe failed");
  return [ends[0] as number, ends[1] as number];
}
function readStatus(fd: number): string {
  const chunk = Buffer.alloc(64);
  let text = "";
  for (let count; (count = readSync(fd, chunk)) > 0;) text += decode(chunk.subarray(0, count));
  return text;
}

export type Command = (args: string[], options?: CommandOptions) => Promise<Completed>;
// stdout and stderr go to one truncated log, or are captured as strict UTF-8.
export const command: Command = async (args, options = {}) => {
  // Descendants that outlive a waited wrapper are no longer its children; a
  // pipe they inherited would hold the wait open, so its output goes to a file.
  assert.ok(!options.waitOnInterrupt || options.log !== undefined, "waited wrapper log required");
  checkInterrupt();
  const controller = new AbortController();
  let forward = () => {
    controller.abort(interruptError());
  };
  const interrupt = () => {
    forward();
  };
  inFlight.add(interrupt);
  const fd = options.log === undefined ? undefined : openSync(options.log, "w");
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let status: [number, number] | undefined;
  // A wrapper tree that outlived its SIGKILL is a failure of its own, never
  // a clean interrupt.
  let treeFailure: Error | undefined;
  try {
    const cwd = resolve(options.cwd ?? process.cwd());
    const stdin = options.input === undefined ? "inherit" : new TextEncoder().encode(options.input);
    if (options.waitOnInterrupt) status = statusPipe();
    const child = status
      ? spawn([process.execPath, "--no-env-file", import.meta.path, cwd, ...args], {
          cwd: import.meta.dir,
          env: options.env ?? process.env,
          stdio: [stdin, fd ?? "pipe", fd ?? "pipe", status[1]],
        })
      : spawn(args, {
          cwd,
          env: options.env ?? process.env,
          stdin,
          stdout: fd ?? "pipe",
          stderr: fd ?? "pipe",
          signal: controller.signal,
          killSignal: "SIGKILL",
        });
    if (status) {
      closeSync(status[1]);
      status[1] = -1;
      forward = () => {
        controller.abort(interruptError());
        child.kill("SIGINT");
        // The first SIGINT starts the bound; a later one does not extend it.
        deadline ??= setTimeout(() => {
          if (!running(child)) return;
          try {
            killTree(child.pid);
          } catch (error) {
            child.kill("SIGKILL");
            const facts = errorFacts(error);
            treeFailure = runtimeError(
              `owned wrapper tree kill failed: ${facts.type}: ${facts.message}`,
            );
          }
        }, options.interruptGrace ?? wrapperInterruptGrace);
      };
    }
    const [stdout, stderr] = await Promise.all([
      child.stdout instanceof ReadableStream ? new Response(child.stdout).bytes() : null,
      child.stderr instanceof ReadableStream ? new Response(child.stderr).bytes() : null,
      child.exited,
    ]);
    // A launcher that never reached its exec failed its setup, unless an
    // interrupt killed it first.
    const launched = status ? readStatus(status[0]) : "ok";
    if (launched !== "ok" && !(launched === "" && controller.signal.aborted))
      throw runtimeError(
        `owned wrapper ${launched.replace(/^ok/, "") || "launcher"} setup failed; executable=${String(args[0])}`,
      );
    if (treeFailure) throw treeFailure;
    if (controller.signal.aborted) throw controller.signal.reason;
    const returncode = observedExit({
      exitCode: child.exitCode ?? 0,
      signalCode: child.signalCode ?? undefined,
    });
    if (options.required !== false && returncode !== 0)
      throw runtimeError(
        `owned command failed exit=${String(returncode)}; executable=${String(args[0])}`,
      );
    return {
      returncode,
      stdout: stdout ? decode(stdout) : "",
      stderr: stderr ? decode(stderr) : "",
    };
  } finally {
    clearTimeout(deadline);
    inFlight.delete(interrupt);
    if (fd !== undefined) closeSync(fd);
    for (const end of status ?? []) if (end >= 0) closeSync(end);
  }
};

// A long-lived owned child (Popen): its exit is waited for with a deadline.
export type Child = Pick<Subprocess, "exited" | "exitCode" | "signalCode" | "kill">;
export const running = (child: Child) => child.exitCode === null && child.signalCode === null;
export async function waitFor(child: Child, milliseconds: number): Promise<number> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      reject(named("TimeoutExpired", "owned child did not exit before its deadline"));
    }, milliseconds);
  });
  try {
    await Promise.race([child.exited, deadline]);
  } finally {
    clearTimeout(timer);
  }
  return observedExit({ exitCode: child.exitCode ?? 0, signalCode: child.signalCode ?? undefined });
}

// str.splitlines() for the line breaks these tools print.
export function lines(text: string): string[] {
  const result = text.split(/\r\n|\r|\n/);
  if (result.at(-1) === "") result.pop();
  return result;
}
const integer = (value: string) => {
  assert.ok(/^[0-9]+$/.test(value), "decimal integer required");
  return Number(value);
};

// json.dumps(value, indent=2) + "\n": the bytes some consumers re-hash.
export const pythonJson = sourceInputText;
// Exclusive create; a receipt is never replaced.
export function writeJson(path: string, value: unknown): void {
  writeFileSync(path, pythonJson(value), { flag: "wx" });
}
// Exclusive create at mode 0600 from the first byte.
export function privateText(path: string, text: string): void {
  const fd = openSync(path, "wx", 0o600);
  try {
    fchmodSync(fd, 0o600);
    writeFileSync(fd, text);
  } finally {
    closeSync(fd);
  }
}
export const privateWrite = (path: string, value: unknown) => {
  privateText(path, pythonJson(value));
};

export const hex40 = /^[0-9a-f]{40}$/;
export const isRecord = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);
// A JSON member the caller requires: a missing key refuses (Python KeyError)
// instead of comparing as undefined.
export function field(value: unknown, key: string): unknown {
  assert.ok(isRecord(value) && Object.hasOwn(value, key), "required JSON member missing");
  return value[key];
}
// [st_dev, st_ino] as JSON: an integer while it is exact as a number, its
// decimal text beyond 2**53 (64-bit inode file systems). Equal files give
// equal values, so receipts compare them with deepEquals.
export type Inode = [number | string, number | string];
export function inodeOf(path: string, link = false): Inode {
  const facts = link ? lstatSync(path, { bigint: true }) : statSync(path, { bigint: true });
  const exact = (value: bigint) => {
    const number = Number(value);
    return Number.isSafeInteger(number) ? number : value.toString();
  };
  return [exact(facts.dev), exact(facts.ino)];
}
// The digest column of `sha256sum` output, one per printed line.
export const copiedHashes = (stdout: string) =>
  lines(stdout).map((line) => line.trim().split(/\s+/)[0]);

// shlex.quote
export const shellQuote = (value: string) =>
  value === ""
    ? "''"
    : /^[A-Za-z0-9_@%+=:,./-]+$/.test(value)
      ? value
      : "'" + value.replaceAll("'", "'\"'\"'") + "'";

// base64.b64decode(validate=True) of a JSON attachment body.
export function attachmentJson(body: unknown): unknown {
  assert.ok(
    typeof body === "string" &&
      /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(body),
    "strict base64 attachment required",
  );
  return JSON.parse(decode(Buffer.from(body, "base64")));
}

// Same canonical digest the runner applies to a recorded failure.
export function failureDigest(value: unknown): string {
  const keys = new Set<string>();
  JSON.stringify(value, (key, child: unknown) => {
    if (key) keys.add(key);
    return child;
  });
  return digest(JSON.stringify(value, [...keys].sort()));
}

// rglob('*') + is_file(): symlinked files count, dangling links do not.
// The same SHA-256 hex as io.ts sha, through one reused buffer: io.ts allocates
// 1 MiB per file, and an input closure is ~130k mostly small files (25 s per
// pass against 3 s here; the Python driver took 6 s).
const hashBuffer = new Uint8Array(1048576);
export function fileSha(path: string): string {
  const fd = openSync(path, "r"),
    hash = createHash("sha256");
  try {
    for (;;) {
      const n = readSync(fd, hashBuffer);
      if (!n) break;
      hash.update(hashBuffer.subarray(0, n));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest("hex");
}
export const treeHashes = (directory: string) =>
  Object.fromEntries(
    files(directory, true).map((path) => [relative(directory, path), fileSha(path)]),
  );

export async function inputCheck(
  before: Inputs,
  head: string,
  tree: string,
  run: Command = command,
): Promise<Inputs> {
  const git = (...args: string[]) =>
    run(["git", "-c", "safe.directory=" + root, "-C", root, ...args]);
  const actualHead = (await git("rev-parse", "HEAD")).stdout.trim(),
    actualTree = (await git("rev-parse", "HEAD^{tree}")).stdout.trim(),
    status = (await git("status", "--short")).stdout;
  assert.ok(actualHead === head && actualTree === tree && status === before.status);
  assert.ok(Object.keys(before.tracked).length && Object.keys(before.external).length);
  const listed = (await git("ls-files", "-z")).stdout.split("\0").slice(0, -1);
  assert.ok(deepEquals(new Set(listed), new Set(Object.keys(before.tracked))));
  const hashes = (names: string[], base: string | null) =>
    Object.fromEntries(
      names.map((name) => [name, fileSha(base === null ? name : join(base, name))]),
    );
  const tracked = hashes(Object.keys(before.tracked), root),
    external = hashes(Object.keys(before.external), null),
    untracked = hashes(Object.keys(before.untracked), root);
  assert.ok(
    deepEquals(tracked, before.tracked) &&
      deepEquals(external, before.external) &&
      deepEquals(untracked, before.untracked),
  );
  return { head: actualHead, tree: actualTree, status, tracked, external, untracked };
}

export interface Row {
  pid: number;
  parent: number;
  uid: number;
  gid: number;
  args: string;
  already_retired_at_observation?: true;
  namespace_pid?: number;
  start_ticks?: string;
}
const startTicks = (stat: string) => {
  const ticks = stat
    .slice(stat.lastIndexOf(")") + 1)
    .trim()
    .split(/\s+/)[19];
  assert.ok(ticks !== undefined && stat.includes(")"), "process start time required");
  return ticks;
};
const missing = (error: unknown) => (error as NodeJS.ErrnoException).code === "ENOENT";
export async function ownedRows(name: string, run: Command = command): Promise<Row[]> {
  const result = await run(["docker", "top", name, "-eo", "pid,ppid,uid,gid,args"]);
  return lines(result.stdout)
    .slice(1)
    .map((line) => {
      // line.split(None, 4): four fields, then the rest with its trailing text.
      const match = /^\s*(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S.*)$/s.exec(line);
      assert.ok(match, "docker top row");
      const [pid, parent, uid, gid] = match.slice(1, 5).map(integer) as [
          number,
          number,
          number,
          number,
        ],
        args = match[5] as string;
      let status: string, stat: string;
      try {
        status = readText(`/proc/${String(pid)}/status`);
        stat = readText(`/proc/${String(pid)}/stat`);
      } catch (error) {
        if (!missing(error)) throw error;
        // The daemon observed this owned short-lived child before it reaped.
        return { pid, parent, uid, gid, args, already_retired_at_observation: true };
      }
      const namespace = /^NSpid:\s+(.+)$/m.exec(status)?.[1]?.trim().split(/\s+/).at(-1);
      assert.ok(namespace !== undefined, "namespace pid required");
      return {
        pid,
        parent,
        uid,
        gid,
        args,
        namespace_pid: integer(namespace),
        start_ticks: startTicks(stat),
      };
    });
}
export function identityGone(row: Row): boolean {
  if (row.already_retired_at_observation) return true;
  try {
    return startTicks(readText(`/proc/${String(row.pid)}/stat`)) !== row.start_ticks;
  } catch (error) {
    if (missing(error)) return true;
    throw error;
  }
}
// Only Docker's positive "no such object" answer proves absence.
export async function ownedObjectAbsent(args: string[], run: Command = command): Promise<boolean> {
  const checked = await run(args, { required: false });
  const stderr = checked.stderr.toLowerCase();
  return (
    checked.returncode !== 0 &&
    ["no such object", "no such container", "no such volume"].some((m) => stderr.includes(m))
  );
}
// socket.connect_ex(('127.0.0.1', port)) != 0 with a one second timeout.
export function portClosed(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port, timeout: 1000 });
    const settle = (closed: boolean) => {
      socket.destroy();
      resolve(closed);
    };
    socket.once("connect", () => {
      settle(false);
    });
    socket.once("error", () => {
      settle(true);
    });
    socket.once("timeout", () => {
      settle(true);
    });
  });
}

export async function cleanupAttempt<T>(
  receipt: Receipt,
  errors: unknown[],
  label: string,
  operation: () => T | Promise<T>,
): Promise<T | null> {
  try {
    return await operation();
  } catch (error) {
    errors.push(label);
    secondary(receipt, label, error);
    return null;
  }
}
export function secondary(receipt: Receipt, phase: string, error: unknown): void {
  const facts = errorFacts(error);
  list(receipt, "secondary_cleanup_errors").push({ phase, ...facts });
}
export function list(receipt: Receipt, key: string): unknown[] {
  const value = receipt[key];
  if (Array.isArray(value)) return value as unknown[];
  const created: unknown[] = [];
  receipt[key] = created;
  return created;
}

export const basePacketKeys = [
  "failed_phase",
  "observed_failed_exit",
  "failure_code",
  "original_driver_failure",
  "original_body_log_sha256",
] as const;
export interface FailureOptions {
  error?: unknown;
  bodyLog?: string;
  packetName?: string;
  // Lane-specific fields recorded with the base keys.
  extraKeys?: readonly string[];
}
// Persist this driver's first actual outcome before any risky cleanup.
export function failureCheckpoint(
  receipt: Receipt,
  directory: string,
  observedExit: unknown,
  options: FailureOptions = {},
): void {
  const failed = "error" in options;
  if (!("original_driver_failure" in receipt)) {
    receipt.failed_phase = receipt.phase;
    receipt.observed_failed_exit = observedExit;
    receipt.original_driver_failure = failed
      ? errorFacts(options.error)
      : { type: "ReturnedNonzero", phase: receipt.phase, observedExit };
    receipt.failure_code = failed ? "SELECTED_DRIVER_EXCEPTION" : "SELECTED_BODY_NONZERO";
    receipt.original_body_log_sha256 = null;
    if (options.bodyLog !== undefined) {
      try {
        receipt.original_body_log_sha256 = sha(options.bodyLog);
      } catch {
        list(receipt, "diagnostic_errors").push("original-body-log-hash-failed");
      }
    }
  }
  const path = join(directory, options.packetName ?? "original-failure.private.json");
  try {
    if (!existsSync(path)) {
      const fd = openSync(path, "wx", 0o600);
      try {
        fchmodSync(fd, 0o600);
        const keys = [...basePacketKeys, ...(options.extraKeys ?? [])];
        writeFileSync(
          fd,
          JSON.stringify(Object.fromEntries(keys.map((k) => [k, receipt[k] ?? null]))),
        );
      } finally {
        closeSync(fd);
      }
    }
    receipt.original_failure_checkpoint_sha256 = sha(path);
  } catch {
    list(receipt, "diagnostic_errors").push("original-failure-checkpoint-write-failed");
  }
}

export const knownOnBrowserTest =
  "selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback";
export const knownRestartBrowserTest =
  "selected normal main restart: fresh actor reads persisted native history and manual revision";
const knownBrowserSuffixes = [
  "e2e-pending/workspace-wiki-selected-backend.spec.ts",
  "e2e-pending/workspace-wiki-selected-auxiliary.ts",
] as const;
const knownBrowserStatuses = ["failed", "timedOut", "interrupted"];
export const browserPacketKeys = [
  "known_browser_test",
  "known_browser_status",
  "known_browser_checkpoint",
  "browser_report_state",
] as const;
export interface BrowserCheckpoint {
  known_browser_test: string | null;
  known_browser_status: string | null;
  known_browser_checkpoint: string | null;
  browser_report_state: string;
}
function knownSource(path: unknown, suffix: string): boolean {
  if (typeof path !== "string" || path.length > 4096 || /[\n\r]/.test(path)) return false;
  const normalized = path.replaceAll("\\", "/");
  return (
    normalized === suffix ||
    normalized.endsWith("/" + suffix) ||
    normalized === suffix.slice(suffix.lastIndexOf("/") + 1)
  );
}
function locationOf(result: Json): Json | null {
  if (isRecord(result.errorLocation)) return result.errorLocation;
  const errors = result.errors;
  if (Array.isArray(errors) && isRecord(errors[0]) && isRecord(errors[0].location))
    return errors[0].location;
  return null;
}
// Reads the reporter's errorLocation fields only. Stacks and messages are never
// parsed; a missing location stays null. `report` comes from io.read so its
// integers keep their source tokens.
export function knownBrowserCheckpoint(report: unknown, restart = false): BrowserCheckpoint {
  const integral = jsonInteger;
  const title = restart ? knownRestartBrowserTest : knownOnBrowserTest;
  const empty = (state: string): BrowserCheckpoint => ({
    known_browser_test: null,
    known_browser_status: null,
    known_browser_checkpoint: null,
    browser_report_state: state,
  });
  try {
    if (!isRecord(report)) return empty("report-unreadable");
    const config = report.config;
    if (!isRecord(config) || !integral(config, "workers") || config.workers !== 1)
      return empty("workers-not-one");
    if (!Array.isArray(report.suites)) return empty("spec-mismatch");
    const found: Json[] = [];
    const stack: unknown[] = [...(report.suites as unknown[])];
    while (stack.length) {
      const suite = stack.pop();
      if (!isRecord(suite)) return empty("report-unreadable");
      if (Array.isArray(suite.specs)) found.push(...(suite.specs as unknown[]).filter(isRecord));
      if (Array.isArray(suite.suites)) stack.push(...(suite.suites as unknown[]));
    }
    const matched = found.filter(
      (spec) => spec.title === title && knownSource(spec.file, knownBrowserSuffixes[0]),
    );
    if (matched.length !== 1) return empty("spec-mismatch");
    const tests = matched[0]?.tests;
    if (!Array.isArray(tests) || tests.length !== 1 || !isRecord(tests[0]))
      return empty("spec-mismatch");
    const results = tests[0].results;
    if (!Array.isArray(results) || results.length !== 1 || !isRecord(results[0]))
      return empty("spec-mismatch");
    const result = results[0];
    const status = result.status;
    if (typeof status !== "string" || !knownBrowserStatuses.includes(status))
      return empty("status-not-known");
    const location = locationOf(result);
    let checkpoint: string | null = null;
    if (
      location &&
      integral(location, "line") &&
      (location.line as number) >= 1 &&
      (location.line as number) <= 10000
    ) {
      const suffix = knownBrowserSuffixes.find((item) => knownSource(location.file, item));
      if (suffix !== undefined) checkpoint = suffix + ":" + String(location.line);
    }
    return {
      known_browser_test: title,
      known_browser_status: status,
      known_browser_checkpoint: checkpoint,
      browser_report_state: "matched",
    };
  } catch {
    return empty("report-unreadable");
  }
}

// file:line of the innermost frame in `file`, from the stack only; never the
// message or locals. Null when the error carries no such frame.
export function frameLine(error: unknown, file: string): number | null {
  const stack = error instanceof Error ? error.stack : undefined;
  if (typeof stack !== "string") return null;
  for (const line of stack.split("\n")) {
    const match = /\(?(\/[^()]*?):([0-9]+):[0-9]+\)?$/.exec(line.trim());
    if (match?.[1] === file) return Number(match[2]);
  }
  return null;
}

// Re-exec of a lane driver under Bun: the same executable, no .env autoload.
export const bunDriver = (driver: string) => [process.execPath, "--no-env-file", driver];
export function assertNoEnvFile(): void {
  assert.ok(process.execArgv.includes("--no-env-file"), "lane driver requires bun --no-env-file");
}

// The restart/normal-server I/O both selected normal lanes share.
export const emit = (line: string) => {
  writeSync(1, line + "\n");
};
export const actor = () => [process.getuid?.(), process.getgid?.()] as const;
export function probeSetup(base: string): Promise<{ status: number; body: unknown }> {
  // node:http never consults proxy variables; the owned server is loopback.
  return new Promise((resolvePromise, reject) => {
    const request = get(base + "/api/v1/setup", { timeout: 10_000 }, (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      response.on("error", reject);
      response.on("end", () => {
        try {
          resolvePromise({
            status: response.statusCode ?? 0,
            body: JSON.parse(decode(Buffer.concat(chunks))),
          });
        } catch (error) {
          reject(error instanceof Error ? error : runtimeError("owned setup probe failed"));
        }
      });
    });
    request.on("timeout", () => request.destroy(runtimeError("owned setup probe timed out")));
    request.on("error", reject);
  });
}
export function spawnServer(args: string[], log: number): Child {
  return spawn(args, { stdin: "ignore", stdout: log, stderr: log });
}

if (import.meta.main) execSubreaper(process.argv.slice(2));
