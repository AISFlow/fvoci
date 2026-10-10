#!/usr/bin/env bun
// Start one group's database, SMTP sink, server and Playwright process.

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { playwrightChildEnv } from "./child-env.ts";
import { command, commandStatus } from "./proc.ts";
import { redactStartupLog } from "./redact.ts";

function required(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`${name}: ${name} is required`);
    process.exit(1);
  }
  return value;
}

function netMark(message: string) {
  const path = process.env.NET_MARKS_LOG;
  if (!path) return;
  const iso = new Date().toISOString().replace("Z", "");
  const [head, fraction = "000"] = iso.split(".");
  writeFileSync(path, `[${head}.${fraction.padEnd(6, "0").slice(0, 6)}] # fvoci: ${message}\n`, {
    flag: "a",
  });
}

function eventCount(path: string | undefined): number {
  if (!path || !existsSync(path)) return 0;
  const text = readFileSync(path, "utf8");
  if (!text) return 0;
  return text.split("\n").filter((line, index, all) => line !== "" || index < all.length - 1)
    .length;
}

async function settleNetwork() {
  const limitSeconds = 10;
  const quietSeconds = 1;
  const monitorLog = process.env.NET_MONITOR_LOG ?? "";
  const monitorPid = process.env.NET_MONITOR_PID ?? "";
  const say = (message: string) => {
    console.error(`network settle: ${message}`);
    netMark(`network settle: ${message}`);
  };
  const monitorRunning = () => {
    if (!monitorPid || !monitorLog || !existsSync(monitorLog)) return false;
    try {
      process.kill(Number(monitorPid), 0);
      return true;
    } catch {
      return false;
    }
  };
  const tentative = async () => {
    const result = await command(["ip", "-6", "-o", "addr", "show", "tentative", "-dadfailed"], {
      stdout: "pipe",
      stderr: "pipe",
    });
    if (result.status !== 0) throw new Error("ip");
    return [
      ...new Set(
        result.stdout
          .split("\n")
          .map((line) => line.split(/\s+/)[1]?.replace(/:$/, ""))
          .filter((item): item is string => Boolean(item)),
      ),
    ].sort();
  };
  const start = performance.now();
  let checkTentative = true;
  try {
    await tentative();
  } catch {
    checkTentative = false;
    say("cannot list tentative addresses (Error)");
  }
  const watchEvents = monitorRunning();
  if (!watchEvents) say("netlink monitor not running; not checking for recent events");
  if (!checkTentative && !watchEvents) {
    say("skipped");
    return;
  }
  for (;;) {
    const elapsed = (performance.now() - start) / 1000;
    const interfaces = checkTentative ? await tentative() : [];
    const quiet = watchEvents ? Date.now() / 1000 - statMtime(monitorLog) : undefined;
    const events = watchEvents
      ? `; netlink events since the group started: ${eventCount(monitorLog)}`
      : "";
    if (!interfaces.length && (quiet === undefined || quiet >= quietSeconds)) {
      say(`settled after ${elapsed.toFixed(2)} s${events}`);
      return;
    }
    if (elapsed >= limitSeconds) {
      let detail = `tentative: ${interfaces.join(", ") || "none"}`;
      if (quiet !== undefined) detail += `; last netlink event ${quiet.toFixed(2)} s ago`;
      say(
        `warning: host network still changing after ${limitSeconds.toFixed(0)} s (${detail}${events}); continuing`,
      );
      return;
    }
    await commandStatus(["sleep", "0.1"], { stdout: "ignore", stderr: "ignore" });
  }
}

function statMtime(path: string): number {
  return statSync(path).mtimeMs / 1000;
}

async function runPlaywright(args: string[], root: string): Promise<number> {
  try {
    await settleNetwork();
  } catch {
    console.error("warning: network settle check failed; continuing");
  }
  const before = eventCount(process.env.NET_MONITOR_LOG);
  netMark("playwright start");
  const status = await commandStatus(
    [process.execPath, "--bun", "x", "--no-install", "playwright", "test", ...args],
    {
      cwd: join(root, "apps/web"),
      env: playwrightChildEnv(),
      stdout: "inherit",
      stderr: "inherit",
      stdin: "ignore",
    },
  );
  netMark(`playwright exited with status ${status}`);
  const pid = process.env.NET_MONITOR_PID;
  if (pid) {
    try {
      process.kill(Number(pid), 0);
      console.error(
        `network: netlink address/link events while Playwright ran: ${eventCount(process.env.NET_MONITOR_LOG) - before}`,
      );
    } catch {
      // The monitor is already gone.
    }
  }
  return status;
}

