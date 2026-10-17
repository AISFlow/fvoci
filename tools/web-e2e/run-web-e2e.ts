import {
  CI_WEB_IMAGE,
  CI_WEB_SDK_SHA256,
  SELECTED_IMAGE_ENV,
  selectedImageHost,
  selectedImageResult,
  finishToolImage,
  containerImage,
  containerArchitecture,
  qualifyContainerDaemon,
  containerImageSource,
  qualifyImageSource,
  CheckError,
  RuntimeExit,
  dockerEndpoint,
  checkedOutput,
  fail,
} from "../selected-backend-ci/runtime.ts";
import type { SelectedImageOperation } from "../selected-backend-ci/runtime.ts";
export {
  CI_WEB_IMAGE,
  CI_WEB_IMAGE_SOURCE,
  containerImage,
  selectedImageHost,
  selectedImageBudget,
  SELECTED_IMAGE_ENV,
  selectedImageInputs,
  selectedImageAccess,
  selectedImageConfigList,
  selectedImageBrowser,
  containerArchitecture,
  qualifyContainerDaemon,
  containerImageSource,
  qualifyImageSource,
  CheckError,
} from "../selected-backend-ci/runtime.ts";
// Checks scripts/run-web-e2e.sh runs as `bun tools/web-e2e/run-web-e2e.ts
// <command> ...`: committed API qualification, shard plan line parsing, and
// the selected companion's SQLite prefix containment, private diagnostics
// directory and launcher stage receipt. A refusal prints one stderr line and
// exits 1; a usage error exits 2.
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  cpSync,
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readlinkSync,
  readdirSync,
  realpathSync,
  statSync,
  statfsSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { randomBytes, createHash } from "node:crypto";
