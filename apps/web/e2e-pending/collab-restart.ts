/**
 * Test-owned fvoci-server process for pending collab acceptance.
 *
 * Graceful dispose signals only the server PID so the parent can flush and
 * reap collab-engine children; group SIGKILL is failure cleanup, not the
 * success path. crashAndRestart SIGKILLs the process group and is the
 * process-tree crash durability case. Group membership is observed via
 * /proc, not assumed from the kill signal.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const LISTEN_RE = /fvoci-server listening on (http:\/\/127\.0\.0\.1:\d+)/;
const BIND_FAIL_RE = /address already in use|error binding|AddrInUse|os error 98/i;

const CHILD_ENV_ALLOW = [
  "PATH",
  "HOME",
  "LANG",
  "LC_ALL",
  "TZ",
  "LD_LIBRARY_PATH",
  "RUST_LOG",
  "DATABASE_APP_URL",
  "PASSWORD_PEPPER_KEYS",
  "PASSWORD_PEPPER_ACTIVE_KEY_ID",
  "FVOCI_STATIC_DIR",
  "FVOCI_STORAGE_DIR",
  "FVOCI_COLLAB_ENGINE",
] as const;

const CHILD_ENV_DENY = [
  "DATABASE_URL",
  "FVOCI_MIGRATION_URL",
  "FVOCI_E2E_ADMIN_DATABASE_URL",
  "TEST_DATABASE_URL",
  "FVOCI_TEST_PG_CONTAINER",
] as const;

export const SERVER_BIN_MISSING =
  "FVOCI_E2E_SERVER_BIN must be the built fvoci-server path from scripts/web-e2e-inner.sh";

export type ProcMember = {
  pid: number;
  comm: string;
  starttime: string;
  pgrp: number;
};

export function ownedServerChildEnv(
  bind: string,
  source: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of CHILD_ENV_ALLOW) {
    const value = source[key];
    if (value !== undefined && value !== "") env[key] = value;
  }
  for (const key of CHILD_ENV_DENY) {
    delete env[key];
  }
  env.FVOCI_BIND = bind;
  env.FVOCI_PUBLIC_ORIGIN = `http://${bind}`;
  return env;
}

export function readProcMember(pid: number): ProcMember | null {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    const close = stat.lastIndexOf(")");
    if (close < 0) return null;
    const comm = stat.slice(stat.indexOf("(") + 1, close);
    const rest = stat.slice(close + 2).split(" ");
    return {
      pid,
      comm,
      pgrp: Number(rest[2]),
      starttime: rest[19] ?? "",
    };
  } catch {
    return null;
  }
}

export function processGroupMembers(pgid: number): ProcMember[] {
  const members: ProcMember[] = [];
  let names: string[] = [];
  try {
    names = readdirSync("/proc");
  } catch {
    return members;
  }
  for (const name of names) {
    if (!/^\d+$/.test(name)) continue;
    const pid = Number(name);
    try {
      const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
      const close = stat.lastIndexOf(")");
      if (close < 0) continue;
      const comm = stat.slice(stat.indexOf("(") + 1, close);
      const rest = stat.slice(close + 2).split(" ");
      if (Number(rest[2]) === pgid) {
        members.push({
          pid,
          comm,
          pgrp: Number(rest[2]),
          starttime: rest[19] ?? "",
        });
      }
    } catch {
      /* process exited while scanning */
    }
  }
  return members;
}

function sameIdentity(before: ProcMember | null, pid: number): boolean {
  if (before == null) return false;
  const now = readProcMember(pid);
  return now != null && now.starttime === before.starttime;
}

