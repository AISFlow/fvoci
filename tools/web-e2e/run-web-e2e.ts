// Checks scripts/run-web-e2e.sh runs as `bun tools/web-e2e/run-web-e2e.ts
// <command> ...`: committed API qualification, shard plan line parsing, and
// the selected companion's SQLite prefix containment, private diagnostics
// directory and launcher stage receipt. A refusal prints one stderr line and
// exits 1; a usage error exits 2.
import {
  appendFileSync,
  chmodSync,
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  statSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { randomBytes, createHash } from "node:crypto";
import process from "node:process";
import { validateSpecRelpath, discoverGroups, assignShards, verifyPlan } from "./groups.ts";
import { databaseUrls } from "./database-urls.ts";

export const CI_WEB_IMAGE =
  "ghcr.io/aisflow/fvoci-ci-web@sha256:036797cb3fb3d3ae06e0d19de6451f0dda20a704ef0c1d98688c7f7d63de0c15";

// This first ordinary-browser slice retains the complete registered shard.
// The other shards still have host Docker/psql fixture oracles to migrate;
// they are never silently run with a socket or with those assertions skipped.
export function containerShard(value: string | undefined): string {
  if (!value || !["1", "2", "4", "6", "7"].includes(value))
    fail(
      "shared containers support complete shards 1,2,4,6,7; timer/archive/service-ledger fixtures retain their host boundary",
    );
  return value;
}

async function command(argv: string[], cwd: string, env = process.env): Promise<number> {
  const child = Bun.spawn(["setsid", ...argv], {
    cwd,
    env,
    stdin: "ignore",
    stdout: "inherit",
    stderr: "inherit",
  });
  const signal = (name: NodeJS.Signals) => {
    try {
      process.kill(-child.pid, name);
    } catch {
      /* already retired */
    }
  };
  const interrupt = () => {
      signal("SIGINT");
    },
    terminate = () => {
      signal("SIGTERM");
    };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  try {
    return await child.exited;
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", terminate);
  }
}

async function stage(name: string, argv: string[], root: string, env = process.env): Promise<void> {
  const started = performance.now();
  process.stderr.write(`web-e2e stage=${name} started\n`);
  const status = await command(argv, root, env);
  process.stderr.write(
    `web-e2e stage=${name} elapsed_seconds=${((performance.now() - started) / 1000).toFixed(3)} exit=${String(status)}\n`,
  );
  if (status !== 0) throw new RuntimeExit(status);
}

class RuntimeExit extends Error {
  constructor(readonly status: number) {
    super("web e2e command failed");
  }
}

async function browserServices(
  root: string,
  batch: string,
  network: string,
  body: (env: NodeJS.ProcessEnv) => Promise<number>,
): Promise<number> {
  const id = randomBytes(16).toString("hex"),
    pg = "fvoci-rust-test-pg-" + id,
    meili = "fvoci-rust-test-meili-" + id,
    password = randomBytes(24).toString("hex"),
    key = randomBytes(16).toString("hex"),
    envFile = join(batch, "services-" + id);
  const owned: string[] = [];
  let status = 1,
    created = false;
  let interrupted = 0;
  const interrupt = () => {
      interrupted = 130;
    },
    terminate = () => {
      interrupted = 143;
    };
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", terminate);
  const inspect = (name: string) =>
    checkedOutput(
      [
        "docker",
        "inspect",
        "--format",
        "{{.State.Running}} {{.State.ExitCode}} {{.State.OOMKilled}}",
        name,
      ],
      root,
    );
  try {
    writeFileSync(envFile + "-pg.env", `POSTGRES_PASSWORD=${password}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    writeFileSync(
      envFile + "-meili.env",
      `MEILI_MASTER_KEY=${key}\nMEILI_NO_ANALYTICS=true\nMEILI_ENV=production\n`,
      { flag: "wx", mode: 0o600 },
    );
    checkedOutput(["docker", "network", "create", "--internal", network], root);
    created = true;
    for (const [name, alias, image, ready] of [
      [
        pg,
        "fvoci-pg",
        "postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336",
        ["pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
      ],
      [
        meili,
        "fvoci-meili",
        "getmeili/meilisearch:v1.55.0@sha256:ff546679970126f8822e15d1b6a06bfbff1f2ce2e43f810cfb5258012f92de5c",
        ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:7700/health"],
      ],
    ] as const) {
      owned.push(name);
      checkedOutput(
        [
          "docker",
          "run",
          "--pull=never",
          "-d",
          "--name",
          name,
          "--network",
          network,
          "--network-alias",
          alias,
          "--cpus=1",
          "--memory=2g",
          "--pids-limit=256",
          "--label",
          "fvoci.test-run=" + id,
          "--env-file",
          envFile + (name === pg ? "-pg.env" : "-meili.env"),
          image,
          ...(name === pg ? ["postgres", "-c", "max_connections=150"] : []),
        ],
        root,
      );
      const deadline = performance.now() + 30_000;
      for (;;) {
        if (interrupted) throw new RuntimeExit(interrupted);
        const probe = Bun.spawnSync(["docker", "exec", name, ...ready], {
          stdout: "ignore",
          stderr: "ignore",
        });
        if (probe.exitCode === 0) break;
        if (performance.now() >= deadline)
          fail("owned browser service readiness failed within 30s");
        await Bun.sleep(1000);
      }
    }
    if (
      !/^18[0-9]{4}$/.test(
        checkedOutput(
          ["docker", "exec", pg, "psql", "-U", "postgres", "-tAc", "SHOW server_version_num"],
          root,
        ),
      )
    )
      fail("owned PostgreSQL version mismatch");
    status = await body({
      TEST_DATABASE_URL: `postgres://postgres:${password}@fvoci-pg:5432/postgres`,
      FVOCI_TEST_PG_CONTAINER: pg,
      FVOCI_TEST_MEILI_CONTAINER: meili,
      FVOCI_MEILI_KEY: key,
      MEILI_MASTER_KEY: key,
    });
    if (interrupted && status === 0) status = interrupted;
  } finally {
    for (const name of owned.reverse()) {
      let retired = true;
      const present = Bun.spawnSync(["docker", "inspect", "--format", "{{.Id}}", name], {
        stdout: "ignore",
        stderr: "ignore",
      });
      if (present.exitCode === 0) {
        if (
          Bun.spawnSync(["docker", "stop", "--time", "10", name], {
            stdout: "ignore",
            stderr: "ignore",
          }).exitCode !== 0
        )
          retired = false;
        try {
          if (inspect(name) !== `false ${name === pg ? "0" : "143"} false`) retired = false;
        } catch {
          retired = false;
        }
        if (
          Bun.spawnSync(["docker", "rm", "-v", name], { stdout: "ignore", stderr: "ignore" })
            .exitCode !== 0
        )
          retired = false;
      } else retired = false;
      if (!retired) {
        process.stderr.write("container-browser owned service normal cleanup failed\n");
        if (status === 0) status = 1;
      } else
        process.stderr.write(
          `container-browser service=${name === pg ? "postgres" : "meili"} normal_cleanup=PASS\n`,
        );
    }
    for (const path of [envFile + "-pg.env", envFile + "-meili.env"])
      if (existsSync(path)) rmSync(path);
    if (
      created &&
      Bun.spawnSync(["docker", "network", "rm", network], { stdout: "ignore", stderr: "ignore" })
        .exitCode !== 0
    ) {
      process.stderr.write("container-browser owned network cleanup failed\n");
      if (status === 0) status = 1;
    }
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", terminate);
  }
  return status;
}