import process from "node:process";
import type { Inputs } from "../selected-backend-ci/types.ts";
import { nativeArchitecture, nativeLinux } from "../selected-backend-ci/types.ts";
import {
  validateSpecRelpath,
  discoverGroups,
  assignShards,
  verifyPlan,
  DEFAULT_SHARD_COUNT,
  TIMER_FILTERS,
} from "./groups.ts";
const BROWSER_SERVICE_IMAGES = [
  "postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336",
  "getmeili/meilisearch:v1.55.0@sha256:ff546679970126f8822e15d1b6a06bfbff1f2ce2e43f810cfb5258012f92de5c",
] as const;
async function selectedImageOperation(
  operation: SelectedImageOperation,
  root: string,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<number> {
  const started = performance.now(),
    log = join(
      process.env.FVOCI_SELECTED_CI_OUTPUT ?? fail("selected output required"),
      operation + ".private.log",
    );
  let status = 1;
  try {
    const result = await selectedImageResult(operation, root, {
      env: environment,
      required: false,
      log,
    });
    status = result.returncode;
    process.stderr.write(
      "selected-image phase=" +
        operation +
        " elapsed_seconds=" +
        ((performance.now() - started) / 1000).toFixed(3) +
        " exit=" +
        String(status) +
        "\n",
    );
    return status;
  } finally {
    if (status !== 0)
      try {
        for (const line of containerStageDiagnostics(log, true)) process.stderr.write(line + "\n");
      } catch {
        process.stderr.write("selected-image diagnostics_unavailable=1\n");
      }
  }
}

function containerStageDiagnostics(path: string, selected = false): string[] {
  const size = statSync(path).size,
    bytes = Buffer.alloc(Math.min(size, 16384)),
    offset = size - bytes.length,
    fd = openSync(path, "r");
  try {
    readSync(fd, bytes, 0, bytes.length, offset);
  } finally {
    closeSync(fd);
  }
  return bytes
    .toString("utf8")
    .split("\n")
    .slice(offset ? 1 : 0)
    .filter(
      (line) =>
        /^(?:web-e2e stage=(?:cargo-fetch|engine-fetch|dependencies|browser-version|sqlite-sdk-admit|browser-input-before|web-build|fixture-build|default-server-build|worker-build|browser-input-after|browser-handoff-export|browser-handoff-consume|browser-group) (?:started|elapsed_seconds=[0-9]+\.[0-9]{3} exit=[0-9]{1,3})|web build handoff check_line=[0-9]{1,6})$/.test(
          line,
        ) ||
        (selected &&
          /^web-e2e stage=(?:selected-(?:dependencies|handoff-admit|cargo-fetch|sqlite-sdk|input-before|lib|install|engine|input-after|handoff-consume|handoff-export)|collaboration-(?:wire-restart-fixtures|helper-fixtures|types)|web-build) (?:started(?: at=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z)?|(?:finished at=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z )?elapsed_seconds=[0-9]+\.[0-9]{3} exit=[0-9]{1,3})$/.test(
            line,
          )),
    );
}

async function selectedImagePhase(root: string, operation: string): Promise<number> {
  if (
    process.env.FVOCI_SELECTED_IMAGE_WORKLOAD !== "1" ||
    process.env.FVOCI_WEB_CONTAINER_IMAGE !== CI_WEB_IMAGE ||
    process.env.FVOCI_WEB_CONTAINER_SDK_SHA256 !== CI_WEB_SDK_SHA256.x86_64
  )
    fail("fixed selected tool image required");
  const output = process.env.FVOCI_SELECTED_CI_OUTPUT ?? fail("selected output required"),
    sqlite = process.env.FVOCI_SELECTED_CI_SQLITE_PARENT ?? fail("selected SQLite parent required");
  if (operation === "inputs") {
    const before = JSON.parse(readFileSync(0, "utf8")) as Inputs;
    const { inputCheck } = await import("../selected-backend-ci/drivers/common.ts");
    const checked = await inputCheck(before, before.head, before.tree);
    process.stdout.write(JSON.stringify(checked));
    return 0;
  }
  if (operation === "access") {
    const { preflight } = await import("../selected-backend-ci/runtime-access-check.ts");
    const request = JSON.parse(readFileSync(0, "utf8")) as Parameters<typeof preflight>[0];
    const receipt = preflight(request);
    process.stdout.write(JSON.stringify(receipt));
    return receipt.missing ? 1 : 0;
  }
  if (operation === "config-list") {
    const { configList } = await import("../selected-backend-ci/config-list.ts");
    return configList(output);
  }
  if (operation === "pending") {
    process.env.FVOCI_COLLAB_ENGINE = join(root, "crates/collab-engine/target/debug/collab-engine");
    return groupWrapper([]);
  }
  if (!["prepare", "consume"].includes(operation)) fail("unknown selected image operation");
  const target = nativeLinux(
    process.platform,
    process.arch,
    checkedOutput(["uname", "-m"], root),
    process.env.CARGO_BUILD_TARGET,
  ).target;
  if (target !== "x86_64-unknown-linux-gnu") fail("selected targets remain native x64");
  await stage(
    "selected-dependencies",
    [process.execPath, "--bun", "install", "--frozen-lockfile"],
    root,
  );
  if (operation === "consume")
    await stage(
      "selected-handoff-admit",
      [
        process.execPath,
        "--no-env-file",
        join(root, "tools/selected-backend-ci/handoff.ts"),
        "admit",
      ],
      root,
    );
  for (const manifest of [join(root, "Cargo.toml"), join(root, "crates/collab-engine/Cargo.toml")])
    await stage(
      "selected-cargo-fetch",
      ["cargo", "fetch", "--locked", "--target", target, "--manifest-path", manifest],
      root,
      { ...process.env, CARGO_NET_OFFLINE: "false" },
    );
  if (operation === "consume") {
    await stage(
      "collaboration-wire-restart-fixtures",
      [
        process.execPath,
        "test",
        "--isolate",
        "--timeout=60000",
        "./apps/web/e2e-pending/collab-wire.test.ts",
        "./apps/web/e2e-pending/collab-restart.test.ts",
      ],
      root,
    );
    await stage(
      "collaboration-helper-fixtures",
      [
        process.execPath,
        "test",
        "--isolate",
        "--timeout=60000",
        "./apps/web/e2e-pending/collab-helpers.test.ts",
      ],
      root,
    );
    await stage(
      "collaboration-types",
      [
        process.execPath,
        "--bun",
        "x",
        "--no-install",
        "tsc",
        "-p",
        "e2e-pending/collab-tsconfig.json",
      ],
      join(root, "apps/web"),
    );
  }
  const tools = join(output, "tools");
  mkdirSync(tools, { mode: 0o755 });
  copyFileSync(process.execPath, join(tools, "bun"));
  chmodSync(join(tools, "bun"), 0o755);
  await stage(
    "selected-sqlite-sdk",
    [
      "/usr/local/bin/fvoci-ci-tools",
      "sqlite-ci",
      "--image-sdk",
      "/opt/fvoci-sqlite",
      "--expected-sdk-sha256",
      CI_WEB_SDK_SHA256.x86_64,
      "--image-ref",
      CI_WEB_IMAGE,
      "--parent",
      sqlite,
      "--env-file",
      join(sqlite, "environment"),
    ],
    root,
  );
  Object.assign(process.env, sqliteEnvironment(readFileSync(join(sqlite, "environment"), "utf8")));
  const { prepareBrowser } = await import("../selected-backend-ci/runtime.ts");
  const browserSource = join(output, "browser-source");
  process.env.PLAYWRIGHT_BROWSERS_PATH = browserSource;
  const chromium = checkedOutput(
    [
      process.execPath,
      "--eval",
      "console.log(require('@playwright/test').chromium.executablePath())",
    ],
    root,
  );
  prepareBrowser(output, chromium);
  rmSync(browserSource, { recursive: true });
  process.env.PLAYWRIGHT_BROWSERS_PATH = join(output, "browser");
  process.env.CARGO_NET_OFFLINE = "true";
  return webEntry(root, [
    "--ci-use-committed-api",
    operation === "prepare" ? "--ci-prepare-selected" : "--ci-consume-selected",
  ]);
}

async function selectedContainerEntry(root: string, argv: string[]): Promise<number> {
  root = realpathSync(root);
  process.env.FVOCI_SELECTED_IMAGE_DEADLINE = String(Date.now() + 900_000);
  const request = webRequest(argv, process.env);
  if (
    !request.selected ||
    !request.committed ||
    !["prepare", "consume"].includes(request.selectedPhase) ||
    process.env.CI !== "true" ||
    process.env.GITHUB_ACTIONS !== "true"
  )
    fail("selected images require the existing explicit committed CI phases");
  qualifyCommittedApi(root, request, process.env);
  const architecture = containerArchitecture(
    process.platform,
    process.arch,
    checkedOutput(["uname", "-m"], root),
  );
  if (architecture !== "x86_64") fail("selected collaboration remains native x64");
  qualifyContainerDaemon(
    dockerEndpoint(root),
    checkedOutput(["docker", "info", "--format", "{{.OSType}} {{.Architecture}}"], root),
    architecture,
  );
  await stage(
    "selected-image-fetch",
    ["docker", "pull", "--platform", "linux/amd64", CI_WEB_IMAGE],
    root,
  );
  const sdk = containerImage(CI_WEB_IMAGE, CI_WEB_SDK_SHA256.x86_64, false, process.env);
  const imageSource = containerImageSource(
    checkedOutput(
      [
        "docker",
        "image",
        "inspect",
        "--format",
        '{{.Id}} {{.Os}} {{.Architecture}} {{index .Config.Labels "org.opencontainers.image.revision"}} {{json .RepoDigests}}',
        CI_WEB_IMAGE,
      ],
      root,
    ),
    sdk,
    architecture,
  );
  qualifyImageSource(root, imageSource, process.env.GITHUB_SHA ?? "");
  const temp = realpathSync(process.env.RUNNER_TEMP ?? fail("RUNNER_TEMP required")),
    output = join(temp, "fvoci-selected-current"),
    sqlite = join(temp, "fvoci-sqlite"),
    cargo = join(temp, "fvoci-image-cargo");
  if (
    process.env.FVOCI_SELECTED_CI_OUTPUT !== output ||
    process.env.FVOCI_SELECTED_CI_SQLITE_PARENT !== sqlite ||
    process.env.FVOCI_WEB_BUILD_HANDOFF !== join(temp, "fvoci-web-build-handoff")
  )
    fail("selected canonical paths required");
  for (const path of [sqlite, cargo, join(output, "tmp"), join(output, "home")])
    mkdirSync(path, { mode: path === cargo ? 0o750 : 0o700 });
  if (request.selectedPhase === "prepare")
    mkdirSync(process.env.FVOCI_WEB_BUILD_HANDOFF, {
      mode: 0o700,
    });
  // Download cache only. Installed dependencies and native outputs are always
  // materialized and checked inside the admitted image.
  for (const name of ["registry", "git"])
    if (process.env.HOME && existsSync(join(process.env.HOME, ".cargo", name)))
      cpSync(join(process.env.HOME, ".cargo", name), join(cargo, name), {
        recursive: true,
        errorOnExist: true,
        force: false,
      });
  Object.assign(process.env, {
    FVOCI_WEB_CONTAINER_IMAGE: CI_WEB_IMAGE,
    FVOCI_WEB_CONTAINER_SDK_SHA256: CI_WEB_SDK_SHA256.x86_64,
    FVOCI_WEB_CONTAINER_IMAGE_SOURCE: imageSource,
    FVOCI_WEB_BUILD_PHASE: request.selectedPhase,
    CARGO_HOME: cargo,
    CARGO_TARGET_DIR: join(root, "target"),
    CARGO_BUILD_JOBS: "2",
    LIBCLANG_PATH: "/usr/lib/llvm-18/lib",
    SQLITE3_LIB_DIR: "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/lib",
    SQLITE3_INCLUDE_DIR: "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/include",
    SQLITE3_STATIC: "1",
    SQLITE3_NO_PKG_CONFIG: "1",
    PLAYWRIGHT_BROWSERS_PATH: join(output, "browser"),
  });
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(([key]) =>
      /^(CI|GITHUB_(?:ACTIONS|SHA|REPOSITORY|RUN_ID|RUN_ATTEMPT|JOB|OUTPUT)|RUNNER_TEMP|LANG|LC_ALL|CARGO_(?:BUILD_JOBS|INCREMENTAL|PROFILE_DEV_DEBUG|PROFILE_TEST_DEBUG)|FVOCI_E2E_PENDING|FVOCI_WEB_BUILD_HANDOFF(?:_SHA256)?)$/.test(
        key,
      ),
    ),
  );
  const prepared = await selectedImageOperation(
    request.selectedPhase as "prepare" | "consume",
    root,
    environment,
  );
  if (prepared === 0 && request.selectedPhase === "prepare" && process.env.HOME)
    for (const name of ["registry", "git"])
      if (existsSync(join(cargo, name)))
        cpSync(join(cargo, name), join(process.env.HOME, ".cargo", name), { recursive: true });
  if (prepared || request.selectedPhase === "prepare") return prepared;
  // Existing host wrappers own PG/Meili. The inner group sees their real
  // loopback bindings through Linux host networking, never their Docker socket.
  const pending = await command(
    [
      "bash",
      join(root, "scripts/start-test-postgres.sh"),
      "bash",
      join(root, "scripts/start-test-meili.sh"),
      process.execPath,
      "--no-env-file",
      import.meta.path,
      "selected-image-pending",
      root,
    ],
    root,
  );
  process.env.PATH = join(output, "tools") + ":" + (process.env.PATH ?? "");
  return selectedAfterPending(pending, () => selectedHost(root, pending, true));
}

