#!/usr/bin/env bun
// Web Playwright entrypoint. Same flags and stage lines as the previous shell wrapper.

import {
  appendFileSync,
  chmodSync,
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { groupLabel } from "./labels.ts";
import { shardPlanLines, verifyPlan, DEFAULT_SHARD_COUNT } from "./groups.ts";
import { command, commandStatus, Fail } from "./proc.ts";

const root = join(import.meta.dir, "../..");
const laneJobs = new Set([
  "collaboration-install-on",
  "collaboration-postgres-on",
  "collaboration-sqlite-on",
  "collaboration-postgres-off",
  "collaboration-sqlite-off",
]);
const lanes = new Set(["install/on", "postgres/on", "sqlite/on", "postgres/off", "sqlite/off"]);

function die(message: string, code = 1): never {
  console.error(message);
  process.exit(code);
}

async function git(args: string[]): Promise<string> {
  const result = await command(["git", "-C", root, ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.status !== 0) die(`committed API qualification failed: git ${args[0]} failed`);
  return result.stdout;
}

async function verifyCommittedApi(shard: string, selected: boolean, browserPhase: string) {
  const fail = (message: string): never => die(`committed API qualification failed: ${message}`);
  if (process.env.CI !== "true" || process.env.GITHUB_ACTIONS !== "true")
    fail("requires GitHub CI");
  let job = shard ? "workspace-browser-shard" : "collaboration-flow";
  if (browserPhase === "prepare") job = "workspace-browser-build";
  if (process.env.GITHUB_JOB === "collaboration-build" && selected) job = "collaboration-build";
  if (!shard && !selected && browserPhase !== "prepare")
    fail("requires a browser shard or selected companion");
  const actual = process.env.GITHUB_JOB ?? "";
  const allowed = job === "collaboration-flow" ? new Set([job, ...laneJobs]) : new Set([job]);
  if (!allowed.has(actual)) fail("requires the allocated browser job");
  const sha = process.env.GITHUB_SHA ?? "";
  if (!/^[0-9a-f]{40}$/.test(sha) || (await git(["rev-parse", "HEAD"])).trim() !== sha)
    fail("checkout HEAD differs from tested SHA");
  if (resolve((await git(["rev-parse", "--show-toplevel"])).trim()) !== resolve(root))
    fail("wrapper must belong to this checkout");
  if (
    (
      await command(["git", "-C", root, "diff", "--quiet", "HEAD", "--"], {
        stdout: "ignore",
        stderr: "pipe",
      })
    ).status !== 0
  )
    fail("tracked checkout is dirty");
  for (const name of ["apps/web/openapi.json", "apps/web/src/generated/api.ts"]) {
    const path = join(root, name);
    const entry = (await git(["ls-tree", "HEAD", "--", name])).trim();
    if (!entry.startsWith("100644 blob ") || !entry.endsWith(`\t${name}`))
      fail(`${name} must be a tracked regular output at HEAD`);
    const oid = entry.split(/\s+/)[2] ?? "";
    if ((await git(["ls-files", "--stage", "--", name])).trim() !== `100644 ${oid} 0\t${name}`)
      fail(`${name} index differs from HEAD`);
    if (
      !existsSync(path) ||
      resolve(path) !== path ||
      lstatSync(path).isSymbolicLink() ||
      !lstatSync(path).isFile()
    )
      fail(`${name} must be a physical regular output`);
    const content = readFileSync(path);
    const blob = await command(["git", "-C", root, "cat-file", "blob", oid], {
      stdout: "pipe",
      stderr: "pipe",
    });
    if (!content.length || blob.status !== 0 || !content.equals(Buffer.from(blob.stdout)))
      fail(`${name} physical bytes differ from HEAD`);
  }
  console.log(`committed API outputs match tested checkout ${sha}`);
}

async function stage(name: string, args: string[], env = process.env): Promise<number> {
  const started = Math.floor(Date.now() / 1000);
  const timed =
    process.env.GITHUB_JOB === "collaboration-build" ||
    process.env.GITHUB_JOB === "collaboration-flow";
  const now = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
  console.error(`web-e2e stage=${name} started${timed ? ` at=${now()}` : ""}`);
  const status = await commandStatus(args, { env, stdout: "inherit", stderr: "inherit" });
  const elapsed = Math.floor(Date.now() / 1000) - started;
  console.error(
    `web-e2e stage=${name}${timed ? ` finished at=${now()}` : ""} elapsed_seconds=${elapsed} exit=${status}`,
  );
  return status;
}

async function must(name: string, args: string[], env = process.env) {
  const status = await stage(name, args, env);
  if (status !== 0) process.exit(status);
}

function sourceExports(path: string) {
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const match = /^export ([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(line);
    if (!match) continue;
    let value = match[2] ?? "";
    if (
      (value.startsWith("'") && value.endsWith("'")) ||
      (value.startsWith('"') && value.endsWith('"'))
    )
      value = value.slice(1, -1);
    process.env[match[1] ?? ""] = value;
  }
}

async function webBuild() {
  const started = Math.floor(Date.now() / 1000);
  console.error("web-e2e stage=web-build started");
  const status = await commandStatus(["bun", "--bun", "run", "build"], {
    cwd: join(root, "apps/web"),
    stdout: "inherit",
    stderr: "inherit",
  });
  console.error(
    `web-e2e stage=web-build elapsed_seconds=${Math.floor(Date.now() / 1000) - started} exit=${status}`,
  );
  if (status !== 0) process.exit(status);
}

function inside(child: string, parent: string): boolean {
  const rel = relative(resolve(parent), resolve(child));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

async function selectedFooter(pendingStatus: number): Promise<number> {
  const repo = process.env.ROOT || root;
  if (process.env.SELECTED_BACKENDS !== "true") return pendingStatus === 0 ? 0 : pendingStatus;
  const output =
    process.env.FVOCI_SELECTED_CI_OUTPUT ??
    die("FVOCI_SELECTED_CI_OUTPUT: required private current cohort output");
  const parent =
    process.env.FVOCI_SELECTED_CI_SQLITE_PARENT ??
    die("FVOCI_SELECTED_CI_SQLITE_PARENT: required exact job-owned SQLite parent");
  const lib = process.env.SQLITE3_LIB_DIR ?? "";
  if (!inside(lib, parent)) process.exit(1);
  const runnerTemp = process.env.RUNNER_TEMP ?? die("RUNNER_TEMP: required");
  const safe = join(resolve(runnerTemp), "fvoci-selected-diagnostics");
  mkdirSync(safe, { mode: 0o700 });
  const safeStat = statSync(safe);
  if (safeStat.uid !== process.getuid?.() || (safeStat.mode & 0o777) !== 0o700) process.exit(1);
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(process.env.GITHUB_OUTPUT, `selected-safe-diagnostics=${safe}\n`);
  const runnerUid = (await command(["id", "-u"], { stdout: "pipe" })).stdout.trim();
  const runnerGid = (await command(["id", "-g"], { stdout: "pipe" })).stdout.trim();
  const dockerGid = (
    await command(["stat", "-c", "%g", "/var/run/docker.sock"], { stdout: "pipe" })
  ).stdout.trim();
  const permissions = await command(
    [
      "python3",
      join(repo, "scripts/run-selected-backend-e2e.py"),
      "permissions",
      "--output",
      output,
      "--sqlite-parent",
      parent,
      "--docker-gid",
      dockerGid,
    ],
    { stdout: "pipe", stderr: "inherit" },
  );
  if (permissions.status !== 0) process.exit(permissions.status);
  const groups = permissions.stdout.trim();
  process.env.PLAYWRIGHT_BROWSERS_PATH = join(output, "browser");
  const lane = process.env.FVOCI_COLLAB_LANE ?? "";
  if (lane && lane !== "install/on") {
    const receipt =
      process.env.FVOCI_CLOSED_INSTALL_RECEIPT ??
      die("FVOCI_CLOSED_INSTALL_RECEIPT: closed install receipt required");
    if (!existsSync(receipt) || lstatSync(receipt).isSymbolicLink())
      die("closed install receipt missing");
    const destination = join(output, "closed-install-receipt.json");
    writeFileSync(destination, readFileSync(receipt), { mode: 0o400 });
    process.env.FVOCI_CLOSED_INSTALL_RECEIPT = destination;
  }
  if (
    (await commandStatus(["sudo", "chown", "-h", "-R", "1000:1000", output, parent], {
      stdout: "inherit",
      stderr: "inherit",
    })) !== 0
  )
    process.exit(1);
  if (
    (await commandStatus(
      ["sudo", "install", "-d", "-o", "1000", "-g", "1000", "-m", "0700", join(output, "tmp")],
      { stdout: "inherit", stderr: "inherit" },
    )) !== 0
  )
    process.exit(1);
  let configList = "not-run";
  let launcher = "not-run";
  let selected = 0;
  const driver = join(repo, "scripts/run-selected-backend-e2e.py");
  const preserveList =
    "PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB";
  if (process.env.SELECTED_PHASE === "consume") {
    configList = "0";
    const stdoutPath = join(safe, "config-list.stdout.log");
    const stderrPath = join(safe, "config-list.stderr.log");
    if (existsSync(stdoutPath) || existsSync(stderrPath)) process.exit(1);
    const outFd = openSync(stdoutPath, "wx", 0o600);
    const errFd = openSync(stderrPath, "wx", 0o600);
    const status = await new Promise<number>((done) => {
      const child = require("node:child_process").spawn(
        "sudo",
        [
          "--preserve-env=" + preserveList + ",FVOCI_WEB_BUILD_PHASE,PLAYWRIGHT_BROWSERS_PATH",
          "setpriv",
          "--reuid=1000",
          "--regid=1000",
          `--groups=${groups}`,
          "env",
          `TMPDIR=${join(output, "tmp")}`,
          "python3",
          driver,
          "config-list",
          "--output",
          output,
        ],
        { stdio: ["ignore", outFd, errFd] },
      );
      child.on("close", (code: number | null) => done(code ?? 1));
    });
    closeSync(outFd);
    closeSync(errFd);
    chmodSync(stdoutPath, 0o600);
    chmodSync(stderrPath, 0o600);
    configList = String(status);
    selected = status;
  }
  if (lane && !lanes.has(lane)) die("unknown collaboration lane");
  const laneArgs = lane ? ["--lane", lane] : [];
  if (configList === "not-run" || configList === "0") {
    const status = await commandStatus(
      [
        "sudo",
        `--preserve-env=${preserveList},PLAYWRIGHT_BROWSERS_PATH`,
        "setpriv",
        "--reuid=1000",
        "--regid=1000",
        `--groups=${groups}`,
        "env",
        `TMPDIR=${join(output, "tmp")}`,
        "python3",
        driver,
        "run",
        "--output",
        output,
        ...laneArgs,
      ],
      { stdout: "inherit", stderr: "inherit" },
    );
    launcher = String(status);
    if (status !== 0) selected = status;
  }
  let ownership = 0;
  const ownershipPath = join(safe, "ownership-stage.json");
  const ownFd = openSync(ownershipPath, "w", 0o600);
  ownership = await new Promise<number>((done) => {
    const child = require("node:child_process").spawn(
      "sudo",
      [
        `--preserve-env=${preserveList}`,
        "setpriv",
        "--reuid=1000",
        "--regid=1000",
        `--groups=${groups}`,
        "python3",
        driver,
        "owner-return",
        "--output",
        output,
        ...laneArgs,
      ],
      { stdio: ["ignore", ownFd, "inherit"] },
    );
    child.on("close", (code: number | null) => done(code ?? 1));
  });
  closeSync(ownFd);
  if (ownership === 0) {
    if (
      (await commandStatus(
        ["sudo", "chown", "-h", "-R", `${runnerUid}:${runnerGid}`, output, parent],
        { stdout: "inherit", stderr: "inherit" },
      )) !== 0
    ) {
      console.error("selected runtime ownership restoration failed");
      if (selected === 0) selected = 1;
    } else if (process.env.GITHUB_OUTPUT) {
      const items = [
        "*-stderr.log",
        "*-stage.json",
        "*-driver.log",
        "selected-ci-receipt.json",
        "handoff-input-before-safe.json",
        "handoff-input-after-safe.json",
        "handoff-input-current-safe.json",
        "handoff-input-delta-safe.json",
      ];
      appendFileSync(
        process.env.GITHUB_OUTPUT,
        `selected-private-diagnostics<<FVOCI_CLOSED_DIAGNOSTICS\n${items.map((item) => `${output}/${item}`).join("\n")}\nFVOCI_CLOSED_DIAGNOSTICS\n`,
      );
    }
  } else {
    console.error("selected runtime ownership retained: resource retirement proof incomplete");
    if (selected === 0) selected = 1;
  }
  try {
    const receipt = join(safe, "launcher-stage.json");
    const fd = openSync(receipt, "wx", 0o600);
    const values = [
      launcher,
      String(ownership),
      String(selected),
      String(pendingStatus),
      configList,
    ].map((value) => (value === "not-run" ? null : Number(value)));
    writeFileSync(
      fd,
      `${JSON.stringify({ actual_launcher_exit: values[0], ownership_return_exit: values[1], selected_final_exit: values[2], pending_exit: values[3], config_list_exit: values[4] })}\n`,
    );
    closeSync(fd);
  } catch {
    if (selected === 0) selected = 1;
  }
  return pendingStatus !== 0 ? pendingStatus : selected;
}

function parse(argv: string[]) {
  let committed = false;
  let selected = false;
  let selectedPhase = "whole";
  let browserPhase = "";
  let shard = "";
  const specs: string[] = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index] ?? "";
    if (arg === "--ci-use-committed-api") {
      if (committed) die("--ci-use-committed-api may be supplied only once");
      committed = true;
    } else if (arg === "--ci-prepare-browser" || arg === "--ci-consume-browser") {
      if (browserPhase) die("duplicate browser phase");
      browserPhase = arg === "--ci-prepare-browser" ? "prepare" : "consume";
    } else if (arg === "--ci-prepare-selected" || arg === "--ci-consume-selected") {
      if (selectedPhase !== "whole" || selected) die("duplicate/mixed selected phase");
      selected = true;
      selectedPhase = arg === "--ci-prepare-selected" ? "prepare" : "consume";
    } else if (arg === "--with-selected-backends") {
      if (selected) die("--with-selected-backends may be supplied only once");
      selected = true;
    } else if (arg === "--ci-shard") {
      const value = argv[++index];
      if (!value) die("--ci-shard requires an index");
      shard = value;
    } else if (arg === "--") {
      specs.push(...argv.slice(index + 1));
      break;
    } else if (arg === "--ci-shard-count")
      die(
        `--ci-shard-count is not supported; CI uses a fixed shard count of ${DEFAULT_SHARD_COUNT}`,
      );
    else specs.push(arg);
  }
  return { committed, selected, selectedPhase, browserPhase, shard, specs };
}

async function runShard(
  shard: string,
  browserPhase: string,
  committed: boolean,
  selected: boolean,
  selectedPhase: string,
) {
  if (process.env.FVOCI_WEB_E2E_SHARD_COUNT)
    die(
      `FVOCI_WEB_E2E_SHARD_COUNT must not override the fixed CI shard count (${DEFAULT_SHARD_COUNT})`,
    );
  const index = Number(shard);
  if (!Number.isInteger(index) || index < 0 || index >= DEFAULT_SHARD_COUNT)
    die(`shard index ${shard} out of range 0..${DEFAULT_SHARD_COUNT - 1}`);
  try {
    verifyPlan(undefined, DEFAULT_SHARD_COUNT);
  } catch (error) {
    if (error instanceof Fail) die(error.message);
    throw error;
  }
  let lines: { specs: string[] }[];
  try {
    lines = shardPlanLines(join(root, "apps/web/e2e"), index, DEFAULT_SHARD_COUNT);
  } catch (error) {
    if (error instanceof Fail) die(error.message);
    throw error;
  }
  if (!lines.length) die(`shard ${shard} plan is empty`);
  console.error(`=== web e2e shard ${shard}/${DEFAULT_SHARD_COUNT}: qualify artifacts ===`);
  if (browserPhase === "consume")
    await must("browser-handoff-consume", [
      "python3",
      join(root, "scripts/selected-backend-ci/web-build-handoff.py"),
      "consume",
    ]);
  else await buildArtifacts(committed, selected, selectedPhase);
  for (const line of lines) {
    if (!line.specs.length) die(`shard plan group has no specs: ${JSON.stringify(line)}`);
    const status = await commandStatus(
      ["bun", join(import.meta.dir, "run-group.ts"), ...line.specs],
      {
        env: {
          ...process.env,
          ROOT: root,
          CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? join(root, "target"),
        },
        stdin: "ignore",
        stdout: "inherit",
        stderr: "inherit",
      },
    );
    if (status !== 0) die(`shard ${shard} failed on group: ${groupLabel(line.specs)}`, 1);
  }
  console.error(`=== web e2e shard ${shard}: all groups passed ===`);
}

async function buildArtifacts(committed: boolean, selected: boolean, selectedPhase: string) {
  const directory = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), "fvoci-sqlite-env-"));
  const file = join(directory, "env");
  const prepared = await commandStatus(
    ["bash", join(root, "scripts/prepare-sqlite-ci.sh"), "--env-file", file],
    { stdout: "inherit", stderr: "inherit" },
  );
  if (prepared !== 0) {
    rmSync(directory, { recursive: true, force: true });
    process.exit(1);
  }
  sourceExports(file);
  rmSync(directory, { recursive: true, force: true });
  if (committed) await verifyCommittedApi(process.env.CI_SHARD_ARG ?? "", selected, "");
  else await must("api-generation", ["bash", join(root, "scripts/generate-api.sh")]);
  await webBuild();
  if (committed) await verifyCommittedApi(process.env.CI_SHARD_ARG ?? "", selected, "");
  const collab = join(root, "crates/collab-engine/target");
  if (selectedPhase === "consume")
    await must("selected-handoff-consume", [
      "python3",
      join(root, "scripts/selected-backend-ci/web-build-handoff.py"),
      "consume",
    ]);
  else if (selected) {
    const output = process.env.FVOCI_SELECTED_CI_OUTPUT ?? "";
    const driver = join(root, "scripts/run-selected-backend-e2e.py");
    await must("selected-input-before", ["python3", driver, "record-before", "--output", output]);
    await must("selected-main", [
      "python3",
      driver,
      "stage",
      "--output",
      output,
      "--stage-name",
      "main",
      "--",
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--features",
      "db-tests,api-schema",
      "--bin",
      "fvoci-server",
      "--bin",
      "fvoci-migrate",
      "--bin",
      "fvoci-e2e-fixture",
      "--message-format=json-render-diagnostics",
    ]);
    await must("selected-lib", [
      "python3",
      driver,
      "stage",
      "--output",
      output,
      "--stage-name",
      "lib",
      "--",
      "cargo",
      "test",
      "--locked",
      "--offline",
      "--features",
      "db-tests,api-schema",
      "--lib",
      "--no-run",
      "--message-format=json-render-diagnostics",
    ]);
    await must("selected-install", [
      "python3",
      driver,
      "stage",
      "--output",
      output,
      "--stage-name",
      "install",
      "--",
      "cargo",
      "test",
      "--locked",
      "--offline",
      "--features",
      "db-tests,api-schema",
      "--test",
      "selected_install_lifetime",
      "--no-run",
      "--message-format=json-render-diagnostics",
    ]);
    await must(
      "selected-engine",
      [
        "python3",
        driver,
        "stage",
        "--output",
        output,
        "--stage-name",
        "engine",
        "--",
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--manifest-path",
        join(root, "crates/collab-engine/Cargo.toml"),
        "--features",
        "worker",
        "--bin",
        "collab-engine",
        "--message-format=json-render-diagnostics",
      ],
      { ...process.env, CARGO_TARGET_DIR: collab },
    );
    await must("selected-input-after", ["python3", driver, "record-after", "--output", output]);
  } else {
    await must("fixture-build", [
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--bin",
      "fvoci-e2e-fixture",
      "--features",
      "db-tests",
    ]);
    await must("default-server-build", [
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--bin",
      "fvoci-server",
      "--bin",
      "fvoci-migrate",
    ]);
    await must(
      "worker-build",
      [
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--manifest-path",
        join(root, "crates/collab-engine/Cargo.toml"),
        "--features",
        "worker",
        "--bin",
        "collab-engine",
      ],
      { ...process.env, CARGO_TARGET_DIR: collab },
    );
  }
}