// The image owns compiler/browser packages; xtask remains the only SQLite recipe.
// Its four verified exports are data, never evaluated as shell input.
export function sqliteEnvironment(text: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of text.trim().split("\n")) {
    const match =
      /^export (SQLITE3_INCLUDE_DIR|SQLITE3_LIB_DIR|SQLITE3_STATIC|SQLITE3_NO_PKG_CONFIG)=(?:'([^'\r\n]*)'|([A-Za-z0-9_./-]+))$/.exec(
        line,
      );
    if (!match || !match[1] || Object.hasOwn(result, match[1]))
      fail("invalid verified SQLite environment");
    result[match[1]] = match[2] ?? match[3] ?? "";
  }
  if (Object.keys(result).length !== 4) fail("incomplete verified SQLite environment");
  if (result.SQLITE3_STATIC !== "1" || result.SQLITE3_NO_PKG_CONFIG !== "1")
    fail("invalid static SQLite policy");
  return result;
}

async function browserPhase(root: string, phase: string, shards: string[]): Promise<number> {
  if (process.env.FVOCI_WEB_CONTAINER_LOCAL !== "1" || !["prepare", "consume"].includes(phase))
    fail("ordinary container phase required");
  process.env.ROOT = root;
  process.env.FVOCI_COLLAB_ENGINE = join(root, "crates/collab-engine/target/debug/collab-engine");
  process.env.FVOCI_WEB_BUILD_PHASE = phase;
  try {
    if (phase === "prepare") {
      if (shards.length || existsSync(join(root, "apps/web/dist")))
        fail("producer requires absent dist and no shard");
      await stage("cargo-fetch", ["cargo", "fetch", "--locked"], root);
      await stage(
        "engine-fetch",
        [
          "cargo",
          "fetch",
          "--locked",
          "--manifest-path",
          join(root, "crates/collab-engine/Cargo.toml"),
        ],
        root,
      );
      // Bun owns lifecycle scripts' node executable without a shell harness.
      await stage(
        "dependencies",
        [process.execPath, "--bun", "install", "--frozen-lockfile"],
        root,
      );
      await stage(
        "browser-version",
        [process.execPath, "--bun", "x", "--no-install", "playwright", "--version"],
        join(root, "apps/web"),
      );
      await stage(
        "xtask-build",
        [
          "cargo",
          "build",
          "--locked",
          "--jobs",
          "2",
          "--manifest-path",
          "xtask/Cargo.toml",
          "--target-dir",
          "xtask/target",
        ],
        root,
      );
      await stage(
        "sqlite-prepare",
        [
          join(root, "xtask/target/debug/xtask"),
          "sqlite-ci",
          "--parent",
          "/fvoci-run/sqlite",
          "--env-file",
          "/fvoci-run/sqlite.env",
        ],
        root,
      );
    }
    Object.assign(process.env, sqliteEnvironment(readFileSync("/fvoci-run/sqlite.env", "utf8")));
    qualifyCommittedApi(
      root,
      {
        shard: phase === "consume" ? (shards[0] ?? "") : "",
        selected: false,
        browserPhase: phase as "prepare" | "consume",
      },
      process.env,
    );
    const handoff = (args: string[]) => [
      process.execPath,
      "--no-env-file",
      join(root, "tools/selected-backend-ci/handoff.ts"),
      ...args,
    ];
    if (phase === "prepare") {
      await stage("browser-input-before", handoff(["browser-before"]), root);
      await stage("web-build", [process.execPath, "--bun", "run", "build"], join(root, "apps/web"));
      qualifyCommittedApi(
        root,
        { shard: "", selected: false, browserPhase: "prepare" },
        process.env,
      );
      await stage("fixture-build", handoff(["browser-stage", "fixture"]), root);
      await stage("default-server-build", handoff(["browser-stage", "default"]), root);
      await stage("worker-build", handoff(["browser-stage", "engine"]), root, {
        ...process.env,
        CARGO_TARGET_DIR: join(root, "crates/collab-engine/target"),
      });
      await stage("browser-input-after", handoff(["browser-after"]), root);
      await stage("browser-handoff-export", handoff(["export"]), root);
    } else {
      if (shards.length !== 1) fail("consumer requires exactly one complete shard");
      const shard = containerShard(shards[0]);
      const groups = discoverGroups(),
        plan = assignShards(groups, 8);
      verifyPlan(undefined, 8);
      await stage("browser-handoff-consume", handoff(["consume"]), root);
      for (const specs of plan[Number(shard)] ?? [])
        await stage(
          "browser-group",
          ["bash", join(root, "scripts/web-e2e-run-group.sh"), ...specs],
          root,
        );
      process.stderr.write(`=== web e2e shard ${shard}: all groups passed ===\n`);
    }
    return 0;
  } catch (error) {
    if (error instanceof RuntimeExit) return error.status;
    throw error;
  }
}