function databaseUrls(
  adminUrl: string,
  dbName: string,
  role: string,
  password: string,
): [string, string] {
  const admin = new URL(adminUrl);
  const owner = new URL(adminUrl);
  owner.pathname = `/${dbName}`;
  const host = admin.hostname || "127.0.0.1";
  const port = admin.port || "5432";
  return [
    owner.toString(),
    `postgres://${encodeURIComponent(role)}:${encodeURIComponent(password)}@${host}:${port}/${dbName}`,
  ];
}

async function psql(container: string, args: string[]) {
  const status = await commandStatus(
    ["docker", "exec", "-i", container, "psql", "-U", "postgres", "-v", "ON_ERROR_STOP=1", ...args],
    {
      stdout: "ignore",
      stderr: "inherit",
    },
  );
  if (status !== 0) process.exit(status);
}

async function main() {
  const root = required("ROOT");
  const serverLog = required("SERVER_LOG");
  required("PEPPER");
  const runDir = required("RUN_DIR");
  const profile = process.env.FVOCI_E2E_PROFILE ?? "debug";
  if (profile !== "debug" && profile !== "release") {
    console.error("FVOCI_E2E_PROFILE must be debug or release");
    process.exit(1);
  }
  const cargoTarget = process.env.CARGO_TARGET_DIR ?? join(root, "target");
  const serverBin = join(cargoTarget, profile, "fvoci-server");
  const migrateBin = join(cargoTarget, profile, "fvoci-migrate");
  let server: ReturnType<typeof spawn> | undefined;
  let smtp: ReturnType<typeof spawn> | undefined;
  const stop = async (child: ReturnType<typeof spawn> | undefined) => {
    if (!child?.pid) return;
    try {
      child.kill();
    } catch {
      return;
    }
    await new Promise((resolve) => child.once("close", resolve));
  };
  process.on("exit", () => {
    try {
      server?.kill();
    } catch {
      /* already gone */
    }
    try {
      smtp?.kill();
    } catch {
      /* already gone */
    }
  });
  const startupFailure = (message: string): never => {
    console.error(message);
    if (existsSync(serverLog))
      console.error(redactStartupLog(readFileSync(serverLog, "utf8")).trimEnd());
    process.exit(1);
  };
  netMark("containers ready; preparing database");
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!container) {
    console.error("FVOCI_TEST_PG_CONTAINER: missing test postgres container");
    process.exit(1);
  }
  const dbName = `fvoci_e2e_${randomBytes(8).toString("hex")}`;
  const role = `fvoci_app_${dbName.replaceAll("-", "_")}`;
  const password = randomBytes(16).toString("hex");
  await psql(container, ["-d", "postgres", "-c", `CREATE DATABASE "${dbName}"`]);
  const adminUrl = process.env.TEST_DATABASE_URL;
  if (!adminUrl) {
    console.error("TEST_DATABASE_URL is required");
    process.exit(1);
  }
  const [ownerUrl, appUrl] = databaseUrls(adminUrl, dbName, role, password);
  process.env.DATABASE_URL = ownerUrl;
  process.env.DATABASE_APP_URL = appUrl;
  if ((await commandStatus([migrateBin], { stdout: "ignore", stderr: "inherit" })) !== 0)
    process.exit(1);
  await psql(container, [
    "-d",
    dbName,
    "-c",
    `CREATE ROLE "${role}" LOGIN PASSWORD '${password}' NOSUPERUSER NOBYPASSRLS`,
  ]);
  if (
    (await commandStatus([migrateBin, "--grant-app-role", role], {
      stdout: "ignore",
      stderr: "inherit",
    })) !== 0
  )
    process.exit(1);
  process.env.PASSWORD_PEPPER_KEYS = process.env.PEPPER;
  process.env.PASSWORD_PEPPER_ACTIVE_KEY_ID = "test";
  process.env.ENCRYPTION_KEYS = JSON.stringify({ e2e: randomBytes(32).toString("hex") });
  process.env.ENCRYPTION_ACTIVE_KEY_ID = "e2e";
  process.env.FVOCI_WEBHOOK_ALLOW_TARGETS = "127.0.0.1";
  process.env.FVOCI_BIND = "127.0.0.1:0";
  process.env.RUST_LOG = process.env.RUST_LOG ?? "warn,tower_http=debug";
  process.env.FVOCI_EXTRACT_POLL_SECS = "2";
  process.env.FVOCI_PUBLIC_ORIGIN = "http://127.0.0.1:0";
  if (!process.env.FVOCI_STATIC_DIR) {
    console.error("FVOCI_STATIC_DIR: run-web-e2e.sh must provide isolated static assets");
    process.exit(1);
  }
  const storage = join(runDir, "storage");
  mkdirSync(storage, { recursive: true });
  process.env.FVOCI_STORAGE_DIR = storage;
  process.env.FVOCI_E2E_ADMIN_DATABASE_URL = ownerUrl;
  process.env.FVOCI_E2E_SERVER_BIN = serverBin;
  process.env.FVOCI_E2E_RESULT_DIR = runDir;
  const playwrightOutput = join(runDir, "playwright-output");
  delete process.env.DATABASE_URL;
  delete process.env.FVOCI_MIGRATION_URL;
  const capture = join(runDir, "smtp.jsonl");
  const portFile = join(runDir, "smtp.port");
  writeFileSync(capture, "");
  smtp = spawn(
    process.execPath,
    [join(import.meta.dir, "smtp-sink.ts"), "--capture", capture, "--port-file", portFile],
    { stdio: "inherit", env: playwrightChildEnv() },
  );
  const deadline = Date.now() + 10_000;
  while (!existsSync(portFile) || readFileSync(portFile, "utf8").trim() === "") {
    if (Date.now() >= deadline) {
      console.error("smtp sink did not write port file");
      process.exit(1);
    }
    try {
      process.kill(smtp.pid ?? -1, 0);
    } catch {
      console.error("smtp sink exited before becoming ready");
      process.exit(1);
    }
    await commandStatus(["sleep", "0.05"], { stdout: "ignore", stderr: "ignore" });
  }
  process.env.SMTP_HOST = "127.0.0.1";
  process.env.SMTP_PORT = readFileSync(portFile, "utf8").trim();
  process.env.SMTP_FROM = "noreply@example.com";
  process.env.FVOCI_E2E_SMTP_CAPTURE = capture;
  const args = process.argv.slice(2);
  if (process.env.FVOCI_E2E_PENDING === "1") {
    const status = await runPlaywright(
      ["--config=e2e-pending/collab-playwright.config.ts", `--output=${playwrightOutput}`, ...args],
      root,
    );
    await stop(smtp);
    process.exit(status);
  }
  const removed = [
    "FVOCI_E2E_ADMIN_DATABASE_URL",
    "TEST_DATABASE_URL",
    "FVOCI_TEST_DATABASE_URL",
    "MEILI_MASTER_KEY",
    "FVOCI_MEILI_MASTER_KEY",
    "FVOCI_MEILI_KEY",
  ];
  const serverEnv: NodeJS.ProcessEnv = { ...process.env };
  for (const name of removed) delete serverEnv[name];
  if (process.env.FVOCI_MEILI_URL && process.env.MEILI_MASTER_KEY) {
    const keyFile = join(runDir, "meili-search.key");
    if (
      (await commandStatus([migrateBin, "--ensure-meili-key", keyFile], {
        stdout: "ignore",
        stderr: "inherit",
      })) !== 0
    )
      process.exit(1);
    serverEnv.FVOCI_MEILI_KEY_FILE = keyFile;
  }
  const names = join(runDir, "server-env-names.txt");
  const listed = await command(["bash", "-c", "compgen -e"], {
    env: serverEnv,
    stdout: "pipe",
    stderr: "pipe",
  });
  writeFileSync(
    names,
    listed.stdout.split("\n").filter(Boolean).sort().join("\n") +
      (listed.stdout.trim() ? "\n" : ""),
    { mode: 0o600 },
  );
  process.env.FVOCI_E2E_SERVER_ENV_NAMES = names;
  const logFd = openSync(serverLog, "a");
  server = spawn(serverBin, [], { env: serverEnv, stdio: ["ignore", logFd, logFd] });
  closeSync(logFd);
  const sequence = (await command(["seq", "1", "120"], { stdout: "pipe", stderr: "pipe" })).stdout
    .trim()
    .split("\n");
  let baseUrl = "";
  let ready = false;
  for (const _ of sequence) {
    const log = existsSync(serverLog) ? readFileSync(serverLog, "utf8") : "";
    const line = log.split("\n").find((item) => item.includes("fvoci-server listening on "));
    baseUrl = line ? (line.split("listening on ")[1] ?? "").replace(/\r$/, "") : "";
    try {
      process.kill(server.pid ?? -1, 0);
    } catch {
      startupFailure("server exited during startup");
    }
    if (
      baseUrl &&
      (
        await command(["curl", "-fsS", `${baseUrl}/api/v1/setup`], {
          stdout: "ignore",
          stderr: "ignore",
        })
      ).status === 0
    ) {
      ready = true;
      break;
    }
    await commandStatus(["sleep", "0.25"], { stdout: "ignore", stderr: "ignore" });
  }
  if (!ready)
    startupFailure("server did not become ready within 30s (GET /api/v1/setup never succeeded)");
  netMark("server ready");
  process.env.PLAYWRIGHT_BASE_URL = baseUrl;
  const status = await runPlaywright([`--output=${playwrightOutput}`, ...args], root);
  await stop(server);
  await stop(smtp);
  process.exit(status);
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "web e2e inner failed");
    process.exit(1);
  });
}
