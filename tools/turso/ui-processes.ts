// One hosted UI consumer's allocations, never a reusable process service.
// The subreaper attribute is process-local. Signals use captured pidfds only.
// Neither /proc secrets nor unrelated process capabilities are inspected.
import { spawn, type Subprocess } from "bun";
import { dlopen, ptr } from "bun:ffi";
import { closeSync, readFileSync, readdirSync, readlinkSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { observedExit, write } from "../selected-backend-ci/io.ts";
import {
  diagnosticJson,
  failureCode,
  require,
  root,
  stdio,
  token,
  UiError,
  type Output,
} from "./ui-common.ts";

export interface ProcRow {
  pid: number;
  parentPid: number;
  startTicks: string;
  state: string;
}
export interface Entry {
  identity: ProcRow;
  label: string;
  allocation: number | null;
  pidfd: number | null;
  reaped: boolean;
}
export type Child = Subprocess<
  "pipe" | "ignore" | "inherit" | number,
  "pipe" | "ignore" | "inherit" | number,
  "pipe" | "ignore" | "inherit" | number
>;
export interface Allocation {
  process: Child;
  label: string;
  closed: boolean;
  forced: boolean;
  key?: string;
}
export interface SpawnOptions {
  env: Record<string, string>;
  cwd?: string;
  stdin?: "pipe" | "ignore" | "inherit" | number;
  stdout?: "pipe" | "ignore" | number;
  stderr?: "pipe" | "ignore" | number;
}

export const SIGTERM = 15;
export const SIGKILL = 9;
const PR_SET_CHILD_SUBREAPER = 36;
const PR_GET_CHILD_SUBREAPER = 37;
const SYS_PIDFD_SEND_SIGNAL = 424n;
const SYS_PIDFD_OPEN = 434n;
const WNOHANG = 1;

const missing = (error: unknown) => {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  return code === "ENOENT" || code === "ESRCH";
};

export function procIdentity(pid: number): ProcRow {
  const raw = (
    readFileSync(join("/proc", String(pid), "stat"), "utf8")
      .split(")")
      .pop() as string
  )
    .trim()
    .split(/\s+/);
  const parent = Number(raw[1]);
  if (!Number.isSafeInteger(parent) || raw.length < 20) throw new TypeError("malformed stat");
  return { pid, parentPid: parent, startTicks: raw[19] as string, state: raw[0] as string };
}

/** Kernel calls the scope needs and Bun does not expose. Tests replace them. */
export interface Kernel {
  procRows(): ProcRow[];
  procIdentity(pid: number): ProcRow;
  pidfdOpen(pid: number): number;
  pidfdSendSignal(fd: number, signal: number): void;
  close(fd: number): void;
  reap(pid: number): number;
  getSubreaper(): number;
  setSubreaper(value: number): void;
  spawn(args: string[], options: SpawnOptions): Child;
}

let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("libc.so.6", {
    // prctl and syscall are variadic: every argument is passed as an unsigned
    // long, including the GET pointer, matching the Linux x86-64/aarch64 ABI.
    prctl: { args: ["i32", "u64", "u64", "u64", "u64"], returns: "i32" },
    syscall: { args: ["i64", "i64", "i64", "i64", "i64"], returns: "i64" },
    waitpid: { args: ["i32", "ptr", "i32"], returns: "i32" },
    poll: { args: ["ptr", "u64", "i32"], returns: "i32" },
    mkfifo: { args: ["ptr", "u32"], returns: "i32" },
  }).symbols;
}
const native = () => (libc ??= openLibc());

const POLLIN = 0x1,
  POLLERR = 0x8,
  POLLHUP = 0x10;
/** select(2) readability without blocking: data, hang-up or error is ready. */
export function readable(fd: number): boolean {
  // struct pollfd { int fd; short events; short revents; }
  const pollfd = new Int32Array(2);
  pollfd[0] = fd;
  pollfd[1] = POLLIN;
  const ready = native().poll(ptr(pollfd), 1n, 0);
  if (ready < 0) throw new Error("poll failed");
  return ready > 0 && ((pollfd[1] >>> 16) & (POLLIN | POLLERR | POLLHUP)) !== 0;
}
export function mkfifo(path: string, mode: number): void {
  if (native().mkfifo(ptr(Buffer.from(path + "\0")), mode) !== 0) throw new Error("mkfifo failed");
}

