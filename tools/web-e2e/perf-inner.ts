#!/usr/bin/env bun
// One perf run: fresh DB and app role, release server on 127.0.0.1:0, Playwright.

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { playwrightChildEnv } from "./child-env.ts";
import { command, commandStatus } from "./proc.ts";
import { redactStartupLog } from "./redact.ts";

function required(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`${name}: required`);
    process.exit(1);
  }
  return value;
}

async function main() {
  const root = required("ROOT");
  const runDir = required("RUN_DIR");
  const release = required("RELEASE");
  const output = required("FVOCI_PERF_OUT");
  const dataset = required("FVOCI_PERF_DATASET");
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!container) {
    console.error("FVOCI_TEST_PG_CONTAINER: missing test postgres container");
    process.exit(1);
  }
  const serverLog = join(runDir, "server.log");
  let server: ReturnType<typeof spawn> | undefined;
  process.on("exit", () => {
    try {
      server?.kill();
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
  const psql = async (args: string[]) => {
    const status = await commandStatus(
      [
        "docker",
        "exec",
        "-i",
        container,
        "psql",
        "-U",
        "postgres",
        "-v",
        "ON_ERROR_STOP=1",
        ...args,
      ],
      { stdout: "pipe", stderr: "inherit" },
    );
    if (status !== 0) process.exit(status);
    return (
      await command(
        [
          "docker",
          "exec",
          "-i",
          container,
          "psql",
          "-U",
          "postgres",
          "-v",
          "ON_ERROR_STOP=1",
          ...args,
        ],
        { stdout: "pipe", stderr: "pipe" },
      )
    ).stdout;
  };
  const dbName = `fvoci_perf_${randomBytes(8).toString("hex")}`;
  const role = `fvoci_app_${dbName}`;
  const password = randomBytes(16).toString("hex");
  await commandStatus(
    [
      "docker",
      "exec",
      "-i",
      container,
      "psql",
      "-U",
      "postgres",
      "-v",
      "ON_ERROR_STOP=1",
      "-d",
      "postgres",
      "-c",
      `CREATE DATABASE "${dbName}"`,
    ],
    { stdout: "ignore", stderr: "inherit" },
  );
  const adminUrl = process.env.TEST_DATABASE_URL;
  if (!adminUrl) process.exit(1);
  const admin = new URL(adminUrl);
  const owner = new URL(adminUrl);
  owner.pathname = `/${dbName}`;
  const appUrl = `postgres://${encodeURIComponent(role)}:${encodeURIComponent(password)}@${admin.hostname || "127.0.0.1"}:${admin.port || "5432"}/${dbName}`;
  process.env.DATABASE_URL = owner.toString();
  process.env.DATABASE_APP_URL = appUrl;
  if (
    (await commandStatus([join(release, "fvoci-migrate")], {
      stdout: "ignore",
      stderr: "inherit",
    })) !== 0
  )
    process.exit(1);
  await commandStatus(
    [
      "docker",
      "exec",
      "-i",
      container,
      "psql",
      "-U",
      "postgres",
      "-v",
      "ON_ERROR_STOP=1",
      "-d",
      dbName,
      "-c",
      `CREATE ROLE "${role}" LOGIN PASSWORD '${password}' NOSUPERUSER NOBYPASSRLS`,
    ],
    { stdout: "ignore", stderr: "inherit" },
  );
  if (
    (await commandStatus([join(release, "fvoci-migrate"), "--grant-app-role", role], {
      stdout: "ignore",
      stderr: "inherit",
    })) !== 0
  )
    process.exit(1);
  const version = (await psql(["-d", "postgres", "-tAc", "SHOW server_version"])).replace(
    /\s/g,
    "",
  );
  process.env.PASSWORD_PEPPER_KEYS =
    '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}';
  process.env.PASSWORD_PEPPER_ACTIVE_KEY_ID = "test";
  process.env.ENCRYPTION_KEYS = JSON.stringify({ perf: randomBytes(32).toString("hex") });
  process.env.ENCRYPTION_ACTIVE_KEY_ID = "perf";
  process.env.FVOCI_BIND = "127.0.0.1:0";
  process.env.FVOCI_PUBLIC_ORIGIN = "http://127.0.0.1:0";
  process.env.FVOCI_STATIC_DIR = join(runDir, "static");
  process.env.FVOCI_STORAGE_DIR = join(runDir, "storage");
  mkdirSync(process.env.FVOCI_STORAGE_DIR, { recursive: true });
  process.env.FVOCI_E2E_ADMIN_DATABASE_URL = process.env.DATABASE_URL;
  delete process.env.DATABASE_URL;
  delete process.env.FVOCI_MIGRATION_URL;
  const logFd = openSync(serverLog, "a");
  server = spawn(join(release, "fvoci-server"), [], {
    env: process.env,
    stdio: ["ignore", logFd, logFd],
  });
  closeSync(logFd);
  const sequence = (await command(["seq", "1", "120"], { stdout: "pipe" })).stdout
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
  const tag = (process.env.FVOCI_PERF_TAG ?? "").replace(/[^A-Za-z0-9-]/g, "");
  const grep = process.env.FVOCI_PERF_GREP;
  const grepArgs = grep ? ["--grep", grep] : [];
  if (grep && !tag) {
    console.error("FVOCI_PERF_GREP requires FVOCI_PERF_TAG");
    process.exit(1);
  }
  const started = new Date();
  const iso = `${started
    .toISOString()
    .replace("Z", "")
    .replace(/\.(\d{3})$/, ".$1000")}+00:00`;
  writeFileSync(
    join(output, `run-${dataset}${tag}.json`),
    `${JSON.stringify({ dataset_run_started: iso, postgres_server_version: version, network: "loopback 127.0.0.1", server: "release fvoci-server (source build)" }, null, 1)}\n`,
  );
  const status = await commandStatus(
    [
      process.execPath,
      "--bun",
      "x",
      "--no-install",
      "playwright",
      "test",
      "--config=e2e/perf/perf.config.ts",
      ...grepArgs,
    ],
    {
      cwd: join(root, "apps/web"),
      env: playwrightChildEnv({
        ...process.env,
        PLAYWRIGHT_BASE_URL: baseUrl,
        FVOCI_PERF_RUN_DIR: runDir,
        CARGO_TARGET_DIR: join(runDir, "fixture-target"),
      }),
      stdout: "inherit",
      stderr: "inherit",
    },
  );
  const warnings = (existsSync(serverLog) ? readFileSync(serverLog, "utf8") : "")
    .split("\n")
    .filter((line) => / (WARN|ERROR) /.test(line)).length;
  writeFileSync(join(output, `server-warn-error-count-${dataset}${tag}.txt`), `${warnings}\n`);
  if (process.env.FVOCI_PERF_KEEP_SERVER_LOG === "1")
    writeFileSync(
      join(output, `server-log-${dataset}${tag}.txt`),
      redactStartupLog(readFileSync(serverLog, "utf8")),
    );
  try {
    server.kill();
  } catch {
    /* already gone */
  }
  process.exit(status);
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "perf inner failed");
    process.exit(1);
  });
}