async function main() {
  if (process.env.FVOCI_WEB_E2E_DRY_RUN) die("FVOCI_WEB_E2E_DRY_RUN is not supported");
  process.env.ROOT = root;
  process.env.CARGO_TARGET_DIR = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target"));
  process.env.FVOCI_COLLAB_ENGINE = join(root, "crates/collab-engine/target/debug/collab-engine");
  const parsed = parse(process.argv.slice(2));
  process.env.CI_SHARD_ARG = parsed.shard;
  process.env.SELECTED_BACKENDS = parsed.selected ? "true" : "false";
  process.env.SELECTED_PHASE = parsed.selectedPhase;
  if (parsed.browserPhase) {
    if (parsed.selected || !parsed.committed || parsed.specs.length)
      die("browser handoff requires committed API and no selected/spec options");
    if (process.env.CI !== "true" || process.env.GITHUB_ACTIONS !== "true")
      die("browser handoff requires GitHub CI");
    if (!process.env.FVOCI_SELECTED_CI_OUTPUT)
      die("FVOCI_SELECTED_CI_OUTPUT: required private browser build output");
    if (!process.env.FVOCI_WEB_BUILD_HANDOFF)
      die("FVOCI_WEB_BUILD_HANDOFF: missing browser producer packet path");
    if ((process.env.FVOCI_E2E_PROFILE ?? "debug") !== "debug")
      die("browser packet requires debug profile");
    process.env.FVOCI_WEB_BUILD_PHASE = parsed.browserPhase;
    if (parsed.browserPhase === "prepare") {
      if (process.env.GITHUB_JOB !== "workspace-browser-build" || parsed.shard)
        die("wrong browser producer job");
    } else if (process.env.GITHUB_JOB !== "workspace-browser-shard" || !parsed.shard)
      die("wrong browser consumer job");
    else if (!process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256)
      die("FVOCI_WEB_BUILD_HANDOFF_SHA256: missing browser producer digest");
  }
  if (
    process.env.GITHUB_ACTIONS === "true" &&
    process.env.GITHUB_JOB === "workspace-browser-shard" &&
    parsed.browserPhase !== "consume"
  ) {
    die("hosted browser shard requires --ci-consume-browser");
  }
  if (parsed.selected) {
    if (parsed.shard || process.env.FVOCI_E2E_PENDING !== "1" || parsed.specs.length)
      die(
        "--with-selected-backends requires the whole pending suite and cannot combine shard/spec/grep options",
      );
    if (!process.env.FVOCI_SELECTED_CI_OUTPUT)
      die("FVOCI_SELECTED_CI_OUTPUT: required private current cohort output");
    if (!process.env.GITHUB_ACTIONS)
      die("GITHUB_ACTIONS: selected companion requires its allocated GitHub job");
  }
  if (parsed.committed)
    await verifyCommittedApi(parsed.shard, parsed.selected, parsed.browserPhase);
  const prepared = await command(["bun", "--bun", "x", "--no-install", "playwright", "--version"], {
    cwd: join(root, "apps/web"),
    stdout: "ignore",
    stderr: "ignore",
  });
  if (prepared.status !== 0)
    die("missing web dependencies or Playwright; run scripts/prepare-web-e2e.sh");
  if (parsed.browserPhase === "prepare") {
    if (existsSync(join(root, "apps/web/dist"))) die("browser producer requires absent dist");
    const handoff = join(root, "scripts/selected-backend-ci/web-build-handoff.py");
    await must("browser-input-before", ["python3", handoff, "browser-before"]);
    await webBuild();
    await verifyCommittedApi(parsed.shard, parsed.selected, parsed.browserPhase);
    await must("fixture-build", ["python3", handoff, "browser-stage", "fixture"]);
    await must("default-server-build", ["python3", handoff, "browser-stage", "default"]);
    await must("worker-build", ["python3", handoff, "browser-stage", "engine"], {
      ...process.env,
      CARGO_TARGET_DIR: join(root, "crates/collab-engine/target"),
    });
    await must("browser-input-after", ["python3", handoff, "browser-after"]);
    await must("browser-handoff-export", ["python3", handoff, "export"]);
    process.exit(0);
  }
  if (parsed.shard) {
    if (parsed.specs.length) die("--ci-shard cannot be combined with explicit spec arguments");
    await runShard(
      parsed.shard,
      parsed.browserPhase,
      parsed.committed,
      parsed.selected,
      parsed.selectedPhase,
    );
    process.exit(0);
  }
  await buildArtifacts(parsed.committed, parsed.selected, parsed.selectedPhase);
  if (parsed.selectedPhase === "prepare") {
    await must("selected-handoff-export", [
      "python3",
      join(root, "scripts/selected-backend-ci/web-build-handoff.py"),
      "export",
    ]);
    process.exit(0);
  }
  const pending = await commandStatus(
    ["bun", join(import.meta.dir, "run-group.ts"), ...parsed.specs],
    { env: process.env, stdin: "ignore", stdout: "inherit", stderr: "inherit" },
  );
  if (process.argv.includes("--selected-footer"))
    process.exit(await selectedFooter(Number(process.env.PENDING_STATUS ?? pending)));
  process.exit(await selectedFooter(pending));
}

if (import.meta.main) {
  if (process.argv.includes("--selected-footer")) {
    selectedFooter(Number(process.env.PENDING_STATUS ?? "0"))
      .then((code) => process.exit(code))
      .catch((error: unknown) => {
        console.error(error instanceof Error ? error.message : "selected footer failed");
        process.exit(1);
      });
  } else {
    main().catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : "web e2e failed");
      process.exit(1);
    });
  }
}