export const linuxKernel: Kernel = {
  procRows() {
    const names = readdirSync("/proc").filter((name) => /^[0-9]+$/.test(name));
    require(names.length <= 4096, "UI_PROCESS_SNAPSHOT_CAP_REFUSED");
    const rows: ProcRow[] = [];
    for (const name of names) {
      try {
        rows.push(procIdentity(Number(name)));
      } catch (error) {
        if (!missing(error)) throw error;
      }
    }
    return rows;
  },
  procIdentity,
  pidfdOpen(pid) {
    const fd = Number(native().syscall(SYS_PIDFD_OPEN, BigInt(pid), 0n, 0n, 0n));
    if (fd >= 0) return fd;
    // errno is not observable through bun:ffi; an absent /proc entry is the
    // ESRCH case, anything else stays an unclassified failure.
    procIdentity(pid);
    throw new Error("pidfd_open failed");
  },
  pidfdSendSignal(fd, signal) {
    if (Number(native().syscall(SYS_PIDFD_SEND_SIGNAL, BigInt(fd), BigInt(signal), 0n, 0n)) !== 0)
      throw new Error("pidfd_send_signal failed");
  },
  close: closeSync,
  reap(pid) {
    const status = new Int32Array(1);
    const reaped = native().waitpid(pid, ptr(status), WNOHANG);
    if (reaped < 0) throw new Error("waitpid failed");
    return reaped;
  },
  getSubreaper() {
    const box = new Int32Array(1);
    if (native().prctl(PR_GET_CHILD_SUBREAPER, BigInt(ptr(box)), 0n, 0n, 0n) !== 0)
      throw new UiError("UI_SUBREAPER_SETUP_FAILED");
    return box[0] as number;
  },
  setSubreaper(value) {
    if (native().prctl(PR_SET_CHILD_SUBREAPER, BigInt(value), 0n, 0n, 0n) !== 0)
      throw new UiError("UI_SUBREAPER_SETUP_FAILED");
  },
  spawn(args, options) {
    // setsid: the child is its own session, as each allocation is signalled
    // only through captured pidfds.
    return spawn(args, {
      env: options.env,
      cwd: options.cwd,
      stdin: options.stdin ?? "ignore",
      stdout: options.stdout ?? "ignore",
      stderr: options.stderr ?? "ignore",
      detached: true,
    });
  },
};

/** Exit status of an exited child (negative signal number), null while it runs. */
export function poll(child: Pick<Child, "exitCode" | "signalCode">): number | null {
  if (child.exitCode === null && child.signalCode === null) return null;
  return observedExit({ exitCode: child.exitCode ?? 0, signalCode: child.signalCode ?? undefined });
}
export class WaitTimeout extends Error {}
export async function wait(child: Child, seconds: number): Promise<number> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<"timeout">((resolve) => {
    timer = setTimeout(() => {
      resolve("timeout");
    }, seconds * 1000);
  });
  try {
    if ((await Promise.race([child.exited, expired])) === "timeout") throw new WaitTimeout();
  } finally {
    clearTimeout(timer);
  }
  return poll(child) as number;
}
/**
 * communicate(): writes `input` (if the child has a stdin pipe), reads stdout
 * to EOF and waits for exit, all within `seconds`; WaitTimeout otherwise.
 */
export async function communicate(
  child: Child,
  input: string,
  seconds: number,
): Promise<Uint8Array> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<"timeout">((resolve) => {
    timer = setTimeout(() => {
      resolve("timeout");
    }, seconds * 1000);
  });
  const sink = child.stdin as { write(data: string): unknown; end(): unknown } | null | undefined;
  const feed = (async () => {
    if (!sink || typeof sink === "number") return;
    // A child that exits without reading its input is not an input failure.
    try {
      if (input) await sink.write(input);
      await sink.end();
    } catch {
      // Broken pipe.
    }
  })();
  const output =
    child.stdout instanceof ReadableStream
      ? new Response(child.stdout).bytes()
      : Promise.resolve(new Uint8Array());
  try {
    const done = await Promise.race([Promise.all([output, child.exited, feed]), expired]);
    if (done === "timeout") throw new WaitTimeout();
    return done[0];
  } finally {
    clearTimeout(timer);
  }
}

