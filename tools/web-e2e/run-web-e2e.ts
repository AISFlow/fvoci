// Checks scripts/run-web-e2e.sh runs as `bun tools/web-e2e/run-web-e2e.ts
// <command> ...`: committed API qualification, shard plan line parsing, and
// the selected companion's SQLite prefix containment, private diagnostics
// directory and launcher stage receipt. A refusal prints one stderr line and
// exits 1; a usage error exits 2.
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
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
import { dirname, isAbsolute, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { randomBytes, createHash } from "node:crypto";
import process from "node:process";
import {
  nativeArchitecture,
  nativeLinux,
  type NativeArchitecture,
} from "../selected-backend-ci/types.ts";
import {
  validateSpecRelpath,
  discoverGroups,
  assignShards,
  verifyPlan,
  DEFAULT_SHARD_COUNT,
} from "./groups.ts";

export const CI_WEB_IMAGE =
  "ghcr.io/aisflow/fvoci-ci-web@sha256:cb1f5290f97ad5fb156b852ac0d21466e91571063979af4e9838172f41d8e23c";
const CI_WEB_SDK_SHA256 = {
  x86_64: "dfd2e7b84d19f6e8ca03b80ded497d69606df8685ed6f1a929b6e8ecaba35892",
  aarch64: "171384584c2a218a591e6d42784578c28ab1f64741487090b285006022260cca",
} as const;
const BROWSER_SERVICE_IMAGES = [
  "postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336",
  "getmeili/meilisearch:v1.55.0@sha256:ff546679970126f8822e15d1b6a06bfbff1f2ce2e43f810cfb5258012f92de5c",
] as const;

export function containerImage(
  image: string,
  sdk: string,
  local: boolean,
  env: NodeJS.ProcessEnv,
): { image: string; sdk: string; local: boolean } {
  if (!/^[a-f0-9]{64}$/.test(sdk)) fail("an explicit sealed SQLite SDK SHA-256 is required");
  if (
    !(
      local ? /^sha256:[a-f0-9]{64}$/ : /^ghcr\.io\/aisflow\/fvoci-ci-web@sha256:[a-f0-9]{64}$/
    ).test(image) ||
    (local && (env.CI !== undefined || Object.keys(env).some((key) => key.startsWith("GITHUB_"))))
  )
    fail("registry image digest required; local engine IDs require explicit non-CI local SDK mode");
  return { image, sdk, local };
}

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

let entryCancellation = 0;

export function commandProcess(argv: string[], system: string) {
  if (system === "linux") return { argv: ["setsid", ...argv], group: true };
  if (system === "darwin" && argv[0] === "docker") return { argv, group: false };
  fail("host-only commands require Linux; Darwin supports the ordinary Docker client only");
}

export async function command(
  argv: string[],
  cwd: string,
  env = process.env,
  stdio: { stdout: "inherit" | number; stderr: "inherit" | number } = {
    stdout: "inherit",
    stderr: "inherit",
  },
  cleanup = false,
): Promise<number> {
  if (entryCancellation && !cleanup) return entryCancellation;
  const invocation = commandProcess(argv, process.platform);
  const child = Bun.spawn(invocation.argv, {
    cwd,
    env,
    stdin: "ignore",
    ...stdio,
  });
  const signal = (name: NodeJS.Signals) => {
    try {
      process.kill(invocation.group ? -child.pid : child.pid, name);
    } catch {
      /* already retired */
    }
  };
  const interrupt = () => {
      entryCancellation ||= 130;
      signal("SIGINT");
    },
    terminate = () => {
      entryCancellation ||= 143;
      signal("SIGTERM");
    };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  try {
    const status = await child.exited;
    return status || (cleanup ? 0 : entryCancellation);
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", terminate);
  }
}

async function stage(name: string, argv: string[], root: string, env = process.env): Promise<void> {
  const started = performance.now();
  const dated = ["collaboration-build", "collaboration-flow"].includes(env.GITHUB_JOB ?? "");
  const timestamp = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
  process.stderr.write(`web-e2e stage=${name} started${dated ? " at=" + timestamp() : ""}\n`);
  const status = await command(argv, root, env);
  process.stderr.write(
    `web-e2e stage=${name}${dated ? " finished at=" + timestamp() : ""} elapsed_seconds=${((performance.now() - started) / 1000).toFixed(3)} exit=${String(status)}\n`,
  );
  if (status !== 0) throw new RuntimeExit(status);
}

class RuntimeExit extends Error {
  constructor(readonly status: number) {
    super("web e2e command failed");
  }
}

export function webRequest(argv: string[], env: NodeJS.ProcessEnv) {
  let shard = "",
    selected = false,
    committed = false,
    host = false;
  let selectedPhase: "whole" | "prepare" | "consume" = "whole";
  let browserPhase: "" | "prepare" | "consume" = "";
  const specs: string[] = [],
    imageOptions: string[] = [];
  for (let at = 0; at < argv.length; at++) {
    const arg = argv[at];
    switch (arg) {
      case "--host":
        host = true;
        break;
      case "--ci-use-committed-api":
        if (committed) fail("--ci-use-committed-api may be supplied only once");
        committed = true;
        break;
      case "--ci-prepare-browser":
      case "--ci-consume-browser":
        if (browserPhase) fail("duplicate browser phase");
        browserPhase = arg === "--ci-prepare-browser" ? "prepare" : "consume";
        break;
      case "--ci-prepare-selected":
      case "--ci-consume-selected":
        if (selected) fail("duplicate/mixed selected phase");
        selected = true;
        selectedPhase = arg === "--ci-prepare-selected" ? "prepare" : "consume";
        break;
      case "--with-selected-backends":
        if (selected) fail("--with-selected-backends may be supplied only once");
        selected = true;
        break;
      case "--ci-shard": {
        const value = argv[++at];
        if (shard || !value || !/^[0-9]+$/.test(value) || Number(value) >= DEFAULT_SHARD_COUNT)
          fail("--ci-shard requires one index within the fixed shard count");
        shard = String(Number(value));
        break;
      }
      case "--ci-shard-count":
        throw new CheckError(
          "--ci-shard-count is not supported; CI uses the fixed shard count from tools/web-e2e/groups.ts",
        );
      case "--image-pin":
      case "--sdk-sha256":
      case "--reuse-cohort": {
        const value = argv[++at];
        if (!value || imageOptions.includes(arg)) fail("duplicate or incomplete container option");
        imageOptions.push(arg, value);
        break;
      }
      case "--local-image-sdk":
        if (imageOptions.includes(arg)) fail("duplicate container option");
        imageOptions.push(arg);
        break;
      case "--":
        specs.push(...argv.slice(at + 1));
        at = argv.length;
        break;
      default:
        if (arg !== undefined) specs.push(arg);
    }
  }
  if (env.FVOCI_WEB_E2E_DRY_RUN) fail("FVOCI_WEB_E2E_DRY_RUN is not supported");
  if (env.FVOCI_WEB_E2E_SHARD_COUNT)
    fail(
      "FVOCI_WEB_E2E_SHARD_COUNT must not override the fixed CI shard count from tools/web-e2e/groups.ts",
    );
  if (shard && specs.length) fail("--ci-shard cannot be combined with explicit spec arguments");
  const github = Object.keys(env).some((key) => key.startsWith("GITHUB_"));
  if (
    browserPhase &&
    (selected || !committed || specs.length || env.CI !== "true" || env.GITHUB_ACTIONS !== "true")
  )
    fail("browser handoff requires GitHub committed API and no selected/spec options");
  if (
    env.GITHUB_ACTIONS === "true" &&
    env.GITHUB_JOB === "workspace-browser-shard" &&
    browserPhase !== "consume"
  )
    fail("hosted browser shard requires --ci-consume-browser");
  if (selected && (shard || specs.length || env.FVOCI_E2E_PENDING !== "1"))
    fail(
      "--with-selected-backends requires the whole pending suite and cannot combine shard/spec/grep options",
    );
  const hostRequired =
    host ||
    github ||
    selected ||
    !!browserPhase ||
    env.FVOCI_E2E_PENDING === "1" ||
    specs.length > 0 ||
    ["0", "3", "5"].includes(shard);
  if (hostRequired && imageOptions.length)
    fail("container image options cannot alter a required host/CI selection");
  return {
    shard,
    selected,
    committed,
    selectedPhase,
    browserPhase,
    specs,
    imageOptions,
    route: hostRequired ? ("host" as const) : shard ? ("docker" as const) : ("hybrid" as const),
  };
}

async function webEntry(root: string, argv: string[]): Promise<number> {
  entryCancellation = 0;
  const interrupt = () => {
      entryCancellation ||= 130;
    },
    terminate = () => {
      entryCancellation ||= 143;
    };
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", terminate);
  try {
    const status = await webEntryBody(root, argv);
    return status || entryCancellation;
  } finally {
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
  }
}

async function webEntryBody(root: string, argv: string[]): Promise<number> {
  if (entryCancellation) return entryCancellation;
  root = realpathSync(root);
  const request = webRequest(argv, process.env);
  if (request.route !== "docker" && (process.platform !== "linux" || process.arch !== "x64"))
    fail(
      "the full requested selection includes Linux x64 host-only assertions; select an explicit supported Docker shard on this client",
    );
  const { shard, selected, committed, selectedPhase, browserPhase, specs } = request;
  if (request.route !== "host") {
    const dockerShards = request.route === "hybrid" ? "1,2,4,6,7" : shard;
    process.stderr.write(
      `web-e2e route=docker shards=${dockerShards}${request.route === "hybrid" ? " host_required=0,3,5" : ""}\n`,
    );
    const status = await containerBrowser([
      root,
      "--ci-shards",
      dockerShards,
      ...request.imageOptions,
    ]);
    if (status !== 0 || entryCancellation || request.route === "docker")
      return status || entryCancellation;
  } else process.stderr.write("web-e2e route=host requested selection retained\n");
  process.env.ROOT = root;
  process.env.CARGO_TARGET_DIR = resolvePath(
    process.env.CARGO_TARGET_DIR ?? join(root, "target"),
    process.cwd(),
  );
  const engineTarget = join(root, "crates/collab-engine/target");
  process.env.FVOCI_COLLAB_ENGINE = join(engineTarget, "debug/collab-engine");
  const bun = (...args: string[]) => [process.execPath, ...args];
  const handoff = (...args: string[]) =>
    bun(join(root, "tools/selected-backend-ci/handoff.ts"), ...args);
  const selectedCli = (...args: string[]) =>
    bun(join(root, "scripts/run-selected-backend-e2e.ts"), ...args);
  const required = (key: string) => {
    const value = process.env[key];
    if (!value) fail(`required ${key}`);
    return value;
  };
  const api = () => {
    const sha = qualifyCommittedApi(root, { shard, selected, browserPhase }, process.env);
    process.stdout.write(`committed API outputs match tested checkout ${sha}\n`);
  };
  if (browserPhase) {
    required("FVOCI_SELECTED_CI_OUTPUT");
    required("FVOCI_WEB_BUILD_HANDOFF");
    if ((process.env.FVOCI_E2E_PROFILE ?? "debug") !== "debug")
      fail("browser packet requires debug profile");
    process.env.FVOCI_WEB_BUILD_PHASE = browserPhase;
    if (browserPhase === "prepare") {
      if (process.env.GITHUB_JOB !== "workspace-browser-build" || shard)
        fail("wrong browser producer job");
    } else {
      if (process.env.GITHUB_JOB !== "workspace-browser-shard" || !shard)
        fail("wrong browser consumer job");
      required("FVOCI_WEB_BUILD_HANDOFF_SHA256");
    }
  }
  if (selected) {
    required("FVOCI_SELECTED_CI_OUTPUT");
    required("GITHUB_ACTIONS");
    if (selectedPhase === "whole") {
      if (
        !["collaboration-flow", ...LANE_JOBS].includes(process.env.GITHUB_JOB ?? "") ||
        process.env.FVOCI_WEB_BUILD_PHASE
      )
        fail("wrong same-job selected authority");
    } else {
      process.env.FVOCI_WEB_BUILD_PHASE = selectedPhase;
      if (!committed || process.env.CI !== "true" || process.env.GITHUB_ACTIONS !== "true")
        fail("handoff requires explicit GitHub committed API mode");
      if (selectedPhase === "prepare") {
        if (process.env.GITHUB_JOB !== "collaboration-build") fail("wrong producer job");
      } else {
        if (!["collaboration-flow", ...LANE_JOBS].includes(process.env.GITHUB_JOB ?? ""))
          fail("wrong consumer job");
        required("FVOCI_WEB_BUILD_HANDOFF");
        required("FVOCI_WEB_BUILD_HANDOFF_SHA256");
      }
    }
  }
  if (committed) api();
  if (selectedPhase === "consume") await stage("selected-handoff-admit", handoff("admit"), root);
  if (
    Bun.spawnSync(bun("--bun", "x", "--no-install", "playwright", "--version"), {
      cwd: join(root, "apps/web"),
      stdout: "ignore",
      stderr: "ignore",
    }).exitCode !== 0
  )
    fail("missing web dependencies or Playwright; run scripts/prepare-web-e2e.sh");
  const native = async (name: string, profile: "default" | "selected") => {
    const args =
      name === "engine"
        ? [
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
          ]
        : name === "lib"
          ? [
              "cargo",
              "test",
              "--locked",
              "--offline",
              "--features",
              "db-tests,api-schema",
              "--lib",
              "--no-run",
            ]
          : name === "install"
            ? [
                "cargo",
                "test",
                "--locked",
                "--offline",
                "--features",
                "db-tests,api-schema",
                "--test",
                "selected_install_lifetime",
                "--no-run",
              ]
            : name === "main"
              ? [
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
                ]
              : name === "fixture"
                ? [
                    "cargo",
                    "build",
                    "--locked",
                    "--offline",
                    "--bin",
                    "fvoci-e2e-fixture",
                    "--features",
                    "db-tests",
                  ]
                : [
                    "cargo",
                    "build",
                    "--locked",
                    "--offline",
                    "--bin",
                    "fvoci-server",
                    "--bin",
                    "fvoci-migrate",
                  ];
    const output = process.env.FVOCI_SELECTED_CI_OUTPUT ?? "";
    const target = name === "engine" ? engineTarget : required("CARGO_TARGET_DIR");
    if (profile === "selected") args.push("--message-format=json-render-diagnostics");
    await stage(
      profile === "selected"
        ? "selected-" + name
        : name === "engine"
          ? "worker-build"
          : name === "fixture"
            ? "fixture-build"
            : "default-server-build",
      profile === "selected"
        ? [...selectedCli("stage", "--output", output, "--stage-name", name, "--"), ...args]
        : args,
      root,
      { ...process.env, CARGO_TARGET_DIR: target },
    );
  };
  const build = async () => {
    if (selectedPhase === "consume") {
      if (
        !process.env.SQLITE3_LIB_DIR ||
        !process.env.SQLITE3_INCLUDE_DIR ||
        process.env.SQLITE3_STATIC !== "1" ||
        process.env.SQLITE3_NO_PKG_CONFIG !== "1"
      )
        fail("selected consumer requires the workflow-prepared SQLite environment");
    } else {
      const temporary = mkdtempSync(join(tmpdir(), "fvoci-sqlite-env.")),
        file = join(temporary, "env");
      try {
        try {
          await stage(
            "sqlite-prepare",
            ["bash", join(root, "scripts/prepare-sqlite-ci.sh"), "--env-file", file],
            root,
          );
        } catch (error) {
          if (error instanceof RuntimeExit) throw new RuntimeExit(1);
          throw error;
        }
        Object.assign(process.env, sqliteEnvironment(readFileSync(file, "utf8")));
      } finally {
        rmSync(temporary, { recursive: true, force: true });
      }
    }
    if (!committed)
      await stage("api-generation", ["bash", join(root, "scripts/generate-api.sh")], root);
    await stage("web-build", bun("--bun", "run", "build"), join(root, "apps/web"));
    if (committed) api();
    if (selectedPhase === "consume")
      await stage("selected-handoff-consume", handoff("consume"), root);
    else if (selected) {
      await stage(
        "selected-input-before",
        selectedCli("record-before", "--output", required("FVOCI_SELECTED_CI_OUTPUT")),
        root,
      );
      for (const name of ["main", "lib", "install", "engine"]) await native(name, "selected");
      await stage(
        "selected-input-after",
        selectedCli("record-after", "--output", required("FVOCI_SELECTED_CI_OUTPUT")),
        root,
      );
    } else for (const name of ["fixture", "server", "engine"]) await native(name, "default");
  };
  if (browserPhase === "prepare") {
    if (existsSync(join(root, "apps/web/dist"))) fail("browser producer requires absent dist");
    await stage("browser-input-before", handoff("browser-before"), root);
    await stage("web-build", bun("--bun", "run", "build"), join(root, "apps/web"));
    api();
    for (const [label, name] of [
      ["fixture-build", "fixture"],
      ["default-server-build", "default"],
      ["worker-build", "engine"],
    ])
      await stage(
        label ?? "",
        handoff("browser-stage", name ?? ""),
        root,
        name === "engine" ? { ...process.env, CARGO_TARGET_DIR: engineTarget } : process.env,
      );
    await stage("browser-input-after", handoff("browser-after"), root);
    await stage("browser-handoff-export", handoff("export"), root);
    return 0;
  }
  const group = (args: string[]) =>
    command(["bash", join(root, "scripts/web-e2e-run-group.sh"), ...args], root);
  if (shard || request.route === "hybrid") {
    verifyPlan(undefined, DEFAULT_SHARD_COUNT);
    const plan = assignShards(discoverGroups(), DEFAULT_SHARD_COUNT);
    if (browserPhase === "consume")
      await stage("browser-handoff-consume", handoff("consume"), root);
    else await build();
    for (const index of request.route === "hybrid" ? ["0", "3", "5"] : [shard]) {
      process.stderr.write(`web-e2e host_complete_shard=${index}\n`);
      for (const specs of plan[Number(index)] ?? []) {
        const status = await group(specs);
        if (status !== 0) return status;
      }
    }
    return 0;
  }
  await build();
  if (selectedPhase === "prepare") {
    await stage("selected-handoff-export", handoff("export"), root);
    return 0;
  }
  const pending = await group(specs);
  if (!selected || entryCancellation) return pending || entryCancellation;
  try {
    return await selectedHost(root, pending, selectedPhase === "consume");
  } catch (error) {
    if (pending) return pending;
    throw error;
  }
}

export async function selectedHost(
  root: string,
  pending: number,
  consume: boolean,
): Promise<number> {
  if (entryCancellation) return pending || entryCancellation;
  const required = (key: string) => {
    const value = process.env[key];
    if (!value) fail(`required ${key}`);
    return value;
  };
  const output = required("FVOCI_SELECTED_CI_OUTPUT"),
    sqlite = required("FVOCI_SELECTED_CI_SQLITE_PARENT");
  requirePathWithin(required("SQLITE3_LIB_DIR"), sqlite);
  const diagnostics = join(required("RUNNER_TEMP"), "fvoci-selected-diagnostics");
  createSafeDiagnostics(diagnostics, process.env);
  const uid = String(process.getuid?.()),
    gid = String(process.getgid?.());
  const dockerGid = checkedOutput(["stat", "-c", "%g", "/var/run/docker.sock"], root);
  const cli = join(root, "scripts/run-selected-backend-e2e.ts");
  const groups = checkedOutput(
    [
      process.execPath,
      cli,
      "permissions",
      "--output",
      output,
      "--sqlite-parent",
      sqlite,
      "--docker-gid",
      dockerGid,
    ],
    root,
  );
  process.env.PLAYWRIGHT_BROWSERS_PATH = join(output, "browser");
  const lane = process.env.FVOCI_COLLAB_LANE,
    laneArgs: string[] = [];
  if (lane) {
    if (!["install/on", "postgres/on", "sqlite/on", "postgres/off", "sqlite/off"].includes(lane))
      fail("unknown collaboration lane");
    laneArgs.push("--lane", lane);
    if (lane !== "install/on") {
      const receipt = required("FVOCI_CLOSED_INSTALL_RECEIPT");
      if (!existsSync(receipt) || lstatSync(receipt).isSymbolicLink())
        fail("closed install receipt missing");
      const destination = join(output, "closed-install-receipt.json");
      await stage("closed-install-receipt", ["install", "-m", "0400", receipt, destination], root);
      process.env.FVOCI_CLOSED_INSTALL_RECEIPT = destination;
    }
  }
  const preserved =
    "PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB";
  const runtime = (mode: string, browser = false) => [
    "sudo",
    "--preserve-env=" +
      preserved +
      (browser ? ",PLAYWRIGHT_BROWSERS_PATH" : "") +
      (mode === "config-list" ? ",FVOCI_WEB_BUILD_PHASE" : ""),
    "setpriv",
    "--reuid=1000",
    "--regid=1000",
    "--groups=" + groups,
    ...(mode === "owner-return" ? [] : ["env", "TMPDIR=" + join(output, "tmp")]),
    process.execPath,
    cli,
    mode,
    "--output",
    output,
    ...(mode === "config-list" ? [] : laneArgs),
  ];
  const capture = async (args: string[], stdout: string, stderr?: string, cleanup = false) => {
    let out: number | undefined, err: number | undefined;
    try {
      out = openSync(join(diagnostics, stdout), "wx", 0o600);
      if (stderr) err = openSync(join(diagnostics, stderr), "wx", 0o600);
      return await command(
        args,
        root,
        process.env,
        { stdout: out, stderr: err ?? "inherit" },
        cleanup,
      );
    } catch {
      process.stderr.write("selected private capture failed\n");
      return 1;
    } finally {
      if (out !== undefined) closeSync(out);
      if (err !== undefined) closeSync(err);
    }
  };
  let config = "not-run",
    launcher = "not-run",
    selected = await command(["sudo", "chown", "-h", "-R", "1000:1000", output, sqlite], root);
  if (selected === 0)
    selected = await command(
      ["sudo", "install", "-d", "-o", "1000", "-g", "1000", "-m", "0700", join(output, "tmp")],
      root,
    );
  if (consume && selected === 0 && !entryCancellation) {
    selected = await capture(
      runtime("config-list", true),
      "config-list.stdout.log",
      "config-list.stderr.log",
    );
    config = String(selected);
  }
  if (selected === 0 && !entryCancellation) {
    selected = await command(runtime("run", true), root);
    launcher = String(selected);
  }
  selected ||= entryCancellation;
  const ownership = await capture(runtime("owner-return"), "ownership-stage.json", undefined, true);
  if (ownership === 0) {
    if (
      (await command(
        ["sudo", "chown", "-h", "-R", uid + ":" + gid, output, sqlite],
        root,
        process.env,
        { stdout: "inherit", stderr: "inherit" },
        true,
      )) !== 0
    ) {
      process.stderr.write("selected runtime ownership restoration failed\n");
      if (!selected) selected = 1;
    } else if (process.env.GITHUB_OUTPUT) {
      try {
        appendFileSync(
          process.env.GITHUB_OUTPUT,
          "selected-private-diagnostics<<FVOCI_CLOSED_DIAGNOSTICS\n" +
            [
              "*-stderr.log",
              "*-stage.json",
              "*-driver.log",
              "selected-ci-receipt.json",
              "handoff-input-before-safe.json",
              "handoff-input-after-safe.json",
              "handoff-input-current-safe.json",
              "handoff-input-delta-safe.json",
            ]
              .map((name) => join(output, name) + "\n")
              .join("") +
            "FVOCI_CLOSED_DIAGNOSTICS\n",
        );
      } catch {
        process.stderr.write("selected diagnostic publication failed\n");
        if (!selected) selected = 1;
      }
    }
  } else {
    process.stderr.write(
      "selected runtime ownership retained: resource retirement proof incomplete\n",
    );
    if (!selected) selected = 1;
  }
  selected ||= entryCancellation;
  try {
    writeLauncherReceipt(diagnostics, [
      launcher,
      String(ownership),
      String(selected),
      String(pending),
      config,
    ]);
  } catch {
    if (!selected) selected = 1;
  }
  return pending || selected;
}

async function browserServices(
  root: string,
  batch: string,
  network: string,
  platform: string,
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
        BROWSER_SERVICE_IMAGES[0],
        ["pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
      ],
      [
        meili,
        "fvoci-meili",
        BROWSER_SERVICE_IMAGES[1],
        ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:7700/health"],
      ],
    ] as const) {
      owned.push(name);
      checkedOutput(
        [
          "docker",
          "run",
          "--pull=never",
          "--platform",
          platform,
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
    if (interrupted || entryCancellation) throw new RuntimeExit(interrupted || entryCancellation);
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
  if (!process.env.FVOCI_WEB_CONTAINER_IMAGE || !["prepare", "inputs", "consume"].includes(phase))
    fail("ordinary container phase required");
  if (
    phase === "inputs" &&
    [
      "TEST_DATABASE_URL",
      "FVOCI_E2E_ADMIN_DATABASE_URL",
      "MEILI_MASTER_KEY",
      "FVOCI_MEILI_KEY",
    ].some((name) => process.env[name])
  )
    fail("browser dependency preparation requires a credential-free phase");
  process.env.ROOT = root;
  process.env.FVOCI_COLLAB_ENGINE = join(root, "crates/collab-engine/target/debug/collab-engine");
  process.env.FVOCI_WEB_BUILD_PHASE = phase === "prepare" ? "prepare" : "consume";
  process.env.CARGO_NET_OFFLINE = "true";
  try {
    qualifyCommittedApi(
      root,
      {
        shard: shards[0] ?? "",
        selected: false,
        browserPhase: phase === "prepare" ? "prepare" : "consume",
      },
      process.env,
    );
    if (phase === "prepare" && (shards.length || existsSync(join(root, "apps/web/dist"))))
      fail("producer requires absent dist and no shard");
    // Isolated jobs reconstruct their locked inputs; only the qualified native
    // packet crosses jobs. Local cohorts already share these bytes read-only.
    if (phase === "prepare" || phase === "inputs") {
      const target = nativeLinux(
        process.platform,
        process.arch,
        checkedOutput(["uname", "-m"], root),
        process.env.CARGO_BUILD_TARGET,
      ).target;
      const fetchEnv = { ...process.env, CARGO_NET_OFFLINE: "false" };
      await stage(
        "cargo-fetch",
        ["cargo", "fetch", "--locked", "--target", target],
        root,
        fetchEnv,
      );
      await stage(
        "engine-fetch",
        [
          "cargo",
          "fetch",
          "--locked",
          "--target",
          target,
          "--manifest-path",
          join(root, "crates/collab-engine/Cargo.toml"),
        ],
        root,
        fetchEnv,
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
    }
    if (phase === "inputs") return 0;
    const sdk = containerImage(
      process.env.FVOCI_WEB_CONTAINER_IMAGE,
      process.env.FVOCI_WEB_CONTAINER_SDK_SHA256 ?? "",
      process.env.FVOCI_WEB_CONTAINER_LOCAL_IMAGE === "1",
      process.env,
    );
    await stage(
      "sqlite-sdk-admit",
      [
        "/usr/local/bin/fvoci-ci-tools",
        "sqlite-ci",
        "--image-sdk",
        "/opt/fvoci-sqlite",
        "--expected-sdk-sha256",
        sdk.sdk,
        "--image-ref",
        sdk.image,
        ...(sdk.local ? ["--local-image-sdk"] : []),
        "--parent",
        "/fvoci-run/sqlite",
        "--env-file",
        "/fvoci-run/sqlite.env",
      ],
      root,
    );
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
      "--no-install",
      "--no-env-file",
      join(root, "tools/selected-backend-ci/handoff.ts"),
      ...args,
    ];
    if (phase === "prepare") {
      await stage("browser-input-before", handoff(["browser-before"]), root);
      await stage(
        "web-build",
        [process.execPath, "--bun", "run", "build"],
        join(root, "apps/web"),
        {
          ...process.env,
          CI: "true",
        },
      );
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
        plan = assignShards(groups, DEFAULT_SHARD_COUNT);
      verifyPlan(undefined, DEFAULT_SHARD_COUNT);
      await stage("browser-handoff-consume", handoff(["consume"]), root);
      for (const specs of plan[Number(shard)] ?? [])
        await stage(
          "browser-group",
          ["bash", join(root, "scripts/web-e2e-run-group.sh"), ...specs],
          root,
          { ...process.env, CI: "true" },
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
  const { databaseUrls } = await import("./database-urls.ts");
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
        process.execPath,
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
        process.execPath,
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
      [process.execPath, "--no-env-file", join(root, "tools/web-e2e/network-settle.ts")],
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
        process.execPath,
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

export function containerArchitecture(
  system: string,
  runtime: string,
  kernel: string,
  darwinArm?: string,
): NativeArchitecture {
  if (system !== "linux" && system !== "darwin")
    fail("a Linux or Darwin Docker client is required");
  if (system === "darwin" && darwinArm !== "0" && darwinArm !== "1")
    fail("physical Darwin architecture is required");
  const physical = nativeArchitecture(
    system === "darwin" ? (darwinArm === "1" ? "arm64" : "x64") : kernel,
  );
  if (nativeArchitecture(runtime) !== physical || nativeArchitecture(kernel) !== physical)
    fail("translated Docker clients are unsupported");
  return physical;
}

export function qualifyContainerDaemon(
  endpoint: string,
  info: string,
  arch: NativeArchitecture,
): void {
  if (!/^unix:\/\/\/[^\s?#\0]+$/.test(endpoint)) fail("a local Unix Docker endpoint is required");
  const match = /^linux (\S+)$/.exec(info);
  if (!match || nativeArchitecture(match[1] as string) !== arch)
    fail("Docker requires a native Linux daemon matching the physical client architecture");
}

export function containerImageSource(
  metadata: string,
  sdk: { image: string; local: boolean },
  arch: NativeArchitecture,
): string {
  const match = /^(sha256:[a-f0-9]{64}) linux (\S+) ([a-f0-9]{40}) (.+)$/.exec(metadata);
  if (!match || nativeArchitecture(match[2] as string) !== arch)
    fail("the pinned native ci-web image must match the client and daemon");
  let digests: unknown;
  try {
    digests = JSON.parse(match[4] as string);
  } catch {
    fail("invalid image digest metadata");
  }
  if (sdk.local ? match[1] !== sdk.image : !Array.isArray(digests) || !digests.includes(sdk.image))
    fail("the actual immutable image identity is required");
  return match[3] as string;
}

export function qualifyImageSource(root: string, imageSource: string, current: string): void {
  if (![imageSource, current].every((sha) => /^[a-f0-9]{40}$/.test(sha)))
    fail("exact image and consumer source revisions required");
  checkedOutput(
    [
      "git",
      "diff",
      "--quiet",
      imageSource,
      current,
      "--",
      "docker/ci-base/Dockerfile",
      "xtask/src",
      "xtask/Cargo.toml",
      "xtask/Cargo.lock",
      ":(glob)xtask/*.tsv",
      "tests/support/official_sqld.rs",
      "rust-toolchain.toml",
      ".bun-version",
    ],
    root,
  );
}

// Reuse never relabels a packet: the image still runs the original exact
// checkout and native qualifier. Only these host Docker functions may differ.
export async function qualifyCohortHostProgram(
  before: string,
  current: string,
  changed: string[],
): Promise<void> {
  if (
    changed.some(
      (path) =>
        ![
          "tools/web-e2e/run-web-e2e.ts",
          "tools/web-e2e/run-web-e2e.test.ts",
          "RUNNING.md",
        ].includes(path),
    )
  )
    fail("cohort reuse requires unchanged product and in-container sources");
  const ts = await import("typescript");
  const host = new Set(["containerBrowser", "browserServices", "qualifyCohortHostProgram"]);
  const program = (text: string) =>
    ts
      .createSourceFile("run-web-e2e.ts", text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
      .statements.filter(
        (node) => !ts.isFunctionDeclaration(node) || !host.has(node.name?.text ?? ""),
      )
      .map((node) => node.getText())
      .join("\n");
  if (program(before) !== program(current))
    fail("cohort reuse cannot replace any in-container program");
}

export function containerFailureArtifacts(
  output: string,
  runDirectory: string,
): string | undefined {
  if (Buffer.byteLength(output) > 16384) fail("container output exceeds diagnostic bound");
  const lines = output.split("\n").filter((line) => line.startsWith("failure-artifacts="));
  if (!lines.length) return undefined;
  const match =
    lines.length === 1 &&
    /^failure-artifacts=\/fvoci-run\/tmp\/(fvoci-collab-e2e-fail\.[A-Za-z0-9]+)$/.exec(
      lines[0] ?? "",
    );
  if (!match) fail("container failure diagnostics require an owned temporary directory");
  const directory = join(runDirectory, "tmp", match[1] ?? "");
  if (realpathSync(directory) !== directory || !isPrivateDirectory(directory))
    fail("container failure diagnostics are not private and physical");
  return directory;
}

export function containerRequest(
  argv: string[],
  env: NodeJS.ProcessEnv,
  architecture: string = process.arch,
) {
  const mode = argv[1];
  if (!argv[0] || !["--prepare", "--consume", "--ci-shard", "--ci-shards"].includes(mode ?? ""))
    throw new UsageError(
      "container-browser ROOT {--prepare|--consume N|--ci-shard N|--ci-shards N,N,...}",
    );
  const options = new Map<string, string>();
  for (let at = mode === "--prepare" ? 2 : 3; at < argv.length; at++) {
    const name = argv[at] ?? "";
    const value = argv[at + 1];
    if (options.has(name)) throw new UsageError("duplicate container option");
    if (name === "--local-image-sdk") options.set(name, "1");
    else if (
      ["--image-pin", "--sdk-sha256", "--reuse-cohort", "--handoff", "--handoff-sha256"].includes(
        name,
      ) &&
      value
    ) {
      options.set(name, value);
      at++;
    } else throw new UsageError("unknown or incomplete container option");
  }
  const imageOverride = options.get("--image-pin"),
    sdkOverride = options.get("--sdk-sha256");
  if ((imageOverride === undefined) !== (sdkOverride === undefined))
    fail("image and sealed SDK overrides must be supplied together");
  const sdk = containerImage(
    imageOverride ?? CI_WEB_IMAGE,
    sdkOverride ?? CI_WEB_SDK_SHA256[nativeArchitecture(architecture)],
    options.has("--local-image-sdk"),
    env,
  );
  const phase = mode === "--prepare" ? "prepare" : mode === "--consume" ? "consume" : "all";
  const shards = phase === "prepare" ? [] : (argv[2] ?? "").split(",").map(containerShard);
  if (
    new Set(shards).size !== shards.length ||
    (mode !== "--ci-shards" && phase !== "prepare" && shards.length !== 1)
  )
    fail("duplicate or invalid shard selection");
  const ci =
    env.GITHUB_ACTIONS !== undefined || Object.keys(env).some((key) => key.startsWith("GITHUB_"));
  if (
    ci &&
    (env.CI !== "true" ||
      env.GITHUB_ACTIONS !== "true" ||
      phase === "all" ||
      env.GITHUB_JOB !==
        (phase === "prepare" ? "workspace-browser-container-build" : "workspace-browser-shard") ||
      !/^[a-f0-9]{40}$/.test(env.GITHUB_SHA ?? "") ||
      !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? "") ||
      !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? "") ||
      !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(env.GITHUB_REPOSITORY ?? ""))
  )
    fail("container phase requires its actual GitHub source/run/attempt/repository/job");
  if (phase !== "all" && (!options.get("--handoff") || options.has("--reuse-cohort")))
    fail("separate container phases require an explicit packet path without cohort reuse");
  if (phase === "all" && (options.has("--handoff") || options.has("--handoff-sha256")))
    fail("combined local cohort cannot accept a cross-job packet");
  if (
    (phase === "consume" && !/^[a-f0-9]{64}$/.test(options.get("--handoff-sha256") ?? "")) ||
    (phase === "prepare" && options.has("--handoff-sha256"))
  )
    fail("consumer requires the successful producer packet SHA-256");
  return { sdk, phase, shards, ci, options };
}

/** Shared local/CI execution: one pinned image and the existing qualified packet. */
async function containerBrowser(argv: string[]): Promise<number> {
  const { sdk, phase: requestedPhase, shards, ci, options } = containerRequest(argv, process.env);
  const root = realpathSync(argv[0] ?? fail("container root required"));
  const architecture = containerArchitecture(
    process.platform,
    process.arch,
    checkedOutput(["uname", "-m"], root),
    process.platform === "darwin"
      ? checkedOutput(["sysctl", "-n", "hw.optional.arm64"], root)
      : undefined,
  );
  const uid = process.getuid?.(),
    gid = process.getgid?.();
  if (!uid || gid === undefined) fail("a non-root POSIX Docker client is required");
  const orchestratorSource = checkedOutput(["git", "rev-parse", "HEAD"], root);
  if (ci && orchestratorSource !== process.env.GITHUB_SHA)
    fail("container checkout differs from tested SHA");
  checkedOutput(["git", "diff", "--quiet", "HEAD", "--"], root);
  let endpoint: unknown = process.env.DOCKER_HOST;
  if (process.env.DOCKER_CONTEXT || !endpoint) {
    const json = checkedOutput(
      [
        "docker",
        "context",
        "inspect",
        "--format",
        "{{json .Endpoints.docker.Host}}",
        process.env.DOCKER_CONTEXT || checkedOutput(["docker", "context", "show"], root),
      ],
      root,
    );
    try {
      endpoint = JSON.parse(json);
    } catch {
      fail("invalid Docker endpoint metadata");
    }
  }
  if (typeof endpoint !== "string") fail("a local Docker endpoint is required");
  qualifyContainerDaemon(
    endpoint,
    checkedOutput(["docker", "info", "--format", "{{.OSType}} {{.Architecture}}"], root),
    architecture,
  );
  const platform = "linux/" + (architecture === "x86_64" ? "amd64" : "arm64");
  if (
    process.env.DOCKER_DEFAULT_PLATFORM !== undefined &&
    process.env.DOCKER_DEFAULT_PLATFORM !== platform
  )
    fail("Docker platform override must match the native architecture");
  const image = sdk.image;
  if (ci) await stage("image-fetch", ["docker", "pull", "--platform", platform, image], root);
  const metadata = checkedOutput(
    [
      "docker",
      "image",
      "inspect",
      "--format",
      '{{.Id}} {{.Os}} {{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}} {{json .RepoDigests}}',
      image,
    ],
    root,
  );
  const imageSource = containerImageSource(metadata, sdk, architecture);
  qualifyImageSource(root, imageSource, orchestratorSource);
  let source = orchestratorSource;
  checkedOutput(["git", "diff", "--quiet", "HEAD", "--"], root);
  const common = realpathSync(
    resolve(root, checkedOutput(["git", "rev-parse", "--git-common-dir"], root)),
  );
  const batch = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-container-browser-")));
  if (statSync(batch).uid !== uid || (statSync(batch).mode & 0o777) !== 0o700)
    fail("the owned private browser batch is required");
  chmodSync(batch, 0o700);
  const reuse = options.get("--reuse-cohort");
  const cohort = reuse === undefined ? batch : realpathSync(reuse);
  let run = randomBytes(16).toString("hex");
  const resources = randomBytes(16).toString("hex");
  const packet = options.get("--handoff");
  if (packet) {
    if (
      !isAbsolute(packet) ||
      resolve(packet) !== packet ||
      realpathSync(dirname(packet)) !== dirname(packet) ||
      statSync(dirname(packet)).uid !== uid ||
      (requestedPhase === "prepare" && existsSync(packet))
    )
      fail("packet requires an owned physical parent and an absent producer destination");
    if (requestedPhase === "consume") {
      if (
        realpathSync(packet) !== packet ||
        !statSync(packet).isDirectory() ||
        statSync(packet).uid !== uid
      )
        fail("consumer packet must be an owned physical directory");
      for (const name of ["handoff.json", "payload.tar"]) {
        const file = join(packet, name);
        if (
          lstatSync(file).isSymbolicLink() ||
          !statSync(file).isFile() ||
          statSync(file).uid !== uid
        )
          fail("consumer packet requires owned regular files");
      }
      if (
        createHash("sha256")
          .update(readFileSync(join(packet, "handoff.json")))
          .digest("hex") !== options.get("--handoff-sha256")
      )
        fail("consumer packet differs from successful producer digest");
      if (!ci) {
        const manifest: unknown = JSON.parse(readFileSync(join(packet, "handoff.json"), "utf8"));
        if (!manifest || typeof manifest !== "object" || Array.isArray(manifest))
          fail("invalid local packet");
        const value = manifest as Record<string, unknown>;
        if (
          value.source !== source ||
          value.root !== root ||
          value.repository !== "local-container" ||
          value.container_image !== image ||
          value.container_sdk_sha256 !== sdk.sdk ||
          value.container_image_source !== imageSource ||
          typeof value.run !== "string" ||
          !/^[a-f0-9]{32}$/.test(value.run)
        )
          fail("local packet source/image/run mismatch");
        run = value.run;
      }
      mkdirSync(join(batch, "handoff"), { mode: 0o700 });
      for (const name of ["handoff.json", "payload.tar"])
        copyFileSync(join(packet, name), join(batch, "handoff", name));
    }
  }
  if (cohort !== batch) {
    if (statSync(cohort).uid !== uid || (statSync(cohort).mode & 0o777) !== 0o700)
      fail("owned private producer cohort required");
    const value: unknown = JSON.parse(readFileSync(join(cohort, "handoff/handoff.json"), "utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value))
      fail("invalid producer cohort");
    const manifest = value as Record<string, unknown>;
    if (
      typeof manifest.source !== "string" ||
      !/^[a-f0-9]{40}$/.test(manifest.source) ||
      typeof manifest.run !== "string" ||
      manifest.container_image !== image ||
      manifest.container_sdk_sha256 !== sdk.sdk ||
      manifest.container_image_source !== imageSource ||
      manifest.root !== root ||
      manifest.repository !== "local-container"
    )
      fail("producer cohort source/image/path mismatch");
    source = manifest.source;
    run = manifest.run;
    checkedOutput(["git", "merge-base", "--is-ancestor", source, orchestratorSource], root);
    const changed = checkedOutput(
      ["git", "diff", "--name-only", source, orchestratorSource, "--"],
      root,
    )
      .split("\n")
      .filter(Boolean);
    await qualifyCohortHostProgram(
      checkedOutput(["git", "show", source + ":tools/web-e2e/run-web-e2e.ts"], root),
      readFileSync(join(root, "tools/web-e2e/run-web-e2e.ts"), "utf8"),
      changed,
    );
  }
  process.stderr.write(
    `container-browser orchestrator_source=${orchestratorSource} build_source=${source} consumer_source=${source} image_source=${imageSource} sdk_sha256=${sdk.sdk} reuse=${String(cohort !== batch)}\n`,
  );
  if (!/^[a-f0-9]{32}$/.test(run)) fail("invalid browser batch identity");
  const producer = join(cohort, "producer");
  {
    for (const directory of [
      ...(cohort === batch && requestedPhase !== "consume"
        ? [producer, "cargo", "home", "tmp", "sqlite", "producer-output"]
        : []),
      ...shards.map((shard) => join(batch, "consumer-" + shard)),
      ...shards.map((shard) => "consumer-output-" + shard),
    ].map((p) => (p.startsWith(batch) ? p : join(batch, p))))
      mkdirSync(directory, { mode: directory.endsWith("/tmp") ? 0o711 : 0o700 });
    // Only Git objects are mounted read-only, not account config or credentials. Both source
    // copies occupy the same physical path inside the image, preserving the
    // existing packet's no-relocation and full-current-input checks.
    for (const directory of [
      ...(cohort === batch && requestedPhase !== "consume" ? [producer] : []),
      ...shards.map((shard) => join(batch, "consumer-" + shard)),
    ]) {
      checkedOutput(
        ["git", "clone", "--shared", "--no-checkout", "--quiet", root, directory],
        root,
      );
      checkedOutput(["git", "checkout", "--quiet", "--detach", source], directory);
    }
  }
  for (const shard of shards) {
    const runDirectory = join(batch, "run-" + shard);
    mkdirSync(runDirectory, { mode: 0o700 });
    for (const name of ["home", "tmp", "cargo", "sqlite", "handoff", "output", "vite-temp"])
      mkdirSync(join(runDirectory, name), { mode: name === "tmp" ? 0o711 : 0o700 });
  }
  const consumerDeadlines = new Map<string, number>();
  async function runContainer(
    phase: "prepare" | "inputs" | "consume",
    services: NodeJS.ProcessEnv = {},
    shard = "",
  ): Promise<number> {
    if (entryCancellation) return entryCancellation;
    const now = performance.now();
    if (phase !== "prepare" && !consumerDeadlines.has(shard))
      consumerDeadlines.set(shard, now + 20 * 60_000);
    const remaining =
      phase === "prepare" ? 15 * 60_000 : (consumerDeadlines.get(shard) ?? now) - now;
    if (remaining <= 0) return 1;
    const directory = phase === "prepare" ? producer : join(batch, "consumer-" + shard);
    const runDirectory = phase === "prepare" ? batch : join(batch, "run-" + shard);
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
      ...(ci
        ? {
            CI: "true",
            GITHUB_ACTIONS: "true",
            GITHUB_SHA: process.env.GITHUB_SHA ?? "",
            GITHUB_REPOSITORY: process.env.GITHUB_REPOSITORY ?? "",
            GITHUB_RUN_ID: process.env.GITHUB_RUN_ID ?? "",
            GITHUB_RUN_ATTEMPT: process.env.GITHUB_RUN_ATTEMPT ?? "",
            GITHUB_JOB: process.env.GITHUB_JOB ?? "",
            GITHUB_OUTPUT: "/fvoci-run/github-output",
          }
        : { FVOCI_WEB_CONTAINER_LOCAL: "1" }),
      FVOCI_WEB_CONTAINER_IMAGE: image,
      FVOCI_WEB_CONTAINER_LOCAL_IMAGE: sdk.local ? "1" : "0",
      FVOCI_WEB_CONTAINER_SDK_SHA256: sdk.sdk,
      FVOCI_WEB_CONTAINER_IMAGE_SOURCE: imageSource,
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
      variables.FVOCI_WEB_BUILD_HANDOFF_SHA256 =
        options.get("--handoff-sha256") ??
        createHash("sha256")
          .update(readFileSync(join(cohort, "handoff/handoff.json")))
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
    const name = "fvoci-browser-" + resources + "-" + phase + shard;
    const args = [
      "docker",
      "run",
      "--pull=never",
      "--platform",
      platform,
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
      [runDirectory, "/fvoci-run", false],
      // Bun's node shim uses /tmp, independently of TMPDIR. Hide the image
      // build's root-owned shim with the same owned ordinary-disk temp dir.
      [join(runDirectory, "tmp"), "/tmp", false],
      [join(batch, output), "/fvoci-run/output", false],
    ] as const) {
      if (from.includes(",") || to.includes(",")) fail("Docker bind paths cannot contain commas");
      args.push("--mount", `type=bind,source=${from},target=${to}${readonly ? ",readonly" : ""}`);
    }
    if (phase === "consume") {
      args.push("--network", "fvoci-browser-" + resources + "-" + shard);
      if (requestedPhase === "all")
        args.push(
          "--mount",
          `type=bind,source=${producer}/node_modules,target=${root}/node_modules,readonly`,
        );
      for (const [from, to] of [
        [join(cohort, "handoff"), "/fvoci-run/handoff"],
        ...(requestedPhase === "all" ? ["registry", "git", "config", "config.toml"] : [])
          .filter((name) => existsSync(join(cohort, "cargo", name)))
          .map((name) => [join(cohort, "cargo", name), "/fvoci-run/cargo/" + name]),
      ]) {
        if (!from || !to || from.includes(",")) fail("invalid shared native input mount");
        args.push("--mount", `type=bind,source=${from},target=${to},readonly`);
      }
      if (requestedPhase === "all")
        args.push(
          "--mount",
          `type=bind,source=${runDirectory}/vite-temp,target=${root}/node_modules/.vite-temp`,
        );
    }
    args.push(
      image,
      "bun",
      "--no-install",
      "--no-env-file",
      join(root, "tools/web-e2e/run-web-e2e.ts"),
      "browser-phase",
      root,
      phase,
      ...(phase === "prepare" ? [] : [shard]),
    );
    const started = performance.now();
    const logFd =
      phase !== "prepare"
        ? openSync(
            join(runDirectory, phase === "consume" ? "console.log" : "inputs-console.log"),
            "wx",
            0o600,
          )
        : undefined;
    let child;
    try {
      child = Bun.spawn(args, {
        stdin: "ignore",
        stdout: logFd ?? "inherit",
        stderr: logFd ?? "inherit",
        timeout: Math.ceil(remaining),
      });
    } catch (error) {
      rmSync(envFile);
      throw error;
    } finally {
      if (logFd !== undefined) closeSync(logFd);
    }
    const interrupt = () => {
        entryCancellation ||= 130;
        child.kill("SIGINT");
      },
      terminate = () => {
        entryCancellation ||= 143;
        child.kill("SIGTERM");
      };
    process.once("SIGINT", interrupt);
    process.once("SIGTERM", terminate);
    let result: number;
    try {
      result = await child.exited;
      result ||= entryCancellation;
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
    if (ci && phase === "consume") {
      try {
        const output = process.env.GITHUB_OUTPUT;
        if (!output) fail("consumer GitHub output path required");
        const retained = join(runDirectory, "github-output");
        if (existsSync(retained)) {
          const directory = containerFailureArtifacts(readFileSync(retained, "utf8"), runDirectory);
          if (directory) appendFileSync(output, "failure-artifacts=" + directory + "\n");
        }
        appendFileSync(
          output,
          "handoff-diagnostics=" + join(batch, "consumer-output-" + shard) + "\n",
        );
      } catch {
        process.stderr.write("container browser safe diagnostic retention failed\n");
        result ||= 1;
      }
    }
    process.stderr.write(
      `container-browser phase=${phase} shard=${shard || "producer"} elapsed_seconds=${((performance.now() - started) / 1000).toFixed(3)} exit=${String(result)}${phase === "consume" ? " log=" + join(runDirectory, "console.log") : ""}\n`,
    );
    return result;
  }
  if (cohort === batch && requestedPhase !== "consume") {
    const prepared = await runContainer("prepare");
    if (prepared !== 0) return prepared;
  }
  if (requestedPhase === "prepare") {
    if (!packet) fail("producer packet destination missing");
    mkdirSync(packet, { mode: 0o700 });
    for (const name of ["handoff.json", "payload.tar"])
      copyFileSync(join(batch, "handoff", name), join(packet, name));
    if (ci) {
      const output = process.env.GITHUB_OUTPUT;
      if (!output) fail("producer GitHub output path required");
      appendFileSync(
        output,
        "handoff_sha256=" +
          createHash("sha256")
            .update(readFileSync(join(packet, "handoff.json")))
            .digest("hex") +
          "\n",
      );
    }
    return 0;
  }
  // Only an empty mountpoint is added: no dependency file/input hash changes.
  if (requestedPhase === "all")
    mkdirSync(join(producer, "node_modules/.vite-temp"), { recursive: true });
  if (ci && requestedPhase === "consume")
    for (const image of BROWSER_SERVICE_IMAGES)
      await stage("service-image-fetch", ["docker", "pull", "--platform", platform, image], root);
  let firstFailure = 0;
  for (let at = 0; at < shards.length; at += 2) {
    if (entryCancellation) return firstFailure || entryCancellation;
    const settled = await Promise.allSettled(
      shards.slice(at, at + 2).map(async (shard) => {
        if (requestedPhase !== "all") {
          const prepared = await runContainer("inputs", {}, shard);
          if (prepared !== 0 || entryCancellation) {
            firstFailure ||= prepared || entryCancellation;
            return;
          }
        }
        const status = await browserServices(
          root,
          batch,
          "fvoci-browser-" + resources + "-" + shard,
          platform,
          async (services) => {
            const result = await runContainer("consume", services, shard);
            if (result !== 0 && firstFailure === 0) firstFailure = result;
            return result;
          },
        );
        if (status !== 0 && firstFailure === 0) firstFailure = status;
      }),
    );
    for (const result of settled)
      if (result.status === "rejected") {
        if (firstFailure === 0)
          firstFailure = result.reason instanceof RuntimeExit ? result.reason.status : 1;
        process.stderr.write("container-browser consumer setup failed\n");
      }
    if (firstFailure) return firstFailure;
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
  if (request.browserPhase === "prepare")
    job = githubJob === "workspace-browser-container-build" ? githubJob : "workspace-browser-build";
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
    containerImage(
      env.FVOCI_WEB_CONTAINER_IMAGE ?? "",
      env.FVOCI_WEB_CONTAINER_SDK_SHA256 ?? "",
      env.FVOCI_WEB_CONTAINER_LOCAL_IMAGE === "1",
      env,
    );
    if (request.selected || Object.keys(env).some((key) => key.startsWith("GITHUB_")))
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
    case "require-browser-producer":
      arity(rest, 0);
      if (
        process.env.PRODUCER_RESULT !== "success" ||
        !/^[1-9][0-9]*$/.test(process.env.PRODUCER_ARTIFACT ?? "") ||
        !/^[a-f0-9]{64}$/.test(process.env.PRODUCER_DIGEST ?? "")
      )
        fail("successful immutable browser producer artifact and digest required");
      return "";
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
    if (argv[0] === "entry") process.exit(await webEntry(argv[1] ?? "", argv.slice(2)));
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