export async function selectedAfterPending(
  pending: number,
  body: () => Promise<number>,
): Promise<number> {
  try {
    const status = await body();
    return pending || status;
  } catch (error) {
    if (pending) return pending;
    throw error;
  }
}

export function selectedPermissionDiagnostics(text: string): string[] {
  if (Buffer.byteLength(text) > 16384) return [];
  const lines = text.split("\n"),
    result: string[] = [];
  for (const [prefix, pattern] of [
    [
      "selected permissions descendant ",
      /^selected permissions descendant prefix=(?:output|sqlite) (?:symlink=1 owner_match=[01]|symlink=0 owner_match=0)$/,
    ],
    [
      "selected permissions failure at ",
      /^selected permissions failure at (?:tools\/selected-backend-ci\/(?:runtime|admission|io)|scripts\/run-selected-backend-e2e)\.ts:[1-9][0-9]{0,3}$/,
    ],
  ] as const) {
    const matches = lines.filter((line) => line.startsWith(prefix));
    if (matches.length === 1 && matches[0] && pattern.test(matches[0])) result.push(matches[0]);
  }
  return result;
}

// Each admitted ordinary-browser shard retains every registered group.
// Host-owned service inspection is relayed and revalidated without a socket.
export function containerShard(value: string | undefined): string {
  if (!value || !["0", "1", "2", "3", "4", "5", "6", "7"].includes(value))
    fail("shared containers require one complete shard 0..7");
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
    specs.length > 0;
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
    route: hostRequired ? ("host" as const) : ("docker" as const),
  };
}