export interface Identity extends ProcRow {
  comm: string;
  exe: string | null;
  exeInspection: "observed" | "UNAVAILABLE";
}
export function identity(pid: number): Identity {
  const row = procIdentity(pid);
  const comm = readFileSync(join("/proc", String(pid), "comm"), "utf8").trim();
  try {
    const exe = readlinkSync(join("/proc", String(pid), "exe"));
    return { ...row, comm, exe, exeInspection: "observed" };
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code !== "EACCES" && code !== "EPERM") throw error;
    return { ...row, comm, exe: null, exeInspection: "UNAVAILABLE" };
  }
}

export const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Monotonic seconds, the clock of every deadline in the consumer. */
export const now = () => performance.now() / 1000;
const keyOf = (row: Pick<ProcRow, "pid" | "startTicks">) => String(row.pid) + ":" + row.startTicks;

export interface ScopeIo {
  output: Output;
  write: typeof write;
  root: () => string;
}
const defaultIo: ScopeIo = { output: stdio, write, root };

export class UiProcesses {
  pid = process.pid;
  entries = new Map<string, Entry>();
  allocations: Allocation[] = [];
  errors: string[] = [];
  prior = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private watching = false;

  constructor(
    readonly kernel: Kernel = linuxKernel,
    readonly io: ScopeIo = defaultIo,
  ) {}

  static active: UiProcesses | null = null;

  open(): this {
    require(UiProcesses.active === null &&
      process.platform === "linux" &&
      this.pidfdUsable(), "UI_PROCESS_CAPABILITY_REQUIRED");
    require(!this.kernel
      .procRows()
      .some((row) => row.parentPid === this.pid), "UI_PREEXISTING_CHILD_REFUSED");
    this.prior = this.kernel.getSubreaper();
    try {
      this.kernel.setSubreaper(1);
      require(this.kernel.getSubreaper() === 1, "UI_SUBREAPER_NOT_CONFIRMED");
      this.startWatch();
      UiProcesses.active = this;
    } catch (original) {
      // No allocation is possible before this method returns. Restore the
      // observed prior attribute without masking the setup failure.
      try {
        this.kernel.setSubreaper(this.prior);
        require(this.kernel.getSubreaper() === this.prior, "UI_SUBREAPER_RESTORE_FAILED");
      } catch {
        this.io.output.err(
          diagnosticJson({
            originalFailure: failureCode(original),
            processCleanupErrors: ["UI_SUBREAPER_RESTORE_FAILED"],
          }),
        );
      }
      throw original;
    }
    return this;
  }

  // Every signal goes through a pidfd, so the scope opens only where both
  // syscalls work; signal 0 on our own pidfd checks delivery without sending.
  private pidfdUsable(): boolean {
    let fd: number;
    try {
      fd = this.kernel.pidfdOpen(this.pid);
    } catch {
      return false;
    }
    let usable = true;
    try {
      this.kernel.pidfdSendSignal(fd, 0);
    } catch {
      usable = false;
    }
    try {
      this.kernel.close(fd);
    } catch {
      usable = false;
    }
    return usable;
  }

  startWatch(): void {
    this.watching = true;
    this.timer = setInterval(() => {
      try {
        this.snapshot();
      } catch {
        this.errors.push("UI_PROCESS_OBSERVATION_FAILED");
        this.stopWatch();
      }
    }, 20);
  }
  stopWatch(): void {
    clearInterval(this.timer);
    this.watching = false;
  }
  get observing(): boolean {
    return this.watching;
  }

  retired(row: Pick<ProcRow, "pid" | "startTicks">): boolean {
    try {
      return this.kernel.procIdentity(row.pid).startTicks !== row.startTicks;
    } catch (error) {
      if (missing(error)) return true;
      throw error;
    }
  }

