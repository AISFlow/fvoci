// Prints the perf measurement environment as JSON: host and toolchain facts
// only. No environment variable is read or recorded, so secrets and session
// tokens in the caller's environment never reach the evidence file.
//
//   bun tools/perf/capture-env.ts <repo-root> <server-binary> <collab-engine-binary>
import { spawnSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import os from "node:os";
import { join } from "node:path";

const COMMAND_TIMEOUT_MS = 30_000;

// Raw probe results; every command result is trimmed stdout or "unavailable: <reason>".
export interface Probes {
  gitHead: string;
  gitStatus: string | null;
  kernel: string;
  osRelease: string;
  lscpu: string;
  logicalCpus: number;
  meminfo: string;
  loadavg: number[];
  dockerServer: string;
  dockerInfo: string;
  bun: string;
  playwright: string;
  serverBytes: number;
  collabBytes: number;
  rustc: string;
}

function fieldAfter(text: string, prefix: string, separator: string): string | undefined {
  const line = text.split("\n").find((l) => l.startsWith(prefix));
  if (line === undefined) return undefined;
  const at = line.indexOf(separator);
  return at < 0 ? undefined : line.slice(at + separator.length);
}

export function cpuModel(lscpu: string): string {
  return fieldAfter(lscpu, "Model name", ":")?.trim() ?? "";
}

export function osName(osRelease: string): string {
  return (fieldAfter(osRelease, "PRETTY_NAME", "=")?.trim() ?? "").replace(/^"+|"+$/g, "");
}

export function memTotal(meminfo: string): string | null {
  return fieldAfter(meminfo, "MemTotal", ":")?.trim() ?? null;
}

// null when `git status` itself failed: an unknown count is not a clean tree.
export function lineCount(text: string | null): number | null {
  if (text === null) return null;
  return text === "" ? 0 : text.split(/\r\n|\r|\n/).length;
}

export function renderEnvironment(p: Probes): string {
  const record = {
    git_head: p.gitHead,
    git_dirty_paths: lineCount(p.gitStatus),
    build_kind: "source build (release cargo + production vite); not a published image",
    published_image_digest: null,
    kernel: p.kernel,
    os: osName(p.osRelease),
    cpu_model: cpuModel(p.lscpu),
    logical_cpus: p.logicalCpus,
    mem_total: memTotal(p.meminfo),
    loadavg_at_start: p.loadavg,
    docker_server: p.dockerServer,
    docker_ncpu_mem: p.dockerInfo,
    docker_container_limits: "none set (containers share the host)",
    bun: p.bun,
    playwright: p.playwright,
    server_binary_bytes: p.serverBytes,
    collab_engine_binary_bytes: p.collabBytes,
    rustc: p.rustc,
  };
  return `${JSON.stringify(record, null, 1)}\n`;
}

export type ProbeResult = { ok: true; out: string } | { ok: false; reason: string };

// A probe failure (spawn error, timeout, signal, non-zero exit) is recorded,
// never turned into an empty or partial value.
export function probe(cmd: string[], cwd?: string, timeoutMs = COMMAND_TIMEOUT_MS): ProbeResult {
  const [file, ...args] = cmd;
  if (file === undefined) throw new Error("empty command");
  const result = spawnSync(file, args, {
    cwd,
    encoding: "utf8",
    timeout: timeoutMs,
    killSignal: "SIGKILL",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const error: NodeJS.ErrnoException | undefined = result.error;
  if (error) return { ok: false, reason: error.code ?? error.name };
  if (result.signal) return { ok: false, reason: `signal ${result.signal}` };
  if (result.status !== 0) return { ok: false, reason: `exit ${String(result.status)}` };
  return { ok: true, out: result.stdout.trim() };
}

export function run(cmd: string[], cwd?: string, timeoutMs = COMMAND_TIMEOUT_MS): string {
  const result = probe(cmd, cwd, timeoutMs);
  return result.ok ? result.out : `unavailable: ${result.reason}`;
}

export function collect(root: string, server: string, collab: string): Probes {
  const status = probe(["git", "status", "--porcelain"], root);
  return {
    gitHead: run(["git", "rev-parse", "HEAD"], root),
    gitStatus: status.ok ? status.out : null,
    kernel: os.release(),
    osRelease: readFileSync("/etc/os-release", "utf8"),
    lscpu: run(["lscpu"]),
    logicalCpus: os.cpus().length,
    meminfo: readFileSync("/proc/meminfo", "utf8"),
    loadavg: os.loadavg(),
    dockerServer: run(["docker", "version", "--format", "{{.Server.Version}}"]),
    dockerInfo: run(["docker", "info", "--format", "{{.NCPU}} cpus {{.MemTotal}} bytes"]),
    bun: run(["bun", "--version"]),
    playwright: run(
      ["bun", "--bun", "x", "--no-install", "playwright", "--version"],
      join(root, "apps/web"),
    ),
    serverBytes: statSync(server).size,
    collabBytes: statSync(collab).size,
    rustc: run(["rustc", "--version"]),
  };
}

if (import.meta.main) {
  const [root, server, collab] = process.argv.slice(2);
  if (!root || !server || !collab) {
    console.error("usage: capture-env.ts <repo-root> <server-binary> <collab-engine-binary>");
    process.exit(2);
  }
  process.stdout.write(renderEnvironment(collect(root, server, collab)));
}