async function webEntry(root: string, argv: string[]): Promise<number> {
  const request = webRequest(argv, process.env);
  if (
    request.selected &&
    request.selectedPhase !== "whole" &&
    process.env.FVOCI_SELECTED_IMAGE_WORKLOAD !== "1"
  )
    return selectedContainerEntry(root, argv);
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
    const dockerShards =
      shard || Array.from({ length: DEFAULT_SHARD_COUNT }, (_, at) => at).join(",");
    process.stderr.write(`web-e2e route=docker shards=${dockerShards}\n`);
    return containerBrowser([root, "--ci-shards", dockerShards, ...request.imageOptions]);
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
  if (selectedPhase === "consume" && process.env.FVOCI_SELECTED_IMAGE_WORKLOAD !== "1")
    await stage("selected-handoff-admit", handoff("admit"), root);
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
    } else if (process.env.FVOCI_SELECTED_IMAGE_WORKLOAD === "1") {
      const sqlite =
        process.env.FVOCI_SELECTED_CI_SQLITE_PARENT ?? fail("selected SDK parent required");
      Object.assign(
        process.env,
        sqliteEnvironment(readFileSync(join(sqlite, "environment"), "utf8")),
      );
      if (
        process.env.SQLITE3_LIB_DIR !== "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/lib" ||
        process.env.SQLITE3_INCLUDE_DIR !== "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/include"
      )
        fail("selected workload requires the admitted immutable image SDK");
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
      for (const name of ["lib", "install", "engine"]) await native(name, "selected");
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
    command([process.execPath, "--no-env-file", import.meta.path, "group-wrapper", ...args], root);
  if (shard) {
    verifyPlan(undefined, DEFAULT_SHARD_COUNT);
    const plan = assignShards(discoverGroups(), DEFAULT_SHARD_COUNT);
    if (browserPhase === "consume")
      await stage("browser-handoff-consume", handoff("consume"), root);
    else await build();
    for (const index of [shard]) {
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
  if (selected && process.env.FVOCI_SELECTED_IMAGE_WORKLOAD === "1") return 0;
  const pending = await group(specs);
  if (!selected || entryCancellation) return pending || entryCancellation;
  return selectedAfterPending(pending, () =>
    selectedHost(root, pending, selectedPhase === "consume"),
  );
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
  if (selectedImageHost()) {
    if (required("SQLITE3_LIB_DIR") !== "/opt/fvoci-sqlite/x86_64-unknown-linux-gnu/lib")
      fail("selected image SDK path changed");
  } else requirePathWithin(required("SQLITE3_LIB_DIR"), sqlite);
  const diagnostics = join(required("RUNNER_TEMP"), "fvoci-selected-diagnostics");
  createSafeDiagnostics(diagnostics, process.env);
  const uid = String(process.getuid?.()),
    gid = String(process.getgid?.());
  const capture = async (
    args: string[],
    stdout: string,
    stderr?: string,
    cleanup = false,
    directory = diagnostics,
  ) => {
    let out: number | undefined, err: number | undefined;
    try {
      out = openSync(join(directory, stdout), "wx", 0o600);
      if (stderr) err = openSync(join(directory, stderr), "wx", 0o600);
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
  const dockerGid = checkedOutput(["stat", "-c", "%g", "/var/run/docker.sock"], root);
  const cli = join(root, "scripts/run-selected-backend-e2e.ts");
  for (const file of ["permissions.stdout.private.log", "permissions.stderr.private.log"]) {
    try {
      lstatSync(join(output, file));
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ENOENT") continue;
      fail("selected private permissions capture unavailable");
    }
    fail("selected private permissions capture already exists");
  }
  const permissions = await capture(
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
    "permissions.stdout.private.log",
    "permissions.stderr.private.log",
    false,
    output,
  );
  if (entryCancellation) return pending || entryCancellation;
  if (permissions !== 0) {
    try {
      const path = join(output, "permissions.stderr.private.log");
      if (statSync(path).size <= 16384)
        for (const line of selectedPermissionDiagnostics(readFileSync(path, "utf8")))
          process.stderr.write(line + "\n");
    } catch {
      // Evidence capture cannot replace the original permission refusal.
    }
    fail("container browser prerequisite failed");
  }
  const groups = readFileSync(join(output, "permissions.stdout.private.log"), "utf8").trim();
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
    "PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB" +
    (selectedImageHost() ? "," + SELECTED_IMAGE_ENV.join(",") : "");
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

export async function browserServices(
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
  const identities: { name: string; container: string; image: string }[] = [];
  let ledger: { name: string; container: string; inspected: string }[] | undefined;
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
  const witness = () =>
    identities.map(({ name, container, image }, at) => {
      const inspected = checkedOutput(
        [
          "docker",
          "inspect",
          "--format",
          "[{{json .Id}},{{json .Name}},{{json .Config.Image}},{{json .State.StartedAt}},{{json .State.Pid}},{{json .Config.Labels}},{{json .Mounts}}]\n{{json .Config.Image}}|{{json .State.StartedAt}}|{{json .State.Pid}}|{{json .Config.Labels}}|{{json .Mounts}}",
          container,
        ],
        root,
      );
      const split = inspected.indexOf("\n");
      let fields: unknown;
      try {
        fields = JSON.parse(inspected.slice(0, split));
      } catch {
        fail("owned service witness identity invalid (contents withheld)");
      }
      if (
        split < 0 ||
        !Array.isArray(fields) ||
        fields.length !== 7 ||
        fields[0] !== container ||
        fields[1] !== "/" + name ||
        fields[2] !== image ||
        typeof fields[3] !== "string" ||
        !fields[3] ||
        !Number.isSafeInteger(fields[4]) ||
        Number(fields[4]) <= 0 ||
        !fields[5] ||
        typeof fields[5] !== "object" ||
        Array.isArray(fields[5]) ||
        (fields[5] as Record<string, unknown>)["fvoci.test-run"] !== id ||
        !Array.isArray(fields[6]) ||
        !inspected.slice(split + 1).trim()
      )
        fail("owned service witness identity invalid (contents withheld)");
      return {
        name: at === 0 ? "FVOCI_TEST_PG_CONTAINER" : "FVOCI_TEST_MEILI_CONTAINER",
        container: name,
        inspected: inspected.slice(split + 1),
      };
    });
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
      const attempted = { name, container: "", image };
      identities.push(attempted);
      const container = checkedOutput(
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
      if (!/^[0-9a-f]{64}$/.test(container)) fail("owned service container ID invalid");
      attempted.container = container;
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
    ledger = witness();
    status = await body({
      TEST_DATABASE_URL: `postgres://postgres:${password}@fvoci-pg:5432/postgres`,
      FVOCI_TEST_PG_CONTAINER: pg,
      FVOCI_TEST_MEILI_CONTAINER: meili,
      FVOCI_E2E_SERVICE_WITNESS: JSON.stringify(ledger),
      FVOCI_MEILI_KEY: key,
      MEILI_MASTER_KEY: key,
    });
    if (interrupted && status === 0) status = interrupted;
  } finally {
    if (ledger) {
      try {
        if (JSON.stringify(witness()) !== JSON.stringify(ledger))
          fail("owned service witness changed (contents withheld)");
        process.stderr.write("container-browser service_witness_unchanged=PASS\n");
      } catch {
        process.stderr.write("container-browser service witness revalidation failed\n");
        if (status === 0) status = 1;
      }
    }
    for (const attempted of [...identities].reverse()) {
      const { name, image } = attempted;
      let { container } = attempted;
      if (!container) {
        try {
          const fields: unknown = JSON.parse(
            checkedOutput(
              [
                "docker",
                "inspect",
                "--format",
                "[{{json .Id}},{{json .Name}},{{json .Config.Image}},{{json .Config.Labels}}]",
                name,
              ],
              root,
            ),
          );
          if (
            Array.isArray(fields) &&
            fields.length === 4 &&
            typeof fields[0] === "string" &&
            /^[0-9a-f]{64}$/.test(fields[0]) &&
            fields[1] === "/" + name &&
            fields[2] === image &&
            fields[3] &&
            typeof fields[3] === "object" &&
            !Array.isArray(fields[3]) &&
            (fields[3] as Record<string, unknown>)["fvoci.test-run"] === id
          )
            container = fields[0];
        } catch {
          /* Absent or foreign attempted names never authorize retirement. */
        }
        if (!container) {
          process.stderr.write(
            "container-browser attempted service ownership unproven; retirement refused\n",
          );
          if (status === 0) status = 1;
          continue;
        }
      }
      let retired = true;
      const present = Bun.spawnSync(["docker", "inspect", "--format", "{{.Id}}", container], {
        stdout: "ignore",
        stderr: "ignore",
      });
      if (present.exitCode === 0) {
        if (
          Bun.spawnSync(["docker", "stop", "--time", "10", container], {
            stdout: "ignore",
            stderr: "ignore",
          }).exitCode !== 0
        )
          retired = false;
        try {
          if (inspect(container) !== `false ${name === pg ? "0" : "143"} false`) retired = false;
        } catch {
          retired = false;
        }
        if (
          Bun.spawnSync(["docker", "rm", "-v", container], { stdout: "ignore", stderr: "ignore" })
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
      try {
        if (existsSync(path)) rmSync(path);
      } catch {
        process.stderr.write("container-browser owned service environment cleanup failed\n");
        if (status === 0) status = 1;
      }
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
  const nativeChecks = phase === "native-checks";
  if (
    !process.env.FVOCI_WEB_CONTAINER_IMAGE ||
    !["prepare", "inputs", "consume", "native-checks"].includes(phase)
  )
    fail("ordinary container phase required");
  if (
    (phase === "inputs" || nativeChecks) &&
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
    if (nativeChecks) qualifyNativeChecks(root);
    else
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
    if (phase === "prepare" || phase === "inputs" || nativeChecks) {
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
      if (!nativeChecks)
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
      if (!nativeChecks)
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
    if (nativeChecks) {
      qualifyNativeChecks(root);
      await stage(
        "native-clippy",
        [
          "cargo",
          "clippy",
          "--locked",
          "--offline",
          "--all-targets",
          "--features",
          "db-tests,api-schema",
          "--",
          "-D",
          "warnings",
        ],
        root,
      );
      await stage(
        "native-static-api",
        ["cargo", "test", "--locked", "--offline", "--test", "static_api"],
        root,
      );
      qualifyNativeChecks(root);
      return 0;
    }
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
          [process.execPath, "--no-env-file", import.meta.path, "group-wrapper", ...specs],
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

/** Explicit selection keeps its original argv and runtime scope. */
export function groupInvocations(argv: string[], env: NodeJS.ProcessEnv): string[][] {
  const timer = argv.filter(
    (arg) => arg === "v050-task-timer.spec.ts" || arg === "e2e/v050-task-timer.spec.ts",
  );
  if (
    timer.length !== 1 ||
    argv.some((arg) => arg.endsWith(".spec.ts") && !timer.some((spec) => spec === arg)) ||
    argv.some(
      (arg) =>
        ["--grep", "--grep-invert", "--shard", "--list", "--", "-g"].includes(arg) ||
        ["--grep=", "--grep-invert=", "--shard=", "-g"].some((prefix) => arg.startsWith(prefix)),
    ) ||
    env.FVOCI_E2E_PENDING === "1"
  )
    return [[...argv]];
  return [
    [
      "--grep-invert",
      [TIMER_FILTERS.new_control, TIMER_FILTERS.recovery, TIMER_FILTERS.restart].join("|"),
    ],
    ["--grep", TIMER_FILTERS.recovery],
    ["--grep", TIMER_FILTERS.restart],
    ["--grep", TIMER_FILTERS.new_control],
  ].map((selection) => [...argv, ...selection]);
}

export function groupEnvironment(
  run: string,
  argv: string[],
  env: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv {
  const specs = argv.filter((arg) => !arg.startsWith("-"));
  const label = specs.length
    ? specs
        .slice(0, 2)
        .map((arg) => basename(arg.replace(/\.spec\.ts$/, "")))
        .join("+")
    : env.FVOCI_E2E_PENDING === "1"
      ? "collaboration-pending"
      : "default-suite";
  return {
    ...env,
    GROUP_LABEL: label,
    RUN_DIR: run,
    SERVER_LOG: join(run, "server.log"),
    PEPPER: '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}',
    FVOCI_STATIC_DIR: join(run, "static"),
    FVOCI_W5_EVIDENCE_DIR: env.FVOCI_W5_EVIDENCE_DIR || join(run, "playwright-output/w5-evidence"),
    NET_MONITOR_LOG: join(run, "net-monitor.log"),
    NET_MARKS_LOG: join(run, "net-marks.log"),
    NET_MONITOR_PID: "",
  };
}

export function redactGroupLog(text: string): string {
  return text
    .replace(/postgres(?:ql)?:\/\/\S+/g, "postgres://redacted")
    .replace(/libsql:\/\/\S+/g, "libsql://redacted")
    .replace(/https:\/\/\S*\.turso\.io\S*/g, "https://redacted")
    .replace(
      /\b(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=\S+/g,
      "$1=redacted",
    );
}

/** Every retention step is attempted; partial summaries never replace proof. */
export async function retainFailureArtifacts(env: NodeJS.ProcessEnv): Promise<number> {
  const run = env.RUN_DIR ?? fail("RUN_DIR is required"),
    label = env.GROUP_LABEL ?? "default-suite";
  let errors = 0,
    retained: string;
  const report = (text: string) => {
    try {
      process.stderr.write(text);
    } catch {
      /* closed reader */
    }
  };
  const error = (step: string, detail = "") => {
    errors++;
    report(
      `web-e2e group ${label}: failure-artifact retention step ${step} failed${detail ? ": " + detail : ""}\n`,
    );
  };
  const attempt = (step: string, body: () => void): boolean => {
    try {
      body();
      return true;
    } catch {
      error(step);
      return false;
    }
  };
  try {
    retained = mkdtempSync(join(env.TMPDIR || tmpdir(), "fvoci-collab-e2e-fail."));
  } catch {
    error("create-directory");
    return 1;
  }
  attempt("chmod-directory", () => {
    chmodSync(retained, 0o700);
  });
  if (env.GITHUB_OUTPUT)
    attempt("github-output", () => {
      appendFileSync(
        env.GITHUB_OUTPUT as string,
        `failure-artifacts=${retained}\nfailure-group=${label}\n`,
      );
    });
  attempt("copy-playwright-output", () => {
    const from = join(run, "playwright-output");
    if (existsSync(from) && readdirSync(from).length)
      cpSync(from, join(retained, "playwright-output"), {
        recursive: true,
        preserveTimestamps: true,
      });
  });
  if (env.SERVER_LOG && existsSync(env.SERVER_LOG))
    attempt("redact-server-log", () => {
      writeFileSync(
        join(retained, "server.log"),
        redactGroupLog(readFileSync(env.SERVER_LOG as string, "utf8")),
      );
    });
  attempt("create-owned-server-directory", () =>
    mkdirSync(join(retained, "owned-server"), { recursive: true }),
  );
  const logs: string[] = [];
  const walk = (directory: string, visit: (path: string) => void, step: string) => {
    try {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) walk(path, visit, step);
        else if (entry.isFile()) visit(path);
      }
    } catch {
      error(step);
    }
  };
  walk(
    run,
    (path) => {
      if (basename(path) === "server.log" && dirname(path) !== run) logs.push(path);
    },
    "find-owned-server-logs",
  );
  for (const log of logs)
    attempt(`redact-owned-server-log (${log})`, () => {
      writeFileSync(
        join(retained, "owned-server", basename(dirname(log)) + ".log"),
        redactGroupLog(readFileSync(log, "utf8")),
      );
    });
  const net = [env.NET_MONITOR_LOG, env.NET_MARKS_LOG].filter(
    (path): path is string => !!path && existsSync(path),
  );
  if (net.length)
    attempt("merge-net-events", () => {
      const lines = net.flatMap((path) => readFileSync(path, "utf8").split("\n").filter(Boolean));
      lines.sort((a, b) => {
        const x = a.split(/\s/, 1)[0] ?? "",
          y = b.split(/\s/, 1)[0] ?? "";
        return x < y ? -1 : x > y ? 1 : 0;
      });
      writeFileSync(
        join(retained, "net-events.log"),
        redactGroupLog(
          "# group namespace netlink address/link events (ip -o -tshort monitor address link, UTC) and group markers\n" +
            lines.join("\n") +
            "\n",
        ),
      );
    });
  const traces: string[] = [];
  walk(
    retained,
    (path) => {
      if (basename(path) === "trace.zip") traces.push(path);
    },
    "find-traces",
  );
  for (const trace of traces) {
    const dest = join(dirname(trace), "browser-summary.txt");
    try {
      const child = Bun.spawn(
        [
          process.execPath,
          "--no-env-file",
          join(env.ROOT ?? fail("ROOT is required"), "tools/web-e2e/trace-summary.ts"),
          trace,
        ],
        { env, stdin: "ignore", stdout: "pipe", stderr: "pipe" },
      );
      const [stdout, stderr, status] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ]);
      if (status !== 0) {
        attempt("remove-partial-summary", () => {
          rmSync(dest, { force: true });
        });
        error(
          `trace-summary (${basename(dirname(trace))}/trace.zip, exit ${String(status)})`,
          redactGroupLog(stderr).trimEnd().split("\n").slice(-5).join(" "),
        );
      } else if (
        !attempt("write-trace-summary", () => {
          writeFileSync(dest, stdout);
        })
      )
        attempt("remove-partial-summary", () => {
          rmSync(dest, { force: true });
        });
    } catch {
      attempt("remove-partial-summary", () => {
        rmSync(dest, { force: true });
      });
      error(`trace-summary (${basename(dirname(trace))}/trace.zip)`);
    }
  }
  report(
    `retained ${errors ? "incomplete " : ""}failure artifacts for group ${label} in ${retained}${errors ? ` (${String(errors)} retention steps failed)` : ""}\n`,
  );
  return errors ? 1 : 0;
}

/** Fresh namespace per invocation; the native server remains group-runtime's child. */
export async function groupWrapper(argv: string[]): Promise<number> {
  const root = process.env.ROOT || fail("ROOT is required");
  if (!process.env.CARGO_TARGET_DIR) fail("CARGO_TARGET_DIR is required");
  type Child = ReturnType<typeof Bun.spawn>;
  let child: Child | undefined,
    interrupted = 0,
    retiring = false;
  let markCancellation: ((text: string) => void) | undefined;
  const cancel = (signal: NodeJS.Signals, code: number) => {
    if (interrupted) return;
    interrupted = code;
    try {
      markCancellation?.("group cancellation status " + String(code));
    } catch {
      /* diagnostic only */
    }
    // Record cleanup-time cancellation without interrupting necessary retirement.
    if (!retiring && child) {
      try {
        process.kill(-child.pid, signal);
      } catch {
        /* already reaped */
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
      cancel(signal, code);
    };
    process.on(signal, handler);
    return [signal, handler] as const;
  });
  const closed = () => {
    cancel("SIGPIPE", 141);
  };
  process.stderr.on("error", closed);
  try {
    for (const args of groupInvocations(argv, process.env)) {
      if (interrupted) return interrupted;
      child = undefined;
      retiring = false;
      const run = mkdtempSync(join(tmpdir(), "fvoci-web-e2e."));
      const env = groupEnvironment(run, args, process.env),
        label = env.GROUP_LABEL ?? "default-suite";
      let status = 0;
      let monitor: Child | undefined;
      const report = (text: string) => {
        try {
          process.stderr.write(text);
        } catch {
          /* closed reader */
        }
      };
      const mark = (text: string) => {
        const micros = Math.floor((performance.timeOrigin + performance.now()) * 1000);
        const stamp =
          new Date(Math.floor(micros / 1_000_000) * 1000).toISOString().slice(0, 19) +
          "." +
          String(micros % 1_000_000).padStart(6, "0");
        appendFileSync(env.NET_MARKS_LOG as string, `[${stamp}] # fvoci: ${text}\n`);
      };
      markCancellation = mark;
      try {
        if (
          !existsSync(join(root, "apps/web/dist")) ||
          !statSync(join(root, "apps/web/dist")).isDirectory()
        )
          fail("missing apps/web/dist; build web assets before running groups");
        cpSync(join(root, "apps/web/dist"), env.FVOCI_STATIC_DIR as string, {
          recursive: true,
          preserveTimestamps: true,
        });
        report(`=== web e2e group: ${label} ===\n`);
        const shared = env.FVOCI_E2E_SHARED_SERVICES === "1";
        if (shared)
          for (const name of [
            "TEST_DATABASE_URL",
            "FVOCI_TEST_PG_CONTAINER",
            "FVOCI_MEILI_URL",
            "FVOCI_MEILI_KEY",
            "MEILI_MASTER_KEY",
            "FVOCI_TEST_MEILI_CONTAINER",
          ])
            if (!env[name])
              fail(
                `FVOCI_E2E_SHARED_SERVICES=1 requires ${name} from the scope that started the services`,
              );
        writeFileSync(env.NET_MONITOR_LOG as string, "");
        if (Bun.which("ip", { PATH: env.PATH })) {
          const fd = openSync(env.NET_MONITOR_LOG as string, "a");
          try {
            monitor = Bun.spawn(["ip", "-o", "-tshort", "monitor", "address", "link"], {
              env: { ...env, TZ: "UTC" },
              stdin: "ignore",
              stdout: fd,
              stderr: fd,
            });
          } finally {
            closeSync(fd);
          }
          env.NET_MONITOR_PID = String(monitor.pid);
          mark(`netlink monitor started (ip monitor address link, pid ${String(monitor.pid)})`);
        } else {
          mark("ip (iproute2) not found; no netlink event log");
          report(`warning: ip (iproute2) not found; no netlink event log for group ${label}\n`);
        }
        mark(
          shared
            ? "using shared test containers (postgres, meilisearch)"
            : "starting test containers (postgres, meilisearch)",
        );
        if (interrupted) throw new RuntimeExit(interrupted);
        child = Bun.spawn(
          [
            "setsid",
            ...(shared
              ? []
              : [
                  "bash",
                  join(root, "scripts/start-test-postgres.sh"),
                  "bash",
                  join(root, "scripts/start-test-meili.sh"),
                ]),
            process.execPath,
            "--no-env-file",
            import.meta.path,
            "group-runtime",
            ...args,
          ],
          { cwd: root, env, stdin: "ignore", stdout: "inherit", stderr: "inherit" },
        );
        status = await child.exited;
        status ||= interrupted;
      } catch (error) {
        status = error instanceof RuntimeExit ? error.status : 1;
        if (error instanceof CheckError) report(error.message + "\n");
      } finally {
        retiring = true;
        try {
          if (monitor) {
            if (monitor.exitCode === null && monitor.signalCode === null) monitor.kill("SIGTERM");
            await monitor.exited;
          }
        } catch {
          report(`web-e2e group ${label}: could not retire netlink monitor\n`);
          status ||= 1;
        }
        status ||= interrupted;
        try {
          mark(`group exiting with status ${String(status)}`);
        } catch {
          /* diagnostic only */
        }
        if (status !== 0 && (await retainFailureArtifacts(env)) !== 0)
          report(
            `web-e2e group ${label}: failure artifacts are incomplete; the group verdict stays exit ${String(status)}\n`,
          );
        try {
          rmSync(run, { recursive: true, force: true });
        } catch {
          report(`web-e2e group ${label}: could not remove its run directory ${run}\n`);
          status ||= 1;
        }
      }
      // Process queued signal callbacks before opening another fresh namespace.
      await new Promise<void>((resolve) => {
        setImmediate(resolve);
      });
      status ||= interrupted;
      if (status !== 0) return status;
    }
    return interrupted;
  } finally {
    for (const [signal, handler] of handlers) process.removeListener(signal, handler);
    process.stderr.removeListener("error", closed);
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
  if (
    !argv[0] ||
    !["--prepare", "--consume", "--ci-shard", "--ci-shards", "--native-checks"].includes(mode ?? "")
  )
    throw new UsageError(
      "container-browser ROOT {--prepare|--consume N|--ci-shard N|--ci-shards N,N,...|--native-checks}",
    );
  const options = new Map<string, string>();
  for (
    let at = mode === "--prepare" || mode === "--native-checks" ? 2 : 3;
    at < argv.length;
    at++
  ) {
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
  if (mode === "--native-checks" && options.size)
    fail("native checks use only the published image/SDK without packet or reuse overrides");
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
  const phase =
    mode === "--native-checks"
      ? "native-checks"
      : mode === "--prepare"
        ? "prepare"
        : mode === "--consume"
          ? "consume"
          : "all";
  const shards =
    phase === "prepare" || phase === "native-checks"
      ? []
      : (argv[2] ?? "").split(",").map(containerShard);
  if (
    new Set(shards).size !== shards.length ||
    (mode !== "--ci-shards" &&
      phase !== "prepare" &&
      phase !== "native-checks" &&
      shards.length !== 1)
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
        (phase === "native-checks"
          ? "web-native-checks"
          : phase === "prepare"
            ? "workspace-browser-container-build"
            : "workspace-browser-shard") ||
      !/^[a-f0-9]{40}$/.test(env.GITHUB_SHA ?? "") ||
      !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? "") ||
      !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? "") ||
      !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(env.GITHUB_REPOSITORY ?? ""))
  )
    fail("container phase requires its actual GitHub source/run/attempt/repository/job");
  if (
    phase !== "all" &&
    phase !== "native-checks" &&
    (!options.get("--handoff") || options.has("--reuse-cohort"))
  )
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

/** Standalone checks have no browser packet or service authority. */
export function qualifyNativeChecks(
  root: string,
  env: NodeJS.ProcessEnv = process.env,
  git: Git = spawnGit(root),
): void {
  const request = containerRequest([root, "--native-checks"], env);
  if (
    !process.getuid?.() ||
    env.FVOCI_WEB_CONTAINER_JOB !== "web-native-checks" ||
    env.FVOCI_WEB_CONTAINER_IMAGE !== request.sdk.image ||
    env.FVOCI_WEB_CONTAINER_SDK_SHA256 !== request.sdk.sdk ||
    env.FVOCI_WEB_CONTAINER_LOCAL_IMAGE !== "0" ||
    (!request.ci && env.FVOCI_WEB_CONTAINER_LOCAL !== "1")
  )
    fail("native checks require the allocated published-image container");
  const source = env.FVOCI_WEB_CONTAINER_SOURCE ?? "";
  if (
    !/^[a-f0-9]{40}$/.test(source) ||
    (request.ci && source !== env.GITHUB_SHA) ||
    gitLine(git, ["rev-parse", "HEAD"]) !== source ||
    gitLine(git, ["rev-parse", "--show-toplevel"]) !== realpathSync(root) ||
    git(["diff", "--quiet", "HEAD", "--"]).status !== 0
  )
    fail("native checks require the exact clean source");
  if (
    env.CARGO_BUILD_JOBS !== "2" ||
    env.CARGO_INCREMENTAL !== "0" ||
    env.CARGO_PROFILE_DEV_DEBUG !== "0" ||
    env.CARGO_PROFILE_TEST_DEBUG !== "0" ||
    env.CARGO_TARGET_DIR !== join(root, "target")
  )
    fail("native checks require the original compiler profile");
  const disk = statfsSync(root);
  if (disk.bavail * disk.bsize < 20_000_000_000)
    fail("native checks require the existing 20GB disk floor");
}

/** Shared local/CI execution: one pinned image and the existing qualified packet. */
async function containerBrowser(argv: string[]): Promise<number> {
  const { sdk, phase: requestedPhase, shards, ci, options } = containerRequest(argv, process.env);
  const nativeDeadline = performance.now() + 15 * 60_000;
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
  qualifyContainerDaemon(
    dockerEndpoint(root),
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
  if (requestedPhase === "native-checks") {
    const disk = statfsSync(root);
    if (disk.bavail * disk.bsize < 20_000_000_000)
      fail("native checks require the existing 20GB disk floor");
  }
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
  const imageSource = qualifyImageSource(root, metadata, sdk, architecture, orchestratorSource);
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
    phase: "prepare" | "inputs" | "consume" | "native-checks",
    services: NodeJS.ProcessEnv = {},
    shard = "",
  ): Promise<number> {
    if (entryCancellation) return entryCancellation;
    const now = performance.now();
    const producerPhase = phase === "prepare" || phase === "native-checks";
    if (!producerPhase && !consumerDeadlines.has(shard))
      consumerDeadlines.set(shard, now + 20 * 60_000);
    const remaining =
      phase === "native-checks"
        ? nativeDeadline - now
        : producerPhase
          ? 15 * 60_000
          : (consumerDeadlines.get(shard) ?? now) - now;
    if (remaining <= 0) return 1;
    const directory = producerPhase ? producer : join(batch, "consumer-" + shard);
    const runDirectory = producerPhase ? batch : join(batch, "run-" + shard);
    const output = producerPhase ? "producer-output" : "consumer-output-" + shard;
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
        phase === "native-checks"
          ? "web-native-checks"
          : phase === "prepare"
            ? "workspace-browser-build"
            : "workspace-browser-shard",
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
        "FVOCI_E2E_SERVICE_WITNESS",
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
      ...(producerPhase ? [] : [shard]),
    );
    const started = performance.now();
    const logFd = !producerPhase
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
    let result: number, childExit: number;
    try {
      childExit = await child.exited;
      result = childExit;
      result ||= entryCancellation;
    } finally {
      process.removeListener("SIGINT", interrupt);
      process.removeListener("SIGTERM", terminate);
      rmSync(envFile);
    }
    const retired = finishToolImage(name, result);
    result = retired.code;
    const inspect = retired.inspected,
      removeExit = retired.removeExit;
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
    if (result !== 0) {
      const state =
        inspect.exitCode === 0
          ? /^(true|false) ([0-9]{1,3}) (true|false)$/.exec(inspect.stdout.toString().trim())
          : null;
      process.stderr.write(
        `container-browser failure child_exit=${String(childExit)} inspect_exit=${String(inspect.exitCode)} running=${state ? (state[1] === "true" ? "1" : "0") : "-1"} container_exit=${state?.[2] ?? "-1"} oom=${state ? (state[3] === "true" ? "1" : "0") : "-1"} remove_exit=${String(removeExit)}\n`,
      );
      if (logFd !== undefined) {
        try {
          const path = join(
            runDirectory,
            phase === "consume" ? "console.log" : "inputs-console.log",
          );
          for (const line of containerStageDiagnostics(path)) process.stderr.write(line + "\n");
        } catch {
          process.stderr.write("container-browser diagnostics_unavailable=1\n");
        }
      }
    }
    process.stderr.write(
      `container-browser phase=${phase} shard=${shard || "producer"} elapsed_seconds=${((performance.now() - started) / 1000).toFixed(3)} exit=${String(result)}${phase === "consume" ? " log=" + join(runDirectory, "console.log") : ""}\n`,
    );
    return result;
  }
  if (requestedPhase === "native-checks") return runContainer("native-checks");
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
class UsageError extends Error {}

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
    if (argv[0] === "selected-image-phase") {
      arity(argv.slice(1), 2);
      process.exit(await selectedImagePhase(argv[1] ?? "", argv[2] ?? ""));
    }
    if (argv[0] === "selected-image-pending") {
      arity(argv.slice(1), 1);
      process.exit(
        await selectedImageOperation("pending", realpathSync(argv[1] ?? ""), {
          ...process.env,
          FVOCI_E2E_SHARED_SERVICES: "1",
        }),
      );
    }
    if (argv[0] === "group-runtime") process.exit(await groupRuntime(argv.slice(1)));
    if (argv[0] === "group-wrapper") process.exit(await groupWrapper(argv.slice(1)));
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
