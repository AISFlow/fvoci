#!/usr/bin/env bun
// One isolated web e2e group. Fresh DB, app, server and storage per invocation.

import { spawn } from "node:child_process";
import {
  chmodSync,
  closeSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { evidenceDirectory, groupLabel, timerInvocations } from "./labels.ts";
import { commandStatus } from "./proc.ts";
import { redactServerLog } from "./redact.ts";
import { summarizeFile } from "./trace-summary.ts";

function stamp(): string {
  const iso = new Date().toISOString().replace("Z", "");
  const [head, fraction = "000"] = iso.split(".");
  return `${head}.${fraction.padEnd(6, "0").slice(0, 6)}`;
}

function netMark(path: string, message: string) {
  try {
    writeFileSync(path, `[${stamp()}] # fvoci: ${message}\n`, { flag: "a" });
  } catch {
    // A gone reader must not skip retention.
  }
}

function walk(directory: string, predicate: (path: string) => boolean): string[] {
  if (!existsSync(directory)) return [];
  const found: string[] = [];
  const visit = (current: string, depth: number) => {
    for (const name of readdirSync(current)) {
      const path = join(current, name);
      const stat = statSync(path);
      if (stat.isDirectory()) visit(path, depth + 1);
      else if (predicate(path) && depth >= 2) found.push(path);
    }
  };
  visit(directory, 0);
  return found;
}

export async function retainFailureArtifacts(input: {
  runDir: string;
  serverLog: string;
  netMonitorLog: string;
  netMarksLog: string;
  label: string;
  tempDir?: string;
}): Promise<string> {
  const retainDir = mkdtempSync(
    join(input.tempDir ?? process.env.TMPDIR ?? tmpdir(), "fvoci-collab-e2e-fail-"),
  );
  chmodSync(retainDir, 0o700);
  const output = process.env.GITHUB_OUTPUT;
  if (output) {
    writeFileSync(output, `failure-artifacts=${retainDir}\nfailure-group=${input.label}\n`, {
      flag: "a",
    });
  }
  const playwright = join(input.runDir, "playwright-output");
  if (existsSync(playwright) && readdirSync(playwright).length)
    cpSync(playwright, join(retainDir, "playwright-output"), { recursive: true });
  if (existsSync(input.serverLog))
    writeFileSync(
      join(retainDir, "server.log"),
      redactServerLog(readFileSync(input.serverLog, "utf8")),
    );
  mkdirSync(join(retainDir, "owned-server"), { recursive: true });
  for (const log of walk(input.runDir, (path) => basename(path) === "server.log")) {
    writeFileSync(
      join(retainDir, "owned-server", `${basename(join(log, ".."))}.log`),
      redactServerLog(readFileSync(log, "utf8")),
    );
  }
  const nets = [input.netMonitorLog, input.netMarksLog].filter((path) => existsSync(path));
  if (nets.length) {
    const sorted = Bun.spawn(["sort", "-s", "-k1,1", ...nets], {
      env: { ...process.env, LC_ALL: "C" },
      stdout: "pipe",
      stderr: "pipe",
    });
    const text = await new Response(sorted.stdout).text();
    writeFileSync(
      join(retainDir, "net-events.log"),
      redactServerLog(
        `# host netlink address/link events (ip -o -tshort monitor address link, UTC) and group markers\n${text}`,
      ),
    );
  }
  const findTraces = (directory: string): string[] => {
    if (!existsSync(directory)) return [];
    const traces: string[] = [];
    const visit = (current: string) => {
      for (const name of readdirSync(current)) {
        const path = join(current, name);
        if (statSync(path).isDirectory()) visit(path);
        else if (name === "trace.zip") traces.push(path);
      }
    };
    visit(directory);
    return traces;
  };
  for (const trace of findTraces(retainDir)) {
    const dest = join(trace, "..", "browser-summary.txt");
    try {
      writeFileSync(dest, summarizeFile(trace));
    } catch {
      writeFileSync(
        dest,
        "trace summary failed; reproduce the group locally to inspect its trace.zip\n",
      );
      console.error(`could not summarize ${basename(join(trace, ".."))}/trace.zip`);
    }
  }
  console.error(`retained failure artifacts for group ${input.label} in ${retainDir}`);
  return retainDir;
}

async function main() {
  const root = process.env.ROOT;
  const cargoTarget = process.env.CARGO_TARGET_DIR;
  if (!root) throw new Error("ROOT: ROOT is required");
  if (!cargoTarget) throw new Error("CARGO_TARGET_DIR: CARGO_TARGET_DIR is required");
  const args = process.argv.slice(2);
  const splits = timerInvocations(args);
  if (splits) {
    for (const extra of splits) {
      const status = await commandStatus([process.execPath, import.meta.path, ...args, ...extra], {
        env: process.env,
        stdout: "inherit",
        stderr: "inherit",
        stdin: "inherit",
      });
      if (status !== 0) process.exit(status);
    }
    process.exit(0);
  }
  const label = groupLabel(args);
  const runDir = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "fvoci-web-e2e-"));
  process.env.FVOCI_W5_EVIDENCE_DIR = evidenceDirectory(runDir);
  const serverLog = join(runDir, "server.log");
  const pepper = '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}';
  const netMonitorLog = join(runDir, "net-monitor.log");
  const netMarksLog = join(runDir, "net-marks.log");
  let monitor: ReturnType<typeof spawn> | undefined;
  let status = 0;
  const stopMonitor = async () => {
    if (!monitor?.pid) return;
    try {
      monitor.kill();
    } catch {
      // Already gone.
    }
    await new Promise((resolve) => monitor?.once("close", resolve));
    monitor = undefined;
  };
  try {
    if (!existsSync(join(root, "apps/web/dist"))) {
      console.error("missing apps/web/dist; build web assets before running groups");
      status = 1;
      return;
    }
    cpSync(join(root, "apps/web/dist"), join(runDir, "static"), { recursive: true });
    console.error(`=== web e2e group: ${label} ===`);
    writeFileSync(netMonitorLog, "");
    if (!Bun.which("ip")) {
      netMark(netMarksLog, "ip (iproute2) not found; no netlink event log");
      console.error(`warning: ip (iproute2) not found; no netlink event log for group ${label}`);
    } else {
      const fd = openSync(netMonitorLog, "a");
      monitor = spawn("ip", ["-o", "-tshort", "monitor", "address", "link"], {
        env: { ...process.env, TZ: "UTC" },
        stdio: ["ignore", fd, fd],
      });
      monitor.unref?.();
      closeSync(fd);
      netMark(netMarksLog, `netlink monitor started (ip monitor address link, pid ${monitor.pid})`);
    }
    netMark(netMarksLog, "starting test containers (postgres, meilisearch)");
    status = await commandStatus(
      [
        "bash",
        join(root, "scripts/start-test-postgres.sh"),
        "bash",
        join(root, "scripts/start-test-meili.sh"),
        "env",
        `RUN_DIR=${runDir}`,
        `SERVER_LOG=${serverLog}`,
        `PEPPER=${pepper}`,
        `ROOT=${root}`,
        `CARGO_TARGET_DIR=${cargoTarget}`,
        `FVOCI_STATIC_DIR=${join(runDir, "static")}`,
        `NET_MONITOR_LOG=${netMonitorLog}`,
        `NET_MARKS_LOG=${netMarksLog}`,
        `NET_MONITOR_PID=${monitor?.pid ?? ""}`,
        process.execPath,
        join(import.meta.dir, "inner.ts"),
        ...args,
      ],
      { stdin: "ignore", stdout: "inherit", stderr: "inherit" },
    );
  } finally {
    await stopMonitor();
    netMark(netMarksLog, `group exiting with status ${status}`);
    if (status !== 0) {
      try {
        await retainFailureArtifacts({ runDir, serverLog, netMonitorLog, netMarksLog, label });
      } catch {
        // Retention must not replace the group status.
      }
    }
    rmSync(runDir, { recursive: true, force: true });
  }
  process.exit(status);
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "web e2e group failed");
    process.exit(1);
  });
}