/** The existing group runtime, shared by ordinary host and image consumers. */
async function groupRuntime(argv: string[]): Promise<number> {
  const root = process.env.ROOT,
    run = process.env.RUN_DIR,
    log = process.env.SERVER_LOG,
    pepper = process.env.PEPPER;
  if (!root || !run || !log || !pepper) fail("ROOT, RUN_DIR, SERVER_LOG and PEPPER are required");
  const profile = process.env.FVOCI_E2E_PROFILE ?? "debug";
  if (!["debug", "release"].includes(profile)) fail("FVOCI_E2E_PROFILE must be debug or release");
  const target = process.env.CARGO_TARGET_DIR ?? join(root, "target"),
    server = join(target, profile, "fvoci-server"),
    migrate = join(target, profile, "fvoci-migrate");
  const env = { ...process.env };
  let step = "prepare",
    status = 0,
    interrupted = 0;
  let db = "",
    role = "",
    index = "",
    keyFile = "";
  type Child = ReturnType<typeof Bun.spawn>;
  let serverChild: Child | undefined, smtp: Child | undefined, playwright: Child | undefined;
  const mark = (message: string) => {
    if (env.NET_MARKS_LOG)
      appendFileSync(
        env.NET_MARKS_LOG,
        `[${new Date().toISOString().replace("Z", "")}] # fvoci: ${message}\n`,
      );
  };
  const send = (child: Child | undefined, signal: NodeJS.Signals) => {
    if (child) {
      try {
        process.kill(-child.pid, signal);
      } catch {
        /* already retired */
      }
    }
  };
  const signals: [NodeJS.Signals, number][] = [
    ["SIGHUP", 129],
    ["SIGINT", 130],
    ["SIGTERM", 143],
    ["SIGPIPE", 141],
  ];
  const handlers = signals.map(([signal, code]) => {
    const handler = () => {
      if (!interrupted) interrupted = code;
      send(playwright, signal);
    };
    process.on(signal, handler);
    return [signal, handler] as const;
  });
  const checkInterrupt = () => {
    if (interrupted) throw new RuntimeExit(interrupted);
  };
  const call = async (args: string[], variables = env) => {
    checkInterrupt();
    const exit = await command(args, root, variables);
    checkInterrupt();
    if (exit !== 0) throw new RuntimeExit(exit);
  };
  const pause = async (seconds: string) => {
    await call(["sleep", seconds]);
  };
  const database = async (action: string) => {
    const child = Bun.spawn(
      [
        "timeout",
        "60",
        "bun",
        "--no-env-file",
        join(root, "tools/web-e2e/database-urls.ts"),
        action,
      ],
      {
        cwd: root,
        env: { ...env, DB_NAME: db, ROLE_NAME: role },
        stdin: "ignore",
        stdout: "inherit",
        stderr: "inherit",
      },
    );
    const exit = await child.exited;
    if (exit !== 0) throw new RuntimeExit(exit);
  };
  const alive = (child: Child) => child.exitCode === null && child.signalCode === null;
  const spawnGroup = (
    args: string[],
    variables: NodeJS.ProcessEnv,
    stdout: "inherit" | number = "inherit",
  ) =>
    Bun.spawn(["setsid", ...args], {
      cwd: root,
      env: variables,
      stdin: "ignore",
      stdout,
      stderr: stdout,
    });
  const runningGroup = (child: Child) => {
    try {
      process.kill(-child.pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  const stop = async (name: string, child: Child | undefined) => {
    if (!child) return;
    send(child, "SIGTERM");
    let deadline = performance.now() + 10_000;
    while (runningGroup(child) && performance.now() < deadline) await Bun.sleep(100);
    if (runningGroup(child)) {
      process.stderr.write(
        `web-e2e cleanup: ${name} process group ${String(child.pid)} still running 10 s after SIGTERM; sending SIGKILL\n`,
      );
      send(child, "SIGKILL");
      deadline = performance.now() + 5_000;
      while (runningGroup(child) && performance.now() < deadline) await Bun.sleep(100);
    }
    if (runningGroup(child)) throw new CheckError("process group survived SIGKILL");
    await child.exited;
  };
  const meili = async (method: string, path: string) => {
    const child = Bun.spawn(
      ["curl", "-sS", "--max-time", "30", "-X", method, "-w", "\\n%{http_code}", "-K", "-"],
      {
        env,
        stdin: new Blob([
          `url = "${(env.FVOCI_MEILI_URL ?? "").replace(/\/$/, "")}${path}"\nheader = "Authorization: Bearer ${env.MEILI_MASTER_KEY ?? ""}"\n`,
        ]),
        stdout: "pipe",
        stderr: "ignore",
      },
    );
    const text = await new Response(child.stdout).text();
    if ((await child.exited) !== 0) throw new CheckError("Meilisearch retirement request failed");
    const split = text.lastIndexOf("\n");
    return { body: text.slice(0, split), code: text.slice(split + 1) };
  };
  const deleteIndex = async () => {
    const initial = await meili("DELETE", "/indexes/" + index);
    const task: unknown = JSON.parse(initial.body);
    if (
      initial.code !== "202" ||
      !task ||
      typeof task !== "object" ||
      !("taskUid" in task) ||
      !Number.isSafeInteger(task.taskUid)
    )
      throw new CheckError("index delete did not enqueue a task");
    const deadline = performance.now() + 60_000;
    let done = false,
      lastStatus = "unknown";
    for (let i = 0; i < 300 && performance.now() < deadline; i++) {
      const response = await meili("GET", "/tasks/" + String(task.taskUid));
      const facts: unknown = JSON.parse(response.body);
      if (response.code !== "200" || !facts || typeof facts !== "object" || !("status" in facts))
        throw new CheckError("invalid index retirement task");
      lastStatus = String(facts.status);
      if (facts.status === "succeeded") {
        done = true;
        break;
      }
      if (
        facts.status === "failed" &&
        "error" in facts &&
        facts.error &&
        typeof facts.error === "object" &&
        "code" in facts.error &&
        facts.error.code === "index_not_found"
      ) {
        done = true;
        break;
      }
      if (["failed", "canceled"].includes(String(facts.status))) break;
      const wait = Bun.spawn(["sleep", "0.2"], { env, stdout: "ignore", stderr: "ignore" });
      if ((await wait.exited) !== 0) throw new CheckError("index retirement wait failed");
    }
    if (!done)
      throw new CheckError(
        `Meilisearch index deletion task ${String(task.taskUid)} did not succeed (status ${lastStatus})`,
      );
    if ((await meili("GET", "/indexes/" + index)).code !== "404")
      throw new CheckError("index still present");
  };
  try {
    mark("containers ready; preparing database");
    step = "create-database";
    const suffix = randomBytes(8).toString("hex");
    db = "fvoci_e2e_" + suffix;
    role = "fvoci_app_" + db;
    env.ROLE_PASSWORD = randomBytes(16).toString("hex");
    await database("create-database");
    step = "database-url";
    const urls = databaseUrls(env.TEST_DATABASE_URL ?? "", db, role, env.ROLE_PASSWORD);
    env.DATABASE_URL = urls.admin;
    env.DATABASE_APP_URL = urls.app;
    step = "migrate";
    await call([migrate]);
    step = "create-app-role";
    await database("create-role");
    step = "grant-app-role";
    await call([migrate, "--grant-app-role", role]);
    delete env.ROLE_PASSWORD;
    Object.assign(env, {
      PASSWORD_PEPPER_KEYS: pepper,
      PASSWORD_PEPPER_ACTIVE_KEY_ID: "test",
      ENCRYPTION_KEYS: JSON.stringify({ e2e: randomBytes(32).toString("hex") }),
      ENCRYPTION_ACTIVE_KEY_ID: "e2e",
      FVOCI_WEBHOOK_ALLOW_TARGETS: "127.0.0.1",
      FVOCI_BIND: "127.0.0.1:0",
      RUST_LOG: env.RUST_LOG ?? "warn,tower_http=debug",
      FVOCI_EXTRACT_POLL_SECS: "2",
      FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0",
      FVOCI_STORAGE_DIR: join(run, "storage"),
      FVOCI_E2E_ADMIN_DATABASE_URL: urls.admin,
      FVOCI_E2E_SERVER_BIN: server,
      FVOCI_E2E_RESULT_DIR: run,
    });
    if (!env.FVOCI_STATIC_DIR) fail("isolated static assets required");
    mkdirSync(join(run, "storage"), { recursive: true });
    delete env.DATABASE_URL;
    delete env.FVOCI_MIGRATION_URL;
    const capture = join(run, "smtp.jsonl"),
      portFile = join(run, "smtp.port");
    writeFileSync(capture, "");
    step = "smtp-sink";
    smtp = spawnGroup(
      [
        "bun",
        join(root, "tools/web-e2e/smtp-sink.ts"),
        "--capture",
        capture,
        "--port-file",
        portFile,
      ],
      env,
    );
    const deadline = performance.now() + 10_000;
    while (!existsSync(portFile) || statSync(portFile).size === 0) {
      if (!alive(smtp) || performance.now() >= deadline) fail("smtp sink did not write port file");
      await pause("0.05");
    }
    Object.assign(env, {
      SMTP_HOST: "127.0.0.1",
      SMTP_PORT: readFileSync(portFile, "utf8").trim(),
      SMTP_FROM: "noreply@example.com",
      FVOCI_E2E_SMTP_CAPTURE: capture,
    });
    if (env.FVOCI_E2E_PENDING !== "1") {
      const setupKeys = new Set([
        "FVOCI_E2E_ADMIN_DATABASE_URL",
        "TEST_DATABASE_URL",
        "FVOCI_TEST_DATABASE_URL",
        "MEILI_MASTER_KEY",
        "FVOCI_MEILI_MASTER_KEY",
        "FVOCI_MEILI_KEY",
      ]);
      const serverEnv = Object.fromEntries(
        Object.entries(env).filter(([key]) => !setupKeys.has(key)),
      );
      if (env.FVOCI_MEILI_URL && env.MEILI_MASTER_KEY) {
        step = "meili-key";
        keyFile = join(run, "meili-search.key");
        index = "fvoci_e2e_" + suffix;
        env.FVOCI_MEILI_INDEX = index;
        serverEnv.FVOCI_MEILI_INDEX = index;
        await call([migrate, "--ensure-meili-key", keyFile]);
        serverEnv.FVOCI_MEILI_KEY_FILE = keyFile;
      }
      step = "server-start";
      const names = join(run, "server-env-names.txt");
      writeFileSync(names, Object.keys(serverEnv).sort().join("\n") + "\n", { mode: 0o600 });
      env.FVOCI_E2E_SERVER_ENV_NAMES = names;
      const fd = openSync(log, "w", 0o600);
      try {
        serverChild = spawnGroup([server], serverEnv, fd);
      } finally {
        closeSync(fd);
      }
      // The fixture controls seq/sleep; real invocations retain 120 polls and 30s.
      if (
        Bun.spawnSync(["seq", "1", "120"], { env, stdout: "ignore", stderr: "inherit" })
          .exitCode !== 0
      )
        fail("startup poll sequence failed");
      let base = "",
        ready = false;
      for (let i = 0; i < 120; i++) {
        checkInterrupt();
        const text = readFileSync(log, "utf8");
        base = /fvoci-server listening on ([^\r\n]+)/.exec(text)?.[1] ?? "";
        if (!alive(serverChild)) fail("server exited during startup");
        if (
          base &&
          Bun.spawnSync(["curl", "-fsS", base + "/api/v1/setup"], {
            env,
            stdout: "ignore",
            stderr: "ignore",
          }).exitCode === 0
        ) {
          ready = true;
          break;
        }
        await pause("0.25");
      }
      if (!ready)
        fail("server did not become ready within 30s (GET /api/v1/setup never succeeded)");
      env.PLAYWRIGHT_BASE_URL = base;
      mark("server ready");
    }
    step = "network-settle";
    const settled = Bun.spawn(
      ["bun", "--no-env-file", join(root, "tools/web-e2e/network-settle.ts")],
      { env, stdout: "inherit", stderr: "pipe" },
    );
    const warning = await new Response(settled.stderr).text();
    process.stderr.write(warning);
    const settleStatus = await settled.exited;
    if (settleStatus !== 0) throw new RuntimeExit(settleStatus);
    const events = () =>
      env.NET_MONITOR_LOG && existsSync(env.NET_MONITOR_LOG)
        ? readFileSync(env.NET_MONITOR_LOG, "utf8").split("\n").length - 1
        : 0;
    const before = events();
    checkInterrupt();
    step = "playwright";
    mark("playwright start");
    const pwEnv = { ...env };
    delete pwEnv.JEST_WORKER_ID;
    playwright = Bun.spawn(
      [
        "setsid",
        "bun",
        "--bun",
        "x",
        "--no-install",
        "playwright",
        "test",
        ...(env.FVOCI_E2E_PENDING === "1"
          ? ["--config=e2e-pending/collab-playwright.config.ts"]
          : []),
        "--output=" + join(run, "playwright-output"),
        ...argv,
      ],
      {
        cwd: join(root, "apps/web"),
        env: pwEnv,
        stdin: "ignore",
        stdout: "inherit",
        stderr: "inherit",
      },
    );
    status = await playwright.exited;
    mark(`playwright exited with status ${String(status)}`);
    if (env.NET_MONITOR_PID) {
      try {
        process.kill(Number(env.NET_MONITOR_PID), 0);
        process.stderr.write(
          `network: netlink address/link events while Playwright ran: ${String(events() - before)}\n`,
        );
      } catch {
        /* monitor already exited */
      }
    }
    if (status !== 0 && warning.includes("network settle: warning: "))
      process.stderr.write(
        "web-e2e: Playwright started before the host network settled (" + warning.trim() + ")\n",
      );
    if (interrupted) status = interrupted;
    if (status !== 0) throw new RuntimeExit(status);
  } catch (error) {
    status = error instanceof RuntimeExit ? error.status : 1;
    process.stderr.write(`web-e2e step ${step} failed (exit ${String(status)})\n`);
    if (error instanceof CheckError) process.stderr.write(error.message + "\n");
    if (step === "server-start" && existsSync(log))
      process.stderr.write(
        readFileSync(log, "utf8")
          .replace(/postgres(?:ql)?:\/\/\S+/g, "postgres://redacted")
          .replace(/libsql:\/\/\S+/g, "libsql://redacted")
          .replace(/https:\/\/\S*\.turso\.io\S*/g, "https://redacted")
          .replace(
            /\b(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=\S+/g,
            "$1=redacted",
          ),
      );
  } finally {
    // A second interrupt cannot cut bounded retirement short.
    for (const [signal, handler] of handlers) process.removeListener(signal, handler);
    const ignore = () => {};
    for (const [signal] of signals) process.on(signal, ignore);
    const failed: string[] = [];
    const retire = async (name: string, body: () => Promise<void>) => {
      try {
        await body();
      } catch (error) {
        failed.push(name);
        if (error instanceof CheckError) process.stderr.write(error.message + "\n");
      }
    };
    if (playwright && alive(playwright)) await Promise.race([playwright.exited, Bun.sleep(30_000)]);
    await retire("stop-playwright", () => stop("playwright", playwright));
    await retire("stop-server", () => stop("server", serverChild));
    await retire("stop-smtp-sink", () => stop("smtp-sink", smtp));
    if (db) {
      await retire("drop-database", () => database("drop-database"));
      await retire("drop-role", () => database("drop-role"));
    }
    if (index) {
      await retire("delete-meili-index", deleteIndex);
      if (keyFile && existsSync(keyFile) && statSync(keyFile).size)
        await retire("delete-meili-key", async () => {
          if (
            (await meili("DELETE", "/keys/" + readFileSync(keyFile, "utf8").trim())).code !== "204"
          )
            fail("Meilisearch key delete failed");
        });
    }
    if (failed.length) {
      process.stderr.write(`web-e2e cleanup failed: ${failed.join(" ")}\n`);
      if (status === 0) status = 1;
      else
        process.stderr.write(
          `web-e2e: the group verdict stays exit ${String(status)} (step ${step})\n`,
        );
    }
    for (const [signal] of signals) process.removeListener(signal, ignore);
  }
  return status;
}

function checkedOutput(argv: string[], cwd: string): string {
  const result = Bun.spawnSync(argv, { cwd, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0 || result.signalCode) fail("container browser prerequisite failed");
  return result.stdout.toString().trim();
}

/** One pinned image, existing producer packet, then one real isolated shard. */
async function containerBrowser(argv: string[]): Promise<number> {
  if (argv.length !== 3 || !["--ci-shard", "--ci-shards"].includes(argv[1] ?? ""))
    throw new UsageError("container-browser ROOT {--ci-shard N|--ci-shards N,N,...}");
  const root = realpathSync(argv[0] ?? "");
  if (process.platform !== "linux") fail("local browser containers currently require a Linux host");
  const shards = (argv[2] ?? "").split(",").map(containerShard);
  if (new Set(shards).size !== shards.length || (argv[1] === "--ci-shard" && shards.length !== 1))
    fail("duplicate or invalid shard selection");
  if (
    process.env.GITHUB_ACTIONS ||
    Object.keys(process.env).some((key) => key.startsWith("GITHUB_"))
  )
    fail("local container execution cannot impersonate a GitHub job");
  const uid = process.getuid?.(),
    gid = process.getgid?.();
  if (!uid || gid === undefined) fail("a non-root POSIX Docker client is required");
  const image = CI_WEB_IMAGE;
  if (
    !/^linux (?:x86_64|amd64)$/.test(
      checkedOutput(["docker", "info", "--format", "{{.OSType}} {{.Architecture}}"], root),
    )
  )
    fail("the first browser slice requires a native Linux amd64 Docker daemon");
  const metadata = checkedOutput(
    [
      "docker",
      "image",
      "inspect",
      "--format",
      "{{.Os}} {{.Architecture}} {{json .RepoDigests}}",
      image,
    ],
    root,
  );
  if (!metadata.startsWith("linux amd64 ") || !metadata.includes(JSON.stringify(image)))
    fail("the locally available pinned native amd64 ci-web image is required (no automatic pull)");
  const source = checkedOutput(["git", "rev-parse", "HEAD"], root);
  checkedOutput(["git", "diff", "--quiet", "HEAD", "--"], root);
  const common = realpathSync(
    resolve(root, checkedOutput(["git", "rev-parse", "--git-common-dir"], root)),
  );
  const batch = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-container-browser-")));
  if (statSync(batch).uid !== uid || (statSync(batch).mode & 0o777) !== 0o700)
    fail("the owned private browser batch is required");
  chmodSync(batch, 0o700);
  const run = randomBytes(16).toString("hex");
  if (!/^[a-f0-9]{32}$/.test(run)) fail("invalid browser batch identity");
  const producer = join(batch, "producer");
  {
    for (const directory of [
      producer,
      ...shards.map((shard) => join(batch, "consumer-" + shard)),
      "cargo",
      "home",
      "tmp",
      "sqlite",
      "producer-output",
      ...shards.map((shard) => "consumer-output-" + shard),
    ].map((p) => (p.startsWith(batch) ? p : join(batch, p))))
      mkdirSync(directory, { mode: directory.endsWith("/tmp") ? 0o711 : 0o700 });
    // Only Git objects are mounted read-only, not account config or credentials. Both source
    // copies occupy the same physical path inside the image, preserving the
    // existing packet's no-relocation and full-current-input checks.
    for (const directory of [
      producer,
      ...shards.map((shard) => join(batch, "consumer-" + shard)),
    ]) {
      checkedOutput(
        ["git", "clone", "--shared", "--no-checkout", "--quiet", root, directory],
        root,
      );
      checkedOutput(["git", "checkout", "--quiet", "--detach", source], directory);
    }
  }
  const network = "fvoci-browser-" + run;
  async function runContainer(
    phase: "prepare" | "consume",
    services: NodeJS.ProcessEnv = {},
    shard = "",
  ): Promise<number> {
    const directory = phase === "prepare" ? producer : join(batch, "consumer-" + shard);
    const output = phase === "prepare" ? "producer-output" : "consumer-output-" + shard;
    const variables: Record<string, string> = {
      HOME: "/fvoci-run/home",
      CARGO_HOME: "/fvoci-run/cargo",
      TMPDIR: "/fvoci-run/tmp",
      LANG: "C.UTF-8",
      LC_ALL: "C.UTF-8",
      CARGO_TARGET_DIR: root + "/target",
      CARGO_BUILD_JOBS: "2",
      CARGO_INCREMENTAL: "0",
      CARGO_PROFILE_DEV_DEBUG: "0",
      CARGO_PROFILE_TEST_DEBUG: "0",
      GIT_OPTIONAL_LOCKS: "0",
      CI: "true",
      FVOCI_WEB_CONTAINER_LOCAL: "1",
      FVOCI_WEB_CONTAINER_IMAGE: image,
      FVOCI_WEB_CONTAINER_RUN: run,
      FVOCI_WEB_CONTAINER_SOURCE: source,
      FVOCI_WEB_CONTAINER_JOB:
        phase === "prepare" ? "workspace-browser-build" : "workspace-browser-shard",
      FVOCI_SELECTED_CI_OUTPUT: "/fvoci-run/output",
      FVOCI_WEB_BUILD_HANDOFF: "/fvoci-run/handoff",
    };
    if (phase === "consume") {
      const pg = new URL(services.TEST_DATABASE_URL ?? "");
      pg.hostname = "fvoci-pg";
      pg.port = "5432";
      variables.TEST_DATABASE_URL = pg.toString();
      variables.FVOCI_E2E_SHARED_SERVICES = "1";
      for (const name of [
        "FVOCI_TEST_PG_CONTAINER",
        "FVOCI_MEILI_KEY",
        "MEILI_MASTER_KEY",
        "FVOCI_TEST_MEILI_CONTAINER",
      ])
        variables[name] = services[name] ?? "";
      variables.FVOCI_MEILI_URL = "http://fvoci-meili:7700";
      variables.FVOCI_WEB_BUILD_HANDOFF_SHA256 = createHash("sha256")
        .update(readFileSync(join(batch, "handoff/handoff.json")))
        .digest("hex");
    }
    const envFile = join(batch, phase + shard + ".env");
    writeFileSync(
      envFile,
      Object.entries(variables)
        .map(([k, v]) => `${k}=${v}\n`)
        .join(""),
      { mode: 0o600, flag: "wx" },
    );
    const name = "fvoci-browser-" + run + "-" + phase + shard;
    const args = [
      "docker",
      "run",
      "--pull=never",
      "--init",
      "--name",
      name,
      "--user",
      `${String(uid)}:${String(gid)}`,
      "--cpus=2",
      "--memory=16g",
      "--pids-limit=1024",
      "--cap-drop=ALL",
      "--security-opt=no-new-privileges",
      "--env-file",
      envFile,
      "--workdir",
      root,
    ];
    for (const [from, to, readonly] of [
      [directory, root, false],
      [join(common, "objects"), join(common, "objects"), true],
      [batch, "/fvoci-run", false],
      // Bun's node shim uses /tmp, independently of TMPDIR. Hide the image
      // build's root-owned shim with the same owned ordinary-disk temp dir.
      [join(batch, "tmp"), "/tmp", false],
      [join(batch, output), "/fvoci-run/output", false],
    ] as const) {
      if (from.includes(",") || to.includes(",")) fail("Docker bind paths cannot contain commas");
      args.push("--mount", `type=bind,source=${from},target=${to}${readonly ? ",readonly" : ""}`);
    }
    if (phase === "consume") {
      args.push(
        "--network",
        network,
        "--mount",
        `type=bind,source=${producer}/node_modules,target=${root}/node_modules,readonly`,
      );
    }
    args.push(
      image,
      "bun",
      "--no-env-file",
      join(root, "tools/web-e2e/run-web-e2e.ts"),
      "browser-phase",
      root,
      phase,
      ...(phase === "consume" ? [shard] : []),
    );
    const started = performance.now();
    let child;
    try {
      child = Bun.spawn(args, {
        stdin: "ignore",
        stdout: "inherit",
        stderr: "inherit",
        timeout: 20 * 60_000,
      });
    } catch (error) {
      rmSync(envFile);
      throw error;
    }
    const interrupt = () => {
        child.kill("SIGINT");
      },
      terminate = () => {
        child.kill("SIGTERM");
      };
    process.once("SIGINT", interrupt);
    process.once("SIGTERM", terminate);
    let result: number;
    try {
      result = await child.exited;
    } finally {
      process.removeListener("SIGINT", interrupt);
      process.removeListener("SIGTERM", terminate);
      rmSync(envFile);
    }
    const inspect = Bun.spawnSync(
      [
        "docker",
        "inspect",
        "--format",
        "{{.State.Running}} {{.State.ExitCode}} {{.State.OOMKilled}}",
        name,
      ],
      { stdout: "pipe", stderr: "pipe" },
    );
    if (inspect.exitCode === 0) {
      if (result === 0 && inspect.stdout.toString().trim() !== "false 0 false") result = 1;
      if (!inspect.stdout.toString().startsWith("false ")) {
        // A stopped/lost Docker client is not proof its container retired.
        // Attempt bounded stop, keep the first failure, and never rm --force.
        Bun.spawnSync(["docker", "stop", "--time", "10", name], { stdout: "pipe", stderr: "pipe" });
        if (result === 0) result = 1;
      }
      const removed = Bun.spawnSync(["docker", "rm", "-v", name], {
        stdout: "pipe",
        stderr: "pipe",
      });
      if (removed.exitCode !== 0 && result === 0) result = 1;
    } else if (result === 0) result = 1;
    process.stderr.write(
      `container-browser phase=${phase} elapsed_seconds=${((performance.now() - started) / 1000).toFixed(3)} exit=${String(result)}\n`,
    );
    return result;
  }
  const prepared = await runContainer("prepare");
  if (prepared !== 0) return prepared;
  for (const shard of shards) {
    const status = await browserServices(root, batch, network, (services) =>
      runContainer("consume", services, shard),
    );
    if (status !== 0) return status;
  }
  return 0;
}

/** A refusal: the CLI prints the message to stderr and exits 1. */
export class CheckError extends Error {}
class UsageError extends Error {}

function fail(message: string): never {
  throw new CheckError(message);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

const LANE_JOBS = [
  "collaboration-install-on",
  "collaboration-postgres-on",
  "collaboration-sqlite-on",
  "collaboration-postgres-off",
  "collaboration-sqlite-off",
];
const COMMITTED_OUTPUTS = ["apps/web/openapi.json", "apps/web/src/generated/api.ts"];

export interface CommittedApiRequest {
  /** The --ci-shard index; empty when no shard was requested. */
  shard: string;
  selected: boolean;
  browserPhase: "" | "prepare" | "consume";
}

/** The GitHub jobs allowed to consume committed API outputs for this request. */
export function committedApiJobs(
  request: CommittedApiRequest,
  githubJob: string | undefined,
): ReadonlySet<string> {
  let job = request.shard ? "workspace-browser-shard" : "collaboration-flow";
  if (request.browserPhase === "prepare") job = "workspace-browser-build";
  if (githubJob === "collaboration-build" && request.selected) job = "collaboration-build";
  if (!request.shard && !request.selected && request.browserPhase !== "prepare") {
    fail("requires a browser shard or selected companion");
  }
  return new Set(job === "collaboration-flow" ? [job, ...LANE_JOBS] : [job]);
}

/** One git command's result; spawn failure and signals are refusals. */
export type Git = (args: string[]) => { status: number; stdout: Uint8Array };

export function spawnGit(root: string): Git {
  return (args) => {
    let result;
    try {
      result = Bun.spawnSync(["git", "-C", root, ...args], {
        stdin: "ignore",
        stdout: "pipe",
        stderr: "inherit",
      });
    } catch (error) {
      fail(`cannot run git ${args.join(" ")}: ${errorText(error)}`);
    }
    // Bun's types say number, but a signal leaves the exit code null.
    if (result.signalCode !== undefined || !Number.isInteger(result.exitCode)) {
      fail(`git ${args.join(" ")} ended by signal ${String(result.signalCode)}`);
    }
    return { status: result.exitCode, stdout: result.stdout };
  };
}

function gitBytes(git: Git, args: string[]): Uint8Array {
  const { status, stdout } = git(args);
  if (status !== 0) fail(`git ${args.join(" ")} failed with exit ${String(status)}`);
  return stdout;
}

function gitLine(git: Git, args: string[]): string {
  let text: string;
  try {
    text = UTF8.decode(gitBytes(git, args));
  } catch (error) {
    if (error instanceof CheckError) throw error;
    fail(`git ${args.join(" ")} printed invalid UTF-8`);
  }
  return text.endsWith("\n") ? text.slice(0, -1) : text;
}

function isPhysicalFile(path: string): boolean {
  try {
    return existsSync(path) && realpathSync(path) === path && lstatSync(path).isFile();
  } catch {
    return false;
  }
}

function isPrivateDirectory(path: string): boolean {
  try {
    const stat = statSync(path);
    return (
      !lstatSync(path).isSymbolicLink() &&
      stat.isDirectory() &&
      stat.uid === process.getuid?.() &&
      (stat.mode & 0o777) === 0o700
    );
  } catch {
    return false;
  }
}

function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

/**
 * Qualify the committed OpenAPI outputs of ROOT for a browser-only CI run:
 * the allocated job, a clean checkout at GITHUB_SHA, and physical regular
 * files whose bytes equal HEAD. Returns the tested SHA.
 */
export function qualifyCommittedApi(
  root: string,
  request: CommittedApiRequest,
  env: Record<string, string | undefined>,
  git: Git = spawnGit(root),
): string {
  const local = env.FVOCI_WEB_CONTAINER_LOCAL === "1";
  if (local) {
    if (
      request.selected ||
      !/^ghcr\.io\/aisflow\/fvoci-ci-web@sha256:[a-f0-9]{64}$/.test(
        env.FVOCI_WEB_CONTAINER_IMAGE ?? "",
      ) ||
      Object.keys(env).some((key) => key.startsWith("GITHUB_"))
    )
      fail("requires an ordinary local browser container");
  } else if (env.CI !== "true" || env.GITHUB_ACTIONS !== "true") fail("requires GitHub CI");
  const job = local ? env.FVOCI_WEB_CONTAINER_JOB : env.GITHUB_JOB;
  const allowed = committedApiJobs(request, job);
  if (job === undefined || !allowed.has(job)) {
    fail("requires the allocated browser job");
  }
  const sha = (local ? env.FVOCI_WEB_CONTAINER_SOURCE : env.GITHUB_SHA) ?? "";
  if (!/^[0-9a-f]{40}$/.test(sha) || gitLine(git, ["rev-parse", "HEAD"]) !== sha) {
    fail("checkout HEAD differs from tested SHA");
  }
  // ROOT is compared unresolved (as a pathlib path): a wrapper reached through
  // a symlink is refused.
  const base = normalizePath(root);
  let toplevel: string;
  try {
    toplevel = realpathSync(gitLine(git, ["rev-parse", "--show-toplevel"]));
  } catch (error) {
    if (error instanceof CheckError) throw error;
    fail("wrapper must belong to this checkout");
  }
  if (toplevel !== base) fail("wrapper must belong to this checkout");
  if (git(["diff", "--quiet", "HEAD", "--"]).status !== 0) fail("tracked checkout is dirty");
  for (const name of COMMITTED_OUTPUTS) {
    const entry = gitLine(git, ["ls-tree", "HEAD", "--", name]);
    const match = /^100644 blob ([0-9a-f]{40}|[0-9a-f]{64})\t(.*)$/su.exec(entry);
    if (!match || match[2] !== name) fail(`${name} must be a tracked regular output at HEAD`);
    const oid = match[1] ?? "";
    if (gitLine(git, ["ls-files", "--stage", "--", name]) !== `100644 ${oid} 0\t${name}`) {
      fail(`${name} index differs from HEAD`);
    }
    const path = join(base, name);
    if (!isPhysicalFile(path)) fail(`${name} must be a physical regular output`);
    let content: Uint8Array;
    try {
      content = readFileSync(path);
    } catch (error) {
      fail(`${name} cannot be read: ${errorText(error)}`);
    }
    if (content.length === 0 || !bytesEqual(content, gitBytes(git, ["cat-file", "blob", oid]))) {
      fail(`${name} physical bytes differ from HEAD`);
    }
  }
  return sha;
}

/** The specs of one groups.ts shard-jsonl line, in order. */
export function planSpecs(line: string): string[] {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    fail(`malformed shard plan line: ${line}`);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail(`shard plan line is not an object: ${line}`);
  }
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== "specs")
    fail(`shard plan line must hold only specs: ${line}`);
  const specs: unknown = (value as { specs: unknown }).specs;
  if (!Array.isArray(specs) || specs.length === 0) fail(`shard plan group has no specs: ${line}`);
  return specs.map((spec: unknown) => {
    if (typeof spec !== "string") fail(`shard plan spec is not a string: ${line}`);
    try {
      validateSpecRelpath(spec);
    } catch (error) {
      fail(errorText(error));
    }
    return spec;
  });
}

// Linux's limit on symlinks followed while resolving one path.
const MAX_SYMLINKS = 40;

/** The parts of a lexical POSIX path, as pathlib.PurePosixPath keeps them. */
function lexicalParts(path: string): { root: string; parts: string[] } {
  const root =
    path.startsWith("//") && !path.startsWith("///") ? "//" : path.startsWith("/") ? "/" : "";
  return { root, parts: path.split("/").filter((part) => part !== "" && part !== ".") };
}

/** pathlib.PurePosixPath(path) as a string: repeated slashes and "." dropped, ".." kept. */
export function normalizePath(path: string): string {
  const { root, parts } = lexicalParts(path);
  return root + parts.join("/") || ".";
}

/**
 * pathlib.Path(path).resolve(): absolute, every existing symlink followed
 * before a later "..", missing or unreadable components kept as written.
 * Unlike Python, a symlink loop is refused (as `realpath -m` does).
 */
export function resolvePath(path: string, cwd: string = process.cwd()): string {
  if (path === "") fail("cannot resolve an empty path");
  const pending = lexicalParts(path.startsWith("/") ? path : `${cwd}/${path}`).parts.reverse();
  let resolved = "";
  let links = 0;
  for (let name = pending.pop(); name !== undefined; name = pending.pop()) {
    if (name === "..") {
      resolved = resolved.slice(0, resolved.lastIndexOf("/"));
      continue;
    }
    const next = `${resolved}/${name}`;
    let target: string | undefined;
    try {
      target = lstatSync(next).isSymbolicLink() ? readlinkSync(next) : undefined;
    } catch {
      target = undefined;
    }
    if (target === undefined) {
      resolved = next;
      continue;
    }
    links += 1;
    if (links > MAX_SYMLINKS) fail(`too many levels of symbolic links: ${path}`);
    if (target.startsWith("/")) resolved = "";
    pending.push(...lexicalParts(target).parts.reverse());
  }
  return resolved || "/";
}

/** Refuse unless CHILD, resolved, is PARENT (resolved) or lies beneath it by whole components. */
export function requirePathWithin(child: string, parent: string, cwd?: string): void {
  const inner = resolvePath(child, cwd);
  const outer = resolvePath(parent, cwd);
  if (inner !== outer && !inner.startsWith(outer === "/" ? "/" : `${outer}/`)) {
    fail(`${child} (${inner}) is not within ${parent} (${outer})`);
  }
}

const SAFE_DIAGNOSTICS = "fvoci-selected-diagnostics";

function requirePrivateDirectory(prefix: string, label: string): void {
  if (!isPrivateDirectory(prefix))
    fail(`${label} ${prefix} must be a 0700 directory owned by this user`);
}

/**
 * Create the runner-owned diagnostics directory PREFIX, which must be
 * $RUNNER_TEMP (resolved)/fvoci-selected-diagnostics. An occupied, symlinked
 * or foreign destination is refused. Publishes the path to GITHUB_OUTPUT.
 */
export function createSafeDiagnostics(
  prefix: string,
  env: Record<string, string | undefined>,
): void {
  const runnerTemp = env.RUNNER_TEMP;
  if (!runnerTemp) fail("RUNNER_TEMP is required for selected safe diagnostics");
  const expected = join(resolvePath(runnerTemp), SAFE_DIAGNOSTICS);
  // Compared as pathlib paths: repeated slashes and "." do not matter, ".." does.
  const directory = normalizePath(prefix);
  if (directory !== expected) {
    fail(`selected safe diagnostics must be ${expected}, not ${prefix}`);
  }
  if (/[\n\r]/.test(directory)) fail("selected safe diagnostics path holds a line break");
  try {
    mkdirSync(directory, { mode: 0o700 });
  } catch (error) {
    fail(`cannot create selected safe diagnostics: ${errorText(error)}`);
  }
  requirePrivateDirectory(directory, "selected safe diagnostics");
  if (env.GITHUB_OUTPUT) {
    try {
      appendFileSync(env.GITHUB_OUTPUT, `selected-safe-diagnostics=${directory}\n`);
    } catch (error) {
      fail(`cannot publish selected safe diagnostics: ${errorText(error)}`);
    }
  }
}

const RECEIPT_KEYS = [
  "actual_launcher_exit",
  "ownership_return_exit",
  "selected_final_exit",
  "pending_exit",
  "config_list_exit",
] as const;

/** A shell exit status (0..255) or `not-run` (null). */
export function stageExit(value: string): number | null {
  if (value === "not-run") return null;
  if (!/^(?:0|[1-9][0-9]{0,2})$/.test(value) || Number(value) > 255) {
    fail(`invalid stage exit status: ${JSON.stringify(value)}`);
  }
  return Number(value);
}

/** The launcher-stage.json bytes (json.dump layout kept for existing readers). */
export function launcherReceipt(values: readonly string[]): string {
  if (values.length !== RECEIPT_KEYS.length) {
    fail(`expected ${String(RECEIPT_KEYS.length)} stage exit statuses`);
  }
  const fields = RECEIPT_KEYS.map((key, index) => {
    const exit = stageExit(values[index] ?? "");
    return `"${key}": ${exit === null ? "null" : String(exit)}`;
  });
  return `{${fields.join(", ")}}\n`;
}

/** Write PREFIX/launcher-stage.json exclusively, mode 0600, after validating every value. */
export function writeLauncherReceipt(prefix: string, values: readonly string[]): void {
  const text = launcherReceipt(values);
  requirePrivateDirectory(prefix, "selected safe diagnostics");
  let fd: number;
  try {
    fd = openSync(join(prefix, "launcher-stage.json"), "wx", 0o600);
  } catch (error) {
    fail(`cannot create launcher stage receipt: ${errorText(error)}`);
  }
  try {
    fchmodSync(fd, 0o600);
    writeFileSync(fd, text);
  } catch (error) {
    fail(`cannot write launcher stage receipt: ${errorText(error)}`);
  } finally {
    closeSync(fd);
  }
}

const USAGE = [
  "usage: run-web-e2e.ts container-browser ROOT {--ci-shard N|--ci-shards N,N,...}",
  "usage: run-web-e2e.ts committed-api ROOT SHARD true|false ''|prepare|consume",
  "       run-web-e2e.ts plan-specs JSON_LINE",
  "       run-web-e2e.ts path-within CHILD PARENT",
  "       run-web-e2e.ts safe-diagnostics PREFIX",
  "       run-web-e2e.ts launcher-receipt PREFIX LAUNCHER OWNERSHIP SELECTED PENDING CONFIG_LIST",
].join("\n");

function arity(rest: string[], count: number): void {
  if (rest.length !== count) {
    throw new UsageError(`expected ${String(count)} arguments, got ${String(rest.length)}`);
  }
}

function run(argv: string[]): string {
  const [command, ...rest] = argv;
  switch (command) {
    case "committed-api": {
      arity(rest, 4);
      const [root = "", shard = "", selected = "", browserPhase = ""] = rest;
      if (selected !== "true" && selected !== "false") {
        throw new UsageError(`selected must be true or false: ${selected}`);
      }
      if (browserPhase !== "" && browserPhase !== "prepare" && browserPhase !== "consume") {
        throw new UsageError(`unknown browser phase: ${browserPhase}`);
      }
      const request = { shard, selected: selected === "true", browserPhase } as const;
      try {
        const sha = qualifyCommittedApi(root, request, process.env);
        return `committed API outputs match tested checkout ${sha}\n`;
      } catch (error) {
        if (error instanceof CheckError) {
          throw new CheckError(`committed API qualification failed: ${error.message}`);
        }
        throw error;
      }
    }
    case "plan-specs":
      arity(rest, 1);
      return planSpecs(rest[0] ?? "")
        .map((spec) => `${spec}\n`)
        .join("");
    case "path-within":
      arity(rest, 2);
      requirePathWithin(rest[0] ?? "", rest[1] ?? "");
      return "";
    case "safe-diagnostics":
      arity(rest, 1);
      createSafeDiagnostics(rest[0] ?? "", process.env);
      return "";
    case "launcher-receipt":
      arity(rest, 1 + RECEIPT_KEYS.length);
      writeLauncherReceipt(rest[0] ?? "", rest.slice(1));
      return "";
    default:
      throw new UsageError(
        command === undefined ? "a command is required" : `unknown command: ${command}`,
      );
  }
}

if (import.meta.main) {
  try {
    const argv = process.argv.slice(2);
    if (argv[0] === "container-browser") process.exit(await containerBrowser(argv.slice(1)));
    if (argv[0] === "browser-phase")
      process.exit(await browserPhase(argv[1] ?? "", argv[2] ?? "", argv.slice(3)));
    if (argv[0] === "group-runtime") process.exit(await groupRuntime(argv.slice(1)));
    process.stdout.write(run(argv));
  } catch (error) {
    if (error instanceof RuntimeExit) process.exit(error.status);
    if (error instanceof UsageError) {
      process.stderr.write(`${USAGE}\nrun-web-e2e.ts: error: ${error.message}\n`);
      process.exit(2);
    }
    if (error instanceof CheckError) {
      process.stderr.write(`${error.message}\n`);
      process.exit(1);
    }
    throw error;
  }
}