/** Refuse a recycled process-group number unless a recorded member still owns it. */
export function signalOwnedGroup(pgid: number, owners: readonly ProcMember[]): void {
  const current = processGroupMembers(pgid);
  if (current.length === 0) return;
  if (!current.some((member) => owners.some((owner) =>
    owner.pid === member.pid && owner.starttime === member.starttime && owner.pgrp === pgid,
  ))) {
    throw new Error(`cannot prove ownership of process group ${pgid}`);
  }
  try {
    process.kill(-pgid, "SIGKILL");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

export class OwnedServer {
  child: ChildProcess | null = null;
  pgid: number | null = null;
  parentPid: number | null = null;
  parentIdentity: ProcMember | null = null;
  private ownedMembers: ProcMember[] = [];
  baseUrl = "";
  bind = "";
  logs = "";
  logPath = "";
  runDir = "";
  storageDir = "";
  lastGracefulLeftovers: ProcMember[] = [];

  static async start(bind = "127.0.0.1:0"): Promise<OwnedServer> {
    const server = new OwnedServer();
    await server.spawnAt(bind);
    return server;
  }

  requiredBin(): string {
    const bin = process.env.FVOCI_E2E_SERVER_BIN?.trim() ?? "";
    if (bin === "" || !existsSync(bin)) {
      throw new Error(SERVER_BIN_MISSING);
    }
    return bin;
  }

  /**
   * Recycle the server PID on the same bind and isolated DB so collab rooms
   * from an independent scenario cannot occupy the product four-room cap.
   * SIGTERM the parent only; this is not the crash SIGKILL case.
   */
  async recycle(): Promise<void> {
    if (this.bind === "") {
      throw new Error("cannot recycle before the owned server has bound a port");
    }
    const bind = this.bind;
    await this.shutdownGraceful();
    await this.spawnAt(bind, bind);
  }

  /** Process-tree crash durability: SIGKILL the group, then rebind the same port. */
  async crashAndRestart(): Promise<void> {
    if (this.bind === "") {
      throw new Error("cannot restart before the owned server has bound a port");
    }
    const bind = this.bind;
    const members = this.observeOwnedMembers();
    if (!members.some((member) => member.comm === "collab-engine")) {
      throw new Error("process-tree crash requires an observed live collaboration helper");
    }
    await this.killGroupObserved();
    await this.spawnAt(bind, bind);
  }

  async dispose(): Promise<void> {
    await this.shutdownGraceful();
  }

  /**
   * SIGTERM the server PID only so its async shutdown can flush and reap helpers.
   * Group SIGKILL is only if parent or helpers remain after the wait.
   */
  async shutdownGraceful(): Promise<void> {
    const child = this.child;
    const pgid = this.pgid;
    const parentPid = this.parentPid;
    const identity = this.parentIdentity;
    const owners = this.observeOwnedMembers();
    this.child = null;
    this.pgid = null;
    this.parentPid = null;
    this.parentIdentity = null;
    if (parentPid == null || pgid == null) return;
    if (sameIdentity(identity, parentPid)) {
      try {
        process.kill(parentPid, "SIGTERM");
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
      }
    }
    if (child && child.exitCode == null && child.signalCode == null) {
      await Promise.race([once(child, "exit"), delay(5_000)]);
    }
    const leftoverDeadline = Date.now() + 5_000;
    let leftovers = processGroupMembers(pgid);
    while (Date.now() < leftoverDeadline && leftovers.length > 0) {
      await delay(50);
      leftovers = processGroupMembers(pgid);
    }
    this.lastGracefulLeftovers = leftovers;
    if (leftovers.length > 0 || sameIdentity(identity, parentPid)) {
      signalOwnedGroup(pgid, owners);
      await delay(200);
      leftovers = processGroupMembers(pgid);
      this.lastGracefulLeftovers = leftovers;
      throw new Error(
        `graceful SIGTERM left process group members: ${JSON.stringify(leftovers)}`,
      );
    }
  }

  private appendLog(chunk: Buffer | string): void {
    const text = chunk.toString();
    this.logs += text;
    if (this.logPath !== "") {
      writeFileSync(this.logPath, text, { flag: "a" });
    }
  }

  private observeOwnedMembers(): ProcMember[] {
    if (this.parentPid != null && this.pgid != null &&
      sameIdentity(this.parentIdentity, this.parentPid)) {
      this.ownedMembers = processGroupMembers(this.pgid);
    }
    return this.ownedMembers;
  }

  private async spawnAt(bind: string, expectedBind?: string): Promise<void> {
    const bin = this.requiredBin();
    const engine = process.env.FVOCI_COLLAB_ENGINE?.trim() ?? "";
    if (engine === "" || !existsSync(engine)) {
      throw new Error("FVOCI_COLLAB_ENGINE must point at the built collab-engine helper");
    }
    if (!process.env.DATABASE_APP_URL) {
      throw new Error("DATABASE_APP_URL is required");
    }
    this.logs = "";
    if (this.runDir === "") {
      const root = process.env.FVOCI_E2E_RESULT_DIR ?? tmpdir();
      mkdirSync(root, { recursive: true });
      this.runDir = join(
        root,
        `collab-server-${process.pid}-${Date.now()}-${Math.random().toString(16).slice(2)}`,
      );
      mkdirSync(this.runDir, { recursive: true, mode: 0o700 });
      this.storageDir = join(this.runDir, "storage");
      mkdirSync(this.storageDir, { recursive: true, mode: 0o700 });
    }
    this.logPath = join(this.runDir, "server.log");
    writeFileSync(this.logPath, "", { mode: 0o600 });

    const env = ownedServerChildEnv(bind);
    env.FVOCI_STORAGE_DIR = this.storageDir;
    const child = spawn(bin, [], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });
    if (child.pid == null) {
      throw new Error("fvoci-server spawn produced no pid");
    }
    this.child = child;
    this.pgid = child.pid;
    this.parentPid = child.pid;
    child.stdout?.on("data", (chunk) => this.appendLog(chunk));
    child.stderr?.on("data", (chunk) => this.appendLog(chunk));
    child.once("exit", () => {
      if (this.child === child) this.child = null;
    });
    await delay(20);
    this.parentIdentity = readProcMember(child.pid);
    this.ownedMembers = [];
    this.observeOwnedMembers();

    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      if (this.child !== child) {
        throw new Error(
          `fvoci-server exited before listen bind=${bind} log=${this.logs.slice(-2000)}`,
        );
      }
      if (BIND_FAIL_RE.test(this.logs)) {
        await this.killGroupObserved();
        throw new Error(`fvoci-server failed to bind ${bind}: ${this.logs.slice(-2000)}`);
      }
      const match = LISTEN_RE.exec(this.logs);
      if (match) {
        const url = match[1];
        const bound = url.replace("http://", "");
        if (expectedBind && bound !== expectedBind) {
          await this.killGroupObserved();
          throw new Error(`owned restart bound ${bound}, expected ${expectedBind}`);
        }
        const ready = await fetch(`${url}/api/v1/setup`, { signal: AbortSignal.timeout(1_000) })
          .then((res) => res.status === 200 || res.status === 404)
          .catch(() => false);
        if (ready) {
          this.baseUrl = url;
          this.bind = bound;
          return;
        }
      }
      await delay(100);
    }
    await this.killGroupObserved();
    throw new Error(`fvoci-server did not become ready on ${bind}: ${this.logs.slice(-2000)}`);
  }

  private async killGroupObserved(): Promise<void> {
    const child = this.child;
    const pgid = this.pgid;
    const parentPid = this.parentPid;
    const identity = this.parentIdentity;
    const owners = this.observeOwnedMembers();
    this.child = null;
    this.pgid = null;
    this.parentPid = null;
    this.parentIdentity = null;
    if (pgid == null) return;
    signalOwnedGroup(pgid, owners);
    if (child && child.exitCode == null && child.signalCode == null) {
      await Promise.race([once(child, "exit"), delay(5_000)]);
    }
    const deadline = Date.now() + 5_000;
    let leftovers = processGroupMembers(pgid);
    while (Date.now() < deadline && leftovers.length > 0) {
      await delay(50);
      leftovers = processGroupMembers(pgid);
    }
    if (parentPid != null && sameIdentity(identity, parentPid)) {
      leftovers = processGroupMembers(pgid);
    }
    if (leftovers.length > 0) {
      throw new Error(
        `crash SIGKILL left process group members: ${JSON.stringify(leftovers)}`,
      );
    }
  }
}

export async function startOwnedServer(bind = "127.0.0.1:0"): Promise<OwnedServer> {
  return OwnedServer.start(bind);
}