  capture(row: ProcRow, label: string, allocation: number | null = null): string {
    const key = keyOf(row);
    if (this.entries.has(key)) return key;
    require(this.entries.size < 512, "UI_PROCESS_HISTORY_CAP_REFUSED");
    let fd: number | null = null;
    try {
      fd = this.kernel.pidfdOpen(row.pid);
      require(this.kernel.procIdentity(row.pid).startTicks ===
        row.startTicks, "UI_PROCESS_IDENTITY_RACE");
    } catch (error) {
      if (!missing(error)) {
        if (fd !== null) this.kernel.close(fd);
        throw error;
      }
      // A vanished process has no usable pidfd; it must already be retired.
      require(this.retired(row), "UI_PROCESS_IDENTITY_UNCONFIRMED");
      if (fd !== null) this.kernel.close(fd);
      fd = null;
    }
    this.entries.set(key, { identity: row, label, allocation, pidfd: fd, reaped: false });
    return key;
  }

  spawn(args: string[], label: string, options: SpawnOptions): Child {
    const child = this.kernel.spawn(args, options);
    const allocation: Allocation = { process: child, label, closed: false, forced: false };
    this.allocations.push(allocation);
    try {
      allocation.key = this.capture(
        this.kernel.procIdentity(child.pid),
        label,
        this.allocations.length - 1,
      );
    } catch (error) {
      this.errors.push("UI_SPAWN_IDENTITY_UNCONFIRMED");
      // No PID-only kill is allowed when capture fails.
      throw error;
    }
    this.snapshot();
    return child;
  }

  snapshot(): void {
    if (!this.allocations.some((a) => !a.closed)) return;
    const rows = this.kernel.procRows();
    const selected = new Map(rows.map((row) => [row.pid, row]));
    const parents = new Set<number>();
    for (const entry of this.entries.values()) {
      const actual = selected.get(entry.identity.pid);
      if (actual && actual.startTicks === entry.identity.startTicks)
        parents.add(entry.identity.pid);
    }
    for (;;) {
      const found = rows.filter((r) => parents.has(r.parentPid) || r.parentPid === this.pid);
      const fresh = found.filter((r) => !this.entries.has(keyOf(r)));
      if (!fresh.length) break;
      for (const row of fresh) {
        // With zero preexisting children, actual adoption by this subreaper
        // proves its own descendant ancestry, even detached.
        const parent = selected.get(row.parentPid);
        const parentEntry = parent ? this.entries.get(keyOf(parent)) : undefined;
        this.capture(
          row,
          "observed-or-adopted-descendant",
          parentEntry ? parentEntry.allocation : null,
        );
        parents.add(row.pid);
      }
    }
    const roots = new Set(this.allocations.map((a) => a.process.pid));
    for (const entry of this.entries.values()) {
      const row = entry.identity;
      const actual = selected.get(row.pid);
      if (
        actual &&
        actual.startTicks === row.startTicks &&
        actual.state === "Z" &&
        actual.parentPid === this.pid &&
        !roots.has(row.pid)
      )
        entry.reaped = this.kernel.reap(row.pid) === row.pid;
    }
  }

  send(entry: Entry, signal: number): void {
    if (this.retired(entry.identity)) return;
    require(entry.pidfd !== null, "UI_PROCESS_SIGNAL_UNQUALIFIED");
    this.kernel.pidfdSendSignal(entry.pidfd, signal);
  }

  async finish(child: Child, normalSignal = false): Promise<number> {
    const index = this.allocations.findIndex((a) => a.process === child);
    if (index < 0) throw new TypeError("unknown allocation");
    const allocation = this.allocations[index] as Allocation;
    this.snapshot();
    const entry = allocation.key === undefined ? undefined : this.entries.get(allocation.key);
    if (entry === undefined) throw new TypeError("uncaptured allocation");
    if (normalSignal && poll(child) === null) this.send(entry, SIGTERM);
    let code: number;
    try {
      code = await wait(child, 10);
    } catch (error) {
      if (!(error instanceof WaitTimeout)) throw error;
      allocation.forced = true;
      this.send(entry, SIGKILL);
      code = await wait(child, 10);
    }
    const deadline = performance.now() + 10000;
    let forcedDeadline: number | null = null;
    for (;;) {
      this.snapshot();
      const ownAlive = [...this.entries.values()].filter(
        (e) => (e.allocation === index || e.allocation === null) && !this.retired(e.identity),
      );
      const otherLive = this.allocations.some((a, i) => i !== index && !a.closed);
      if (!ownAlive.length) break;
      if (performance.now() >= deadline) {
        allocation.forced = true;
        // Unknown adopted ancestry is still our own descendant, but cannot be
        // assigned to a concurrent allocation.
        for (const e of ownAlive) if (e.allocation === index || !otherLive) this.send(e, SIGKILL);
        forcedDeadline ??= performance.now() + 10000;
        if (performance.now() >= forcedDeadline) break;
      }
      await pause(20);
    }
    this.snapshot();
    allocation.closed =
      this.retired(entry.identity) &&
      [...this.entries.values()]
        .filter((e) => e.allocation === index)
        .every((e) => this.retired(e.identity));
    require(!allocation.forced &&
      allocation.closed &&
      !this.errors.length, "UI_PROCESS_CLOSURE_FAILED");
    return code;
  }

  closure(): boolean {
    this.snapshot();
    return (
      !this.errors.length &&
      this.allocations.every((a) => a.closed) &&
      [...this.entries.values()].every((e) => this.retired(e.identity))
    );
  }

  /** Closes every allocation; `original` is the scope body's failure, if any. */
  async close(original: unknown = null): Promise<void> {
    const failures: string[] = [];
    let closed = false;
    try {
      for (const allocation of this.allocations) {
        if (!allocation.closed) {
          try {
            await this.finish(allocation.process, true);
          } catch {
            failures.push("UI_PROCESS_FINAL_CLOSURE_FAILED");
          }
        }
      }
      try {
        closed = this.closure();
      } catch {
        failures.push("UI_PROCESS_FINAL_OBSERVATION_FAILED");
      }
      if (closed) {
        try {
          this.kernel.setSubreaper(this.prior);
          require(this.kernel.getSubreaper() === this.prior, "UI_SUBREAPER_RESTORE_FAILED");
        } catch {
          failures.push("UI_SUBREAPER_RESTORE_FAILED");
        }
      }
    } finally {
      try {
        this.stopWatch();
      } catch {
        failures.push("UI_PROCESS_OBSERVER_STOP_FAILED");
      }
      for (const entry of this.entries.values()) {
        if (entry.pidfd !== null) {
          try {
            this.kernel.close(entry.pidfd);
          } catch {
            failures.push("UI_PIDFD_CLOSE_FAILED");
          }
        }
      }
      UiProcesses.active = null;
    }
    try {
      this.io.write(join(this.io.root(), "process-closure-" + token(6) + ".private.json"), {
        confirmed: closed,
        normalClosure: closed && !this.allocations.some((a) => a.forced),
        errors: [...this.errors, ...failures],
        allocations: this.allocations.map(({ label, closed: done, forced }) => ({
          label,
          closed: done,
          forced,
        })),
        identities: [...this.entries.values()].map((e) => ({
          identity: e.identity,
          allocation: e.allocation,
          reaped: e.reaped,
        })),
      });
    } catch {
      failures.push("UI_PROCESS_RECEIPT_WRITE_FAILED");
    }
    if (failures.length) {
      try {
        this.io.output.err(
          diagnosticJson({
            originalFailure: original !== null ? failureCode(original) : null,
            processCleanupErrors: failures,
          }),
        );
      } catch {
        // The original failure stays the result.
      }
    }
    if (original === null) require(closed && !failures.length, "UI_PROCESS_CLOSURE_FAILED");
  }
}

/** The scope surface the fixture, server, browser and container code use. */
export type Scope = Pick<
  UiProcesses,
  "io" | "spawn" | "finish" | "closure" | "allocations" | "entries" | "retired" | "send" | "capture"
>;

/** Runs `body` inside one owned process scope; the body's failure stays first. */
export async function withProcesses<T>(
  body: (scope: UiProcesses) => Promise<T>,
  scope: UiProcesses = new UiProcesses(),
): Promise<T> {
  scope.open();
  let result: T;
  try {
    result = await body(scope);
  } catch (error) {
    await scope.close(error);
    throw error;
  }
  await scope.close();
  return result;
}
