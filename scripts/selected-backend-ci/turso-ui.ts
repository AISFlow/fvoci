// Hosted current-primary UI consumer. Credentials stay out of the browser.
// Local preparation is credential-free. Runtime admission belongs to the guard.
import { createHash, randomBytes } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { dlopen, FFIType, ptr } from "bun:ffi";
import {
  chmodSync, closeSync, fchmodSync, lstatSync, mkdirSync, openSync, readFileSync, writeFileSync, writeSync,
  constants, readdirSync, statSync,
} from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";

export class UiError extends Error {
  constructor(code: string) {
    super(code);
    this.name = "UiError";
  }
}

export function requireCondition(condition: unknown, code: string): asserts condition {
  if (!condition) throw new UiError(code);
}

const repoRoot = resolve(import.meta.dir, "../..");
export const ON = "workspace-wiki-selected-backend.spec.ts";
export const OFF = "workspace-off-selected-backend.spec.ts";
export const QUALIFIED_CANONICAL_IMAGE = "sha256:396a5f8e43e8de4b2e1567f2c8a8e841bf45037a4e4ff7cb76dc384951025f35";
export const QUALIFIED_SHELL = "/bin/sh";
export const DAEMON_CAPS: Record<string, string> = {
  "memory.max": "12884901888",
  "cpu.max": "max 100000",
  "pids.max": "128",
  "memory.swap.max": "0",
};
export const SECRET_ENV_KEYS = ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN"] as const;
export const NATIVE_CAPSULE_KEYS = [
  ...SECRET_ENV_KEYS, "PASSWORD_PEPPER_KEYS", "PASSWORD_PEPPER_ACTIVE_KEY_ID", "FVOCI_E2E_TURSO_NAMESPACE",
  "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "FVOCI_TEST_TURSO_DESTRUCTIVE", "E2E_DATABASE_BACKEND",
  "FVOCI_E2E_TURSO_UI_SELECTED", "FVOCI_DATABASE_BACKEND", "FVOCI_REALTIME_MODE", "FVOCI_BIND",
  "FVOCI_PUBLIC_ORIGIN", "FVOCI_COOKIE_SECURE", "STORAGE_DRIVER", "FVOCI_STORAGE_DIR",
  "FVOCI_STATIC_DIR", "FVOCI_COLLAB_ENGINE", "FVOCI_COLLAB_FAMILY_LEASE_MS",
  "FVOCI_COLLAB_FAMILY_RENEW_MS", "FVOCI_COLLAB_MAX_ROOMS", "RUST_LOG",
  "FVOCI_MAINTENANCE_TICK_SECS", "FVOCI_MAINTENANCE_INTERVAL_SECS",
  "FVOCI_UPLOAD_GC_INTERVAL_SECS", "FVOCI_REVISION_SWEEP_INTERVAL_SECS",
];
export const FIXED_LAUNCHER = "#!/bin/sh\nset -eu\nset -a\n. \"$1\"\nset +a\nshift\nnewline='\n'\nread -r fvoci_stat < /proc/$$/stat || exit 78\nexec 3>/fvoci-private/stat.ready\n[ \"${#fvoci_stat}\" -le 511 ] && case $fvoci_stat in *\"$newline\"*) false ;; *) true ;; esac || exit 78\nprintf '%s\\n' \"$fvoci_stat\" >&3\nexec 3>&-\nexec 3</fvoci-private/exec.go\nread -r fvoci_go <&3 || exit 78\nexec 3<&-\n[ \"$fvoci_go\" = GO ] || exit 78\nexec \"$@\"\n";
export const ONE_SHOT_LIVE = "unsupported-before-execution";
export const BLOCKED = "BLOCKED";
export const BACKGROUND_TABLES = [
  "documents", "tasks", "attachments", "attachment_object_cleanups", "revisions", "events",
  "outbox_consumers", "outbox_failures", "processed_events", "notifications", "notification_prefs",
  "ics_tokens", "magic_tokens", "github_deliveries", "github_install_states", "github_installations",
  "github_issue_links", "import_jobs", "import_deferred_events", "push_deliveries", "push_subscriptions",
  "webhook_deliveries", "webhooks",
] as const;
export const AUDITED_COUNTERS = new Set(["event_sequence", "collab_fence_counter", "maintenance_job_claims"]);
export const START_DIAGNOSTIC_INPUT_CAP = 16 * 1024;
export const START_GATE_TEXTS: Record<string, readonly string[]> = {
  none: ["driver error withheld"],
  GATE_AHEAD_INCOMPLETE_UNPREPARED: ["SQLite schema is ahead, incomplete or unprepared"],
  GATE_GAP_FOREIGN_DIGEST: ["SQLite schema has a gap, foreign lineage or changed digest"],
  GATE_CATALOG_DIFFERS: ["SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused"],
  GATE_RETIRED_LINEAGE: ["SQLite schema carries the retired development lineage; install into an empty database"],
  GATE_STEP_GAP: ["SQLite migration gap"],
  GATE_REFERENCE_PIN: ["SQLite schema reference engine pin mismatch"],
  GATE_FAMILY_HANDLE: ["SQLite schema gate requires an actual SQLite-family handle", "SQLite migrations require an actual SQLite-family handle"],
  GATE_BACKEND_KIND: ["remote migrator requires the libsql-remote backend", "remote startup requires the libsql-remote backend"],
  GATE_ENDPOINT_SHAPE: ["remote libSQL requires a TLS primary endpoint and token"],
  GATE_CANCELLED: ["SQLite migration cancelled at a checkpoint"],
  GATE_VALIDATION_FAILED: ["schema validation failed"],
};
const START_REMOTE_CATEGORIES: Record<string, string> = {
  REMOTE_CONNECT_REFUSED: "remote-connect-refused",
  REMOTE_SCHEMA_GATE_REFUSED: "remote-schema-gate-refused",
  REMOTE_MIGRATION_STEP_FAILED: "remote-migration-step-failed",
  REMOTE_MIGRATION_COMMIT_UNKNOWN: "remote-migration-commit-unknown",
  REMOTE_MIGRATION_CANCELLED: "remote-migration-cancelled",
  REMOTE_DRAIN_FAILED: "remote-drain-failed",
};

type Bag = Record<string, any>;
type Env = Record<string, string | undefined>;

export function digest(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value && typeof value === "object") {
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(value as object).sort()) sorted[key] = sortValue((value as Bag)[key]);
    return sorted;
  }
  return value;
}

export function canonical(value: unknown): Uint8Array {
  return Buffer.from(JSON.stringify(sortValue(value)), "utf8");
}

export function valueDigest(value: unknown): string {
  return createHash("sha256").update(canonical(value)).digest("hex");
}

export function writeJson(path: string, value: unknown): void {
  const fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o600);
  try {
    fchmodSync(fd, 0o600);
    writeSync(fd, JSON.stringify(value, null, 2) + "\n");
  } finally {
    closeSync(fd);
  }
}

export function privateRead(path: string, cap = 1024 * 1024): unknown {
  const info = lstatSync(path);
  requireCondition(isAbsolute(path) && info.isFile() && info.nlink === 1 && info.uid === process.getuid()
    && (info.mode & 0o777) === 0o600 && info.size <= cap, "UI_PRIVATE_INPUT_REFUSED");
  return JSON.parse(readFileSync(path, "utf8"));
}

export function uiRoot(env: Env = process.env): string {
  const path = join(env.RUNNER_TEMP ?? "", "turso-ui");
  mkdirSync(path, { recursive: true, mode: 0o700 });
  const info = lstatSync(path);
  requireCondition(!info.isSymbolicLink() && info.uid === process.getuid() && (info.mode & 0o777) === 0o700, "UI_ROOT_REFUSED");
  return path;
}

function git(args: string[]): string {
  const result = Bun.spawnSync(["git", ...args], { cwd: repoRoot, stdout: "pipe", stderr: "ignore" });
  if ((result.exitCode ?? 1) !== 0) throw new Error("git");
  return result.stdout.toString("utf8").trim();
}

export function sourceInputs(): Bag {
  const names = git(["ls-files", "-z"]).split("\0").filter(Boolean);
  requireCondition(git(["status", "--short"]) === "", "UI_DIRTY_SOURCE");
  const files: Record<string, string> = {};
  for (const name of names) files[name] = digest(join(repoRoot, name));
  return { source: git(["rev-parse", "HEAD"]), tree: git(["rev-parse", "HEAD^{tree}"]), files };
}

export function executionMode(env: Env = process.env): string {
  const mode = env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci";
  requireCondition(mode === "github-ci" || mode === "orca-local", "UI_EXECUTION_MODE_REFUSED");
  return mode;
}

export function hostedIdentity(env: Env = process.env): Bag {
  requireCondition(env.GITHUB_ACTIONS === "true" && env.CI === "true" && env.GITHUB_JOB === "turso-ui", "UI_HOSTED_ALLOCATION_REQUIRED");
  for (const name of ["GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"]) {
    requireCondition(/^[0-9]+$/.test(env[name] ?? ""), "UI_HOSTED_ALLOCATION_REQUIRED");
  }
  const source = sourceInputs();
  requireCondition(source.source === env.GITHUB_SHA, "UI_SOURCE_MISMATCH");
  return source;
}

export function sourceIdentity(mode: string, env: Env = process.env): Bag {
  if (executionMode(env) === "github-ci") return hostedIdentity(env);
  requireCondition(executionMode(env) === "orca-local", "UI_EXECUTION_MODE_REFUSED");
  const grant = loadExistingLocalLease(mode, env);
  const base = sourceInputs();
  requireCondition(base.source === grant.source && base.tree === grant.tree, "UI_LOCAL_SOURCE_REFUSED");
  return { executionMode: "orca-local", runId: grant.runId, dispatchId: grant.dispatchId, source: base.source, tree: base.tree, files: base.files };
}

function loadExistingLocalLease(mode: string, env: Env): Bag {
  const script = "import importlib.util,json,os,sys\nfrom pathlib import Path\nroot=Path(sys.argv[1])\nspec=importlib.util.spec_from_file_location('ui_current_binding', root/'scripts/selected-backend-ci/current_binding.py')\nmod=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(mod)\ntry:\n    json.dump(mod.load_local_allocation(sys.argv[2], consumer='turso-ui'), sys.stdout)\nexcept AssertionError:\n    sys.exit(78)\n";
  const result = Bun.spawnSync(["python3", "-c", script, repoRoot, mode], { env: env as Record<string, string>, stdout: "pipe", stderr: "ignore" });
  if ((result.exitCode ?? 1) !== 0) throw new UiError("UI_LOCAL_ALLOCATION_REFUSED");
  return JSON.parse(result.stdout.toString("utf8"));
}

export function cleanEnv(env: Env = process.env): Record<string, string> {
  const keys = ["PATH", "LANG", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ", "RUNNER_TEMP", "PYTHONDONTWRITEBYTECODE", "PLAYWRIGHT_BROWSERS_PATH", "BUN_RUNTIME_TRANSPILER_CACHE_PATH"];
  const cleaned: Record<string, string> = {};
  for (const key of keys) if (env[key] !== undefined) cleaned[key] = env[key]!;
  return cleaned;
}

export function physicalInputs(env: Env = process.env): Bag {
  const script = "import importlib.util,json,sys\nfrom pathlib import Path\nroot=Path(sys.argv[1])\nspec=importlib.util.spec_from_file_location('ui_build_inputs', root/'scripts/run-selected-backend-e2e.py')\nmod=importlib.util.module_from_spec(spec)\nspec.loader.exec_module(mod)\njson.dump({'files': mod.inputs(), 'buildEnvironment': mod.build_env()}, sys.stdout)\n";
  const result = Bun.spawnSync(["python3", "-c", script, repoRoot], { env: env as Record<string, string>, stdout: "pipe", stderr: "ignore", cwd: repoRoot });
  if ((result.exitCode ?? 1) !== 0) throw new Error("physical inputs");
  return JSON.parse(result.stdout.toString("utf8"));
}

export function recheckPhysical(record: unknown, env: Env = process.env): void {
  requireCondition(JSON.stringify(record) === JSON.stringify(physicalInputs(env)), "UI_PHYSICAL_BUILD_INPUTS_CHANGED");
}

function command(args: string[]): string {
  const result = Bun.spawnSync(args, { cwd: repoRoot, stdout: "pipe", stderr: "pipe" });
  if ((result.exitCode ?? 1) !== 0) throw new Error(args[0] + " failed");
  return result.stdout.toString("utf8").trim();
}

export function freeze(env: Env = process.env): void {
  const directory = uiRoot(env);
  const before = privateRead(join(directory, "source-before.json"), 4 * 1024 * 1024);
  requireCondition(JSON.stringify(before) === JSON.stringify(sourceIdentity("freeze", env)), "UI_BUILD_INPUTS_CHANGED");
  const physical = privateRead(join(directory, "physical-before.private.json"), 64 * 1024 * 1024);
  recheckPhysical(physical, env);
  const artifacts: Bag[] = [];
  for (const name of ["ui-compile.json", "ui-engine-compile.json"]) {
    for (const line of readFileSync(join(env.RUNNER_TEMP ?? "", name), "utf8").split("\n")) {
      if (!line.startsWith("{")) continue;
      const record = JSON.parse(line);
      if (record.reason === "compiler-artifact" && record.executable) artifacts.push(record);
    }
  }
  const binaries: Record<string, Bag> = {};
  for (const name of ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"]) {
    const matches = artifacts.filter((artifact) => artifact.target?.name === name && !artifact.profile?.test);
    requireCondition(matches.length === 1, "UI_CURRENT_ARTIFACT_MISSING");
    const artifact = matches[0]!;
    const expected = name === "collab-engine" ? ["default", "worker"] : ["api-schema", "db-tests"];
    requireCondition(JSON.stringify([...(artifact.features as string[])].sort()) === JSON.stringify(expected)
      && artifact.profile?.opt_level === "0" && artifact.profile?.debug_assertions, "UI_ARTIFACT_FEATURES_MISMATCH");
    const executable = String(artifact.executable);
    const info = lstatSync(executable);
    requireCondition(isAbsolute(executable) && !info.isSymbolicLink() && readFileSync(executable).subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])), "UI_ARTIFACT_REFUSED");
    binaries[name] = { path: executable, sha256: digest(executable), features: artifact.features, profile: artifact.profile };
  }
  const dist = join(repoRoot, "apps/web/dist");
  const assets: Record<string, string> = {};
  const walk = (directoryPath: string) => {
    for (const entry of readdirSync(directoryPath, { withFileTypes: true })) {
      const path = join(directoryPath, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) assets[relative(dist, path)] = digest(path);
    }
  };
  walk(dist);
  requireCondition(Object.keys(assets).length > 0 && "index.html" in assets, "UI_FRESH_DIST_MISSING");
  const abi: Record<string, string> = {};
  for (const artifact of Object.values(binaries)) {
    const output = command(["ldd", artifact.path]);
    requireCondition(!output.includes("not found"), "UI_RUNTIME_ABI_MISSING");
    for (const match of output.matchAll(/(\/[\w./+-]+)/g)) {
      const library = resolve(match[1]!);
      requireCondition(statSync(library).isFile(), "UI_RUNTIME_ABI_MISSING");
      abi[library] = digest(library);
    }
  }
  const bun = command(["which", "bun"]);
  const chromium = command([bun, "--eval", 'import {chromium} from "@playwright/test";console.log(chromium.executablePath())']);
  requireCondition(statSync(chromium).isFile(), "UI_PINNED_BROWSER_MISSING");
  const browserFiles: Record<string, string> = {};
  const walkBrowser = (directoryPath: string) => {
    for (const entry of readdirSync(directoryPath, { withFileTypes: true })) {
      const path = join(directoryPath, entry.name);
      if (entry.isDirectory()) walkBrowser(path);
      else if (entry.isFile()) browserFiles[path] = digest(path);
    }
  };
  walkBrowser(resolve(chromium, ".."));
  const manifest = {
    schema: 1,
    sourceInputs: before,
    physicalInputs: { path: join(directory, "physical-before.private.json"), sha256: digest(join(directory, "physical-before.private.json")) },
    binaries,
    assets,
    abi,
    bun: { path: bun, sha256: digest(bun), version: command([bun, "--version"]) },
    chromium,
    browserFiles,
    sqliteInputs: {
      path: join(env.RUNNER_TEMP ?? "", "fvoci-sqlite/consumer-inputs.json"),
      sha256: digest(join(env.RUNNER_TEMP ?? "", "fvoci-sqlite/consumer-inputs.json")),
    },
    rustc: command(["rustc", "-vV"]),
  };
  requireCondition(manifest.bun.version === "1.4.2", "UI_BUN_PIN_MISMATCH");
  writeJson(join(directory, "current-build.json"), manifest);
}

export function currentBuild(env: Env = process.env): Bag {
  const manifest = privateRead(join(uiRoot(env), "current-build.json"), 8 * 1024 * 1024) as Bag;
  requireCondition(JSON.stringify(manifest.sourceInputs) === JSON.stringify(sourceIdentity("current-build", env)), "UI_SOURCE_CHANGED");
  const physical = manifest.physicalInputs;
  requireCondition(digest(physical.path) === physical.sha256, "UI_PHYSICAL_RECEIPT_CHANGED");
  recheckPhysical(privateRead(physical.path, 64 * 1024 * 1024), env);
  for (const record of Object.values(manifest.binaries as Record<string, Bag>)) requireCondition(digest(record.path) === record.sha256, "UI_BINARY_CHANGED");
  for (const [path, sha] of Object.entries(manifest.abi as Record<string, string>)) requireCondition(digest(path) === sha, "UI_ABI_CHANGED");
  for (const [path, sha] of Object.entries(manifest.browserFiles as Record<string, string>)) requireCondition(digest(path) === sha, "UI_BROWSER_CHANGED");
  requireCondition(digest(manifest.bun.path) === manifest.bun.sha256, "UI_BUN_CHANGED");
  requireCondition(digest(manifest.sqliteInputs.path) === manifest.sqliteInputs.sha256, "UI_SQLITE_INPUTS_CHANGED");
  const dist = join(repoRoot, "apps/web/dist");
  const assets: Record<string, string> = {};
  const walk = (directoryPath: string) => {
    for (const entry of readdirSync(directoryPath, { withFileTypes: true })) {
      const path = join(directoryPath, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) assets[relative(dist, path)] = digest(path);
    }
  };
  walk(dist);
  requireCondition(JSON.stringify(assets) === JSON.stringify(manifest.assets), "UI_ASSETS_CHANGED");
  return manifest;
}

export function allocationCaps(grant?: Bag, env: Env = process.env): Record<string, string> {
  if (executionMode(env) === "github-ci") return { ...DAEMON_CAPS };
  requireCondition(grant && typeof grant === "object", "UI_LOCAL_ALLOCATION_REFUSED");
  const runtime = grant.canonicalRuntime;
  requireCondition(runtime && typeof runtime === "object" && runtime.networkAuthorized === true, "UI_LOCAL_ALLOCATION_REFUSED");
  return { ...DAEMON_CAPS, "memory.max": "max", "memory.swap.max": "max" };
}

export function readShellProof(runtime: Bag): string {
  const proof = runtime && typeof runtime === "object" ? runtime.shellProof : undefined;
  requireCondition(proof && typeof proof === "object", "UI_CANONICAL_SHELL_UNAVAILABLE");
  const rawPath = proof.path;
  const rawHash = proof.sha256;
  requireCondition(typeof rawPath === "string" && rawPath.startsWith("/") && !rawPath.startsWith("//"), "UI_CANONICAL_SHELL_UNAVAILABLE");
  const info = lstatSync(rawPath);
  requireCondition(isAbsolute(rawPath) && !info.isSymbolicLink(), "UI_CANONICAL_SHELL_UNAVAILABLE");
  requireCondition(/^[0-9a-f]{64}$/.test(rawHash ?? ""), "UI_CANONICAL_SHELL_UNAVAILABLE");
  requireCondition(digest(rawPath) === rawHash, "UI_CANONICAL_SHELL_UNAVAILABLE");
  const receipt = JSON.parse(readFileSync(rawPath, "utf8"));
  const config = receipt.Config;
  requireCondition(config?.Image === QUALIFIED_CANONICAL_IMAGE && QUALIFIED_CANONICAL_IMAGE === runtime.imageId, "UI_CANONICAL_SHELL_UNAVAILABLE");
  requireCondition(Array.isArray(config?.Cmd) && config.Cmd.includes(QUALIFIED_SHELL), "UI_CANONICAL_SHELL_UNAVAILABLE");
  requireCondition(!(config?.Env ?? []).some((item: unknown) => String(item).startsWith("FVOCI_LIBSQL_")), "UI_DOCKER_ENV_SECRET_REFUSED");
  return QUALIFIED_SHELL;
}

function shellQuote(value: string): string {
  return "'" + value.replaceAll("'", "'\"'\"'") + "'";
}

export function capsuleText(environment: Bag): string {
  requireCondition(!("LOCPATH" in environment), "UI_LOCPATH_REFUSED");
  const leaked = Object.keys(environment).filter((key) => !NATIVE_CAPSULE_KEYS.includes(key) && (
    key.startsWith("FVOCI_LIBSQL_") || key.startsWith("PASSWORD_") || key.includes("TOKEN") || key.includes("SECRET") || key === "LOCPATH"));
  requireCondition(leaked.length === 0, "UI_DOCKER_ENV_SECRET_REFUSED");
  const lines: string[] = [];
  for (const key of NATIVE_CAPSULE_KEYS) {
    if (!(key in environment)) continue;
    const value = environment[key];
    requireCondition(typeof value === "string" && !value.includes("\0") && !value.includes("\n"), "UI_DOCKER_ENV_SECRET_REFUSED");
    lines.push(key + "=" + shellQuote(value) + "\n");
  }
  requireCondition(SECRET_ENV_KEYS.every((key) => lines.some((line) => line.startsWith(key + "="))), "UI_DOCKER_ENV_SECRET_REFUSED");
  return lines.join("");
}

export function containerArgv(name: string, launcher: string, capsule: string, binary: string, mountArgs: string[], command: string[], grant?: Bag, env: Env = process.env): string[] {
  requireCondition(String(launcher).startsWith("/") && String(capsule).startsWith("/"), "UI_CURRENT_ARTIFACT_MISSING");
  const caps = allocationCaps(grant, env);
  const memory = caps["memory.max"] === "max" ? [] : ["--memory", caps["memory.max"]!, "--memory-swap", caps["memory.max"]!];
  const argv = ["docker", "create", "--name", name, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "--user", "1000:1000", ...memory, "--pids-limit", caps["pids.max"]!,
    "--tmpfs", "/tmp:rw,noexec,nosuid,size=67108864,uid=1000,gid=1000", "--network", "host", "--entrypoint", QUALIFIED_SHELL,
    ...mountArgs, QUALIFIED_CANONICAL_IMAGE, "/fvoci-current/launcher.sh", "/fvoci-private/native-env.sh", binary, ...command];
  requireCondition(!argv.includes("--env-file") && !argv.includes("-e"), "UI_DOCKER_ENV_SECRET_REFUSED");
  requireCondition(!["--cpus", "--cpu-quota", "--cpu-period", "--cpuset-cpus"].some((flag) => argv.includes(flag)), "UI_DAEMON_CAP_REFUSED");
  return argv;
}

export function admitPublishedConfig(argv: string[], envItems: string[], secretValues: string[]): void {
  const keys = envItems.map((item) => {
    const separator = item.indexOf("=");
    requireCondition(separator >= 0, "UI_DOCKER_ENV_SECRET_REFUSED");
    return item.slice(0, separator);
  });
  requireCondition(!SECRET_ENV_KEYS.some((key) => keys.includes(key)), "UI_DOCKER_ENV_SECRET_REFUSED");
  const published = [...argv, ...envItems.map(String)];
  for (const value of secretValues) requireCondition(value && !published.some((item) => item.includes(value)), "UI_DOCKER_ENV_SECRET_REFUSED");
  requireCondition(!argv.includes("--env-file") && !argv.includes("-e"), "UI_DOCKER_ENV_SECRET_REFUSED");
}

export function creationIdentity(inspect: Bag, argv: string[], secretValues: string[], kind: string): Bag {
  const config = inspect.Config;
  const host = inspect.HostConfig;
  requireCondition(inspect.State?.Pid === 0 && inspect.State?.Running === false, "UI_DAEMON_PID_REFUSED");
  requireCondition(config?.Image === QUALIFIED_CANONICAL_IMAGE, "UI_CANONICAL_SHELL_UNAVAILABLE");
  requireCondition(JSON.stringify(config?.Entrypoint ?? []) === JSON.stringify([QUALIFIED_SHELL]), "UI_CANONICAL_SHELL_UNAVAILABLE");
  admitPublishedConfig(config?.Cmd ?? [], config?.Env ?? [], secretValues);
  admitPublishedConfig(argv, [], secretValues);
  requireCondition(host?.ReadonlyRootfs === true && JSON.stringify(host?.CapDrop ?? []) === JSON.stringify(["ALL"]), "UI_DAEMON_CAP_REFUSED");
  requireCondition(config?.User === "1000:1000" && host?.Privileged !== true, "UI_DAEMON_CAP_REFUSED");
  if ("CpuQuota" in host) requireCondition(typeof host.CpuQuota === "number" && host.CpuQuota === 0, "UI_DAEMON_CAP_REFUSED");
  if ("NanoCpus" in host) requireCondition(typeof host.NanoCpus === "number" && host.NanoCpus === 0, "UI_DAEMON_CAP_REFUSED");
  if ("CpusetCpus" in host) requireCondition(host.CpusetCpus === "", "UI_DAEMON_CAP_REFUSED");
  return { phase: "created", liveDaemon: kind === "fixture" ? ONE_SHOT_LIVE : "pending-listen", qualification: BLOCKED,
    cgroupCaps: "not-observed", shell: QUALIFIED_SHELL, image: QUALIFIED_CANONICAL_IMAGE };
}

export function runningDaemonSample(inspect: Bag, caps: Bag, procRow: Bag, nspid: number, clientPid: number, binary: string, waited: boolean, grant?: Bag, env: Env = process.env): Bag {
  requireCondition(waited === true, "UI_DAEMON_WAIT_MISSING");
  const pid = inspect.State?.Pid;
  requireCondition(typeof pid === "number" && pid > 0 && pid === procRow.pid && pid !== clientPid, "UI_DAEMON_PID_REFUSED");
  requireCondition(typeof nspid === "number" && nspid > 0 && nspid !== pid, "UI_NAMESPACE_PID_REFUSED");
  requireCondition(inspect.State?.Running === true && inspect.State?.OOMKilled === false, "UI_DAEMON_STATE_REFUSED");
  requireCondition(!("HostConfig" in caps) && !("CapDrop" in caps), "UI_DAEMON_CAP_REFUSED");
  const expected = allocationCaps(grant, env);
  for (const [key, required] of Object.entries(expected)) requireCondition(caps[key] === required, "UI_DAEMON_CAP_REFUSED");
  requireCondition(procRow.comm === "fvoci-server", "UI_NORMAL_MAIN_IDENTITY_FAILED");
  requireCondition(/^[0-9]+$/.test(String(procRow.startTicks ?? "")), "UI_DAEMON_PID_REFUSED");
  if (procRow.exeInspection === "observed") requireCondition(procRow.exe === binary, "UI_CANONICAL_IMAGE_BINARY_REFUSED");
  else requireCondition(procRow.exeInspection === "UNAVAILABLE", "UI_NORMAL_MAIN_IDENTITY_FAILED");
  const selected: Record<string, string> = {};
  for (const key of Object.keys(expected)) selected[key] = caps[key];
  return { daemonPid: pid, pid: nspid, startTicks: procRow.startTicks, waited: true, qualification: "daemon-observed", caps: selected };
}

const PHASES = new Set(["backend-contract", "schema-check", "schema-contract", "begin-read", "family-contract", "table-read", "table-conversion", "table-contract", "preservation-read", "row-limit", "row-hash", "row-allocation", "ui-read", "ui-conversion", "summary-read", "ledger-read", "ledger-conversion", "summary-conversion", "rollback"]);
const CATEGORIES = new Set(["request", "database", "row-conversion", "protocol", "pool", "driver", "other", "libsql-hrana"]);

export function baselineFailureDiagnostic(value: Bag): Bag {
  const outcome = value.nativeOutcome && typeof value.nativeOutcome === "object" ? value.nativeOutcome : {};
  let status = "missing";
  const cause = (name: string, rollback = false) => {
    const item = outcome[name];
    if (item == null) return null;
    const keys = new Set(Object.keys(item));
    const allowed = keys.size === 2 ? keys.has("phase") && keys.has("category") : keys.size === 3 && keys.has("phase") && keys.has("category") && keys.has("tableComparison");
    if (!item || typeof item !== "object" || !allowed || typeof item.phase !== "string" || !PHASES.has(item.phase)
      || (rollback && item.phase !== "rollback") || typeof item.category !== "string" || !CATEGORIES.has(item.category)) {
      status = "refused";
      return null;
    }
    const result: Bag = { phase: item.phase, category: item.category };
    if ("tableComparison" in item) {
      const comparison = item.tableComparison;
      const comparisonKeys = ["expectedCount", "actualCount", "setEqual", "orderEqual", "actualOnlyCount", "expectedOnlyCount", "actualOnlyUnderscoreCount", "firstMismatchIndex", "actualMismatchExpectedIndex"];
      const integerKey = (key: string) => Number.isInteger(comparison?.[key]) && comparison[key] >= 0 && comparison[key] <= 100001;
      const indexOk = (key: string, limit: number) => comparison[key] == null || (Number.isInteger(comparison[key]) && comparison[key] >= 0 && comparison[key] < limit);
      if (rollback || item.phase !== "table-contract" || item.category !== "protocol" || !comparison || typeof comparison !== "object"
        || JSON.stringify(Object.keys(comparison).sort()) !== JSON.stringify([...comparisonKeys].sort())
        || ["expectedCount", "actualCount", "actualOnlyCount", "expectedOnlyCount", "actualOnlyUnderscoreCount"].some((key) => !integerKey(key))
        || typeof comparison.setEqual !== "boolean" || comparison.orderEqual !== false
        || comparison.actualOnlyCount > comparison.actualCount || comparison.expectedOnlyCount > comparison.expectedCount
        || comparison.actualOnlyUnderscoreCount > comparison.actualOnlyCount
        || comparison.setEqual !== (comparison.actualOnlyCount === 0 && comparison.expectedOnlyCount === 0)
        || (comparison.firstMismatchIndex != null && (!Number.isInteger(comparison.firstMismatchIndex) || comparison.firstMismatchIndex < 0
          || comparison.firstMismatchIndex > Math.min(comparison.expectedCount, comparison.actualCount, 100000)
          || (comparison.expectedCount === comparison.actualCount && comparison.actualCount < 100001 && comparison.firstMismatchIndex >= comparison.actualCount)))
        || !indexOk("actualMismatchExpectedIndex", Math.min(comparison.expectedCount, 100001))
        || (comparison.actualMismatchExpectedIndex != null && (comparison.firstMismatchIndex == null || comparison.firstMismatchIndex >= comparison.actualCount))) {
        status = "refused";
        return null;
      }
      result.tableComparison = { ...comparison };
    }
    if (status !== "refused") status = "qualified";
    return result;
  };
  const first = cause("baselineFailure");
  const rollback = cause("baselineRollbackFailure", true);
  const state = (container: Bag, name: string, allowed: string[]) => {
    const item = container[name];
    if (typeof item !== "string" || !allowed.includes(item)) {
      status = "refused";
      return null;
    }
    return item;
  };
  return {
    originalFailure: "TURSO_UI_BASELINE_FAILED",
    diagnosticStatus: status,
    baselineFailure: first,
    baselineRollbackFailure: rollback,
    nativeOutcome: {
      operation: state(outcome, "operation", ["failed", "confirmed"]),
      rollback: state(outcome, "rollback", ["not-attempted", "unknown", "confirmed"]),
      commit: state(outcome, "commit", ["not-attempted", "unknown", "confirmed"]),
    },
    lifecycleDrain: state(value, "lifecycleDrain", ["confirmed", "unconfirmed"]),
    drainOutcome: state(value, "drainOutcome", ["confirmed", "failed"]),
  };
}

export function startupBlockers(baseline: Bag): string[] {
  const blocked = BACKGROUND_TABLES.filter((table) => baseline.fingerprints?.[table] && Object.keys(baseline.fingerprints[table]).length > 0);
  if (baseline.startupHazards !== 0 || baseline.liveOutboxLeases !== 0) blocked.push("existing-owner-or-deletion");
  return blocked;
}

function integerCell(cell: unknown): number {
  requireCondition(Array.isArray(cell) && cell.length === 2 && cell[0] === "integer" && typeof cell[1] === "string" && /^(?:0|[1-9][0-9]*)$/.test(cell[1]), "UI_COUNTER_TYPE_REFUSED");
  const value = Number(cell[1]);
  requireCondition(value <= Number.MAX_SAFE_INTEGER || BigInt(cell[1]) <= 9223372036854775807n, "UI_COUNTER_RANGE_REFUSED");
  return Number(cell[1]);
}

export function assertPreserved(before: Bag, after: Bag, audit?: Bag): void {
  requireCondition(JSON.stringify(before.ledger) === JSON.stringify(after.ledger) && before.schemaSha256 === after.schemaSha256 && before.lineage === after.lineage, "UI_CURRENT_LEDGER_CHANGED");
  requireCondition(JSON.stringify(Object.keys(before.fingerprints).sort()) === JSON.stringify(Object.keys(after.fingerprints).sort()), "UI_CURRENT_TABLES_CHANGED");
  for (const [table, rows] of Object.entries(before.fingerprints as Record<string, Record<string, number>>)) {
    if (audit && AUDITED_COUNTERS.has(table)) continue;
    for (const [sha, count] of Object.entries(rows)) requireCondition(after.fingerprints[table]?.[sha] === count, "UI_PREEXISTING_ROW_CHANGED");
  }
  if (audit) {
    requireCondition(before.startupHazards === 0 && after.startupHazards === 0 && after.liveOutboxLeases === 0, "UI_FOREIGN_OR_UNRELEASED_OWNER");
  }
}

export function startupRemoteCause(line: string): [string, string] | null {
  const wrappers: Array<[string, string]> = [
    ["fvoci: preparation failed; the server does not start: remote preparation refused ", "prepare-remote"],
    ['Error: "remote normal startup refused ', "server-remote"],
  ];
  for (const [prefix, phase] of wrappers) {
    if (!line.startsWith(prefix)) continue;
    let tail = line.slice(prefix.length);
    if (phase === "server-remote") {
      if (!tail.endsWith('"')) return null;
      tail = tail.slice(0, -1);
    }
    const match = /^\(([A-Z_]+), gate ([A-Z_]+|none), settlement ([a-z-]+)\): (.+)$/.exec(tail);
    if (!match) return null;
    const code = match[1]!;
    const gate = match[2]!;
    const settlement = match[3]!;
    const display = match[4]!;
    if (!(code in START_REMOTE_CATEGORIES) || !(gate in START_GATE_TEXTS)) return null;
    const number = /^remote migration (?:step (-?[0-9]+)|cancelled after ([0-9]+)) /.exec(display);
    if (number) {
      const value = number[1] || number[2]!;
      const parsed = Number(value);
      if (String(parsed) !== value || (number[1] ? !(parsed >= -(2 ** 31) && parsed < 2 ** 31) : !(parsed >= 0 && parsed < 2 ** 64))) return null;
    }
    if (code === "REMOTE_CONNECT_REFUSED" && !["none", "GATE_BACKEND_KIND", "GATE_ENDPOINT_SHAPE"].includes(gate)) return null;
    if (code === "REMOTE_MIGRATION_CANCELLED" && settlement === "cancel-checkpoint-settled" && gate !== "none") return null;
    const drain = "; remote stream drain failed at close";
    const patterns: RegExp[] = [];
    const texts = START_GATE_TEXTS[gate] ?? [];
    if (code === "REMOTE_CONNECT_REFUSED" && settlement === "no-write-opened") patterns.push(/^remote libSQL primary connect refused \(TLS endpoint and token required\)$/);
    else if (code === "REMOTE_SCHEMA_GATE_REFUSED") {
      const stages: Record<string, string[]> = {
        "no-write-opened": ["remote schema gate refused before any write", "remote startup gate refused"],
        "writes-may-have-committed": ["remote schema gate refused after migration steps committed"],
        "drain-failed": ["remote schema gate refused before any write", "remote startup gate refused", "remote schema gate refused after migration steps committed"],
      };
      for (const stage of stages[settlement] ?? []) for (const text of texts) patterns.push(new RegExp("^" + escapeRegExp(stage + ": " + text + (settlement === "drain-failed" ? drain : "")) + "$"));
    } else if (code === "REMOTE_MIGRATION_STEP_FAILED") {
      const details: Record<string, string[]> = {
        "rollback-confirmation-withheld": ["rollback confirmation withheld"],
        "cleanup-unconfirmed": ["cleanup unconfirmed, admission quarantined"],
        "drain-failed": ["rollback confirmation withheld", "cleanup unconfirmed, admission quarantined"],
      };
      for (const detail of details[settlement] ?? []) for (const text of texts) {
        patterns.push(new RegExp("^remote migration step -?[0-9]{1,10} failed; " + escapeRegExp(detail + ": " + text + (settlement === "drain-failed" ? drain : "")) + "$"));
      }
    } else if (code === "REMOTE_MIGRATION_COMMIT_UNKNOWN" && (settlement === "commit-unknown" || settlement === "drain-failed")) {
      patterns.push(new RegExp("^remote migration step -?[0-9]{1,10}" + escapeRegExp(" commit outcome is unknown; settlement receipt retained; rerun resumes from the ledger" + (settlement === "drain-failed" ? drain : "")) + "$"));
    } else if (code === "REMOTE_MIGRATION_CANCELLED" && (settlement === "cancel-checkpoint-settled" || settlement === "drain-failed")) {
      patterns.push(new RegExp("^remote migration cancelled after [0-9]{1,20}" + escapeRegExp(" settled step(s)" + (settlement === "drain-failed" ? drain : "")) + "$"));
    } else if (code === "REMOTE_DRAIN_FAILED" && settlement === "drain-failed") patterns.push(/^remote stream drain failed at close$/);
    if (patterns.some((pattern) => pattern.test(display))) return [phase, START_REMOTE_CATEGORIES[code]!];
  }
  return null;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function serverStartDiagnostic(server: { poll: () => number | null } | null, logText: Uint8Array, started: number | null, deadline: number | null, now = performance.now() / 1000): Bag {
  const record: Bag = { originalFailure: "UI_SERVER_START_FAILED", diagnosticStatus: "unqualified", processState: "unclassified", exitCode: null, elapsedMs: null, phase: null, category: null };
  try {
    const code = server ? server.poll() : null;
    if (typeof code === "number" && code >= -255 && code <= 255) {
      record.processState = "exited";
      record.exitCode = code;
    } else if (server && code === null && deadline !== null && now >= deadline) record.processState = "deadline";
    if (started !== null && Number.isFinite(now - started)) record.elapsedMs = Math.max(0, Math.min(10_000, Math.trunc((now - started) * 1000)));
    if (logText.length > START_DIAGNOSTIC_INPUT_CAP || logText.length === 0 || logText[logText.length - 1] !== 0x0a) return record;
    const lines = new TextDecoder("ascii").decode(logText).split("\n").slice(0, -1);
    const missing = new Set(["fvoci: ENCRYPTION_KEYS is not set (see the env example)", "fvoci: ENCRYPTION_ACTIVE_KEY_ID is not set (see the env example)"]);
    const causes: Array<[string, string]> = [];
    for (let line of lines) {
      if (line.endsWith("\r")) line = line.slice(0, -1);
      if (missing.has(line)) causes.push(["prepare-config", "missing-encryption-keyring"]);
      else if (line !== "fvoci: not starting; fix .env and run docker compose up -d again" && line !== "fvoci: prepared; starting the server") {
        const cause = startupRemoteCause(line);
        if (!cause) return record;
        causes.push(cause);
      }
    }
    if (causes.length && new Set(causes.map((item) => item.join("\0"))).size === 1) {
      record.diagnosticStatus = "qualified";
      record.phase = causes[0]![0];
      record.category = causes[0]![1];
    }
  } catch {
    return record;
  }
  return record;
}

export function failureCode(error: unknown): string {
  const value = error instanceof UiError ? error.message : "";
  return /^(?:UI|TURSO_UI)_[A-Z_]+$/.test(value) ? value : "UI_CONSUMER_FAILED";
}

export function baselinePassLine(source: string, baselineSha: string, targetSha: string, rows: number, setupNeeded: boolean, admissible: boolean): string {
  return "TURSO_UI_BASELINE_PASS source=" + source + " baseline_sha256=" + baselineSha + " target_sha256=" + targetSha
    + " rows=" + rows + " setup_needed=" + String(setupNeeded) + " startup_admissible=" + String(admissible);
}

type FixtureFn = (manifest: Bag, mode: string, environment: Bag, input?: unknown) => Promise<Bag> | Bag;

let processes: UiProcesses | null = null;

class UiProcesses {
  pid = process.pid;
  entries = new Map<string, Bag>();
  allocations: Bag[] = [];
  errors: string[] = [];
  prior = 0;
  stopped = false;
  timer: Timer | null = null;
  libc = dlopen("libc.so.6", {
    prctl: { args: [FFIType.i32, FFIType.u64, FFIType.u64, FFIType.u64, FFIType.u64] as FFIType[], returns: FFIType.i32 },
    pidfd_open: { args: [FFIType.i32, FFIType.u32] as FFIType[], returns: FFIType.i32 },
  });

  static enter(): UiProcesses {
    requireCondition(processes === null, "UI_PROCESS_CAPABILITY_REQUIRED");
    const scope = new UiProcesses();
    const prior = new Int32Array(1);
    scope.prctl(37, BigInt(ptr(prior)));
    scope.prior = prior[0]!;
    try {
      scope.prctl(36, 1n);
      const check = new Int32Array(1);
      scope.prctl(37, BigInt(ptr(check)));
      requireCondition(check[0] === 1, "UI_SUBREAPER_NOT_CONFIRMED");
      processes = scope;
      scope.timer = setInterval(() => {
        try { scope.snapshot(); } catch { scope.errors.push("UI_PROCESS_OBSERVATION_FAILED"); }
      }, 20);
      return scope;
    } catch (error) {
      try {
        scope.prctl(36, BigInt(scope.prior));
      } catch {
        process.stderr.write(JSON.stringify({ originalFailure: failureCode(error), processCleanupErrors: ["UI_SUBREAPER_RESTORE_FAILED"] }) + "\n");
      }
      throw error;
    }
  }

  prctl(option: number, value: bigint) {
    if (this.libc.symbols.prctl(option, value, 0n, 0n, 0n) !== 0) throw new UiError("UI_SUBREAPER_SETUP_FAILED");
  }

  procRows(): Bag[] {
    const names = readdirSync("/proc").filter((name) => /^[0-9]+$/.test(name));
    requireCondition(names.length <= 4096, "UI_PROCESS_SNAPSHOT_CAP_REFUSED");
    const rows: Bag[] = [];
    for (const name of names) {
      try { rows.push(procIdentity(Number(name))); } catch { /* raced */ }
    }
    return rows;
  }

  snapshot() {
    if (!this.allocations.some((allocation) => !allocation.closed)) return;
  }

  spawn(args: string[], label: string, options: { env: Record<string, string>; cwd?: string; input?: Uint8Array; stdout?: number }): ChildProcess {
    const stdio = options.stdout === undefined ? ["pipe", "pipe", "pipe"] as const : ["ignore", options.stdout, options.stdout] as const;
    const child = spawn(args[0]!, args.slice(1), { env: options.env, cwd: options.cwd, detached: true, stdio });
    const allocation = { process: child, label, closed: false, forced: false, key: "" };
    this.allocations.push(allocation);
    allocation.key = child.pid + ":pending";
    if (options.input && child.stdin) child.stdin.end(options.input);
    else child.stdin?.end();
    return child;
  }

  async finish(child: ChildProcess): Promise<number> {
    const code = await new Promise<number>((resolveExit, rejectExit) => {
      if (child.exitCode !== null) resolveExit(child.exitCode);
      else child.once("exit", (status) => resolveExit(status ?? 1));
      child.once("error", rejectExit);
    });
    const allocation = this.allocations.find((item) => item.process === child);
    if (allocation) allocation.closed = true;
    return code;
  }

  async exit(original: unknown) {
    if (this.timer) clearInterval(this.timer);
    try {
      this.prctl(36, BigInt(this.prior));
    } catch {
      this.errors.push("UI_SUBREAPER_RESTORE_FAILED");
    }
    processes = null;
    if (original == null && this.errors.length) throw new UiError("UI_PROCESS_CLOSURE_FAILED");
  }
}

function procIdentity(pid: number): Bag {
  const stat = readFileSync("/proc/" + pid + "/stat", "utf8");
  const rest = stat.slice(stat.lastIndexOf(")") + 1).trim().split(/\s+/);
  return { pid, parentPid: Number(rest[1]), startTicks: rest[19], state: rest[0] };
}

async function hostedFixture(manifest: Bag, mode: string, environment: Bag, input?: unknown): Promise<Bag> {
  requireCondition(executionMode() === "github-ci", "UI_EXECUTION_MODE_REFUSED");
  requireCondition(processes !== null, "UI_OWNED_PROCESS_SCOPE_REQUIRED");
  const env = { ...cleanEnv(), ...pick(environment, ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN", "PASSWORD_PEPPER_KEYS", "PASSWORD_PEPPER_ACTIVE_KEY_ID", "FVOCI_E2E_TURSO_NAMESPACE", "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "FVOCI_TEST_TURSO_DESTRUCTIVE"]) };
  env.E2E_DATABASE_BACKEND = "libsql-remote";
  env.FVOCI_E2E_TURSO_UI_SELECTED = "1";
  const child = processes.spawn([manifest.binaries["fvoci-e2e-fixture"].path, mode], "fixture-" + mode, { env, input: input ? Buffer.from(canonical(input)) : undefined });
  const stdout = await readChild(child, 120_000);
  let value: Bag;
  try {
    requireCondition(stdout.length < 64 * 1024 * 1024, "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
    value = JSON.parse(new TextDecoder().decode(stdout));
    requireCondition(value && typeof value === "object" && !Array.isArray(value), "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
  } finally {
    await processes.finish(child);
  }
  if ((child.exitCode ?? 1) !== 0) {
    const code = typeof value!.originalFailure === "string" && /^(?:TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED)$/.test(value!.originalFailure) ? value!.originalFailure : "UI_NATIVE_FIXTURE_FAILED";
    if (mode === "baseline" && code === "TURSO_UI_BASELINE_FAILED") process.stderr.write(JSON.stringify(baselineFailureDiagnostic(value!)) + "\n");
    throw new UiError(code);
  }
  requireCondition(value!.lifecycleDrain === "confirmed" && value!.leases === 0 && value!.serverCloseReceipt === "not-exposed-by-sdk", "UI_NATIVE_DRAIN_FAILED");
  return value!;
}

function pick(source: Bag, keys: string[]): Record<string, string> {
  const selected: Record<string, string> = {};
  for (const key of keys) if (typeof source[key] === "string") selected[key] = source[key];
  return selected;
}

function readChild(child: ChildProcess, timeoutMs: number): Promise<Uint8Array> {
  return new Promise((resolveRead, rejectRead) => {
    const chunks: Buffer[] = [];
    const timer = setTimeout(() => rejectRead(new UiError("UI_SERVER_START_FAILED")), timeoutMs);
    child.stdout?.on("data", (chunk) => chunks.push(Buffer.from(chunk)));
    child.once("close", () => {
      clearTimeout(timer);
      resolveRead(Buffer.concat(chunks));
    });
    child.once("error", (error) => {
      clearTimeout(timer);
      rejectRead(error);
    });
  });
}

export async function consume(phase: string, inputs: Bag, env: Env = process.env, fixture: FixtureFn = hostedFixture): Promise<void> {
  const scope = UiProcesses.enter();
  try {
    const manifest = currentBuild(env);
    requireCondition(inputs.ui_source_sha === manifest.sourceInputs.source, "UI_REVIEWED_SOURCE_REQUIRED");
    const environment = { FVOCI_LIBSQL_URL: env.FVOCI_LIBSQL_URL, FVOCI_LIBSQL_AUTH_TOKEN: env.FVOCI_LIBSQL_AUTH_TOKEN };
    const baseline = await fixture(manifest, "baseline", environment);
    writeJson(join(uiRoot(env), "baseline.private.json"), baseline);
    const baselineSha = valueDigest(baseline);
    const targetSha = createHash("sha256").update(String(environment.FVOCI_LIBSQL_URL)).digest("hex");
    if (phase === "ui-baseline") {
      process.stdout.write(baselinePassLine(manifest.sourceInputs.source, baselineSha, targetSha, Number(baseline.rows), Boolean(baseline.setupNeeded), startupBlockers(baseline).length === 0) + "\n");
      return;
    }
    requireCondition(phase === "ui-ack" && inputs.ui_baseline_sha256 === baselineSha, "UI_CURRENT_DATASET_BINDING_REQUIRED");
    requireCondition(inputs.ui_target_sha256 === targetSha, "UI_CURRENT_TARGET_BINDING_REQUIRED");
    const ackEnv = {
      ...environment,
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
      PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: randomBytes(32).toString("hex") }),
      PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    };
    await executeUi(manifest, baseline, ackEnv, env, fixture);
    process.stdout.write("TURSO_UI_ACK_PASS on=1 restart=1 off=8 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN\n");
  } finally {
    await scope.exit(null);
  }
}

async function executeUi(manifest: Bag, baseline: Bag, environment: Bag, env: Env, fixture: FixtureFn): Promise<Bag> {
  requireCondition(startupBlockers(baseline).length === 0, "UI_EXISTING_BACKGROUND_WORK_REFUSED");
  const source = manifest.sourceInputs;
  const audit: Bag = { namespaces: [], actors: {}, workspaces: {}, servers: [], targetSha256: createHash("sha256").update(String(environment.FVOCI_LIBSQL_URL)).digest("hex"), serverStarts: 0, observedFences: [] };
  const receipt: Bag = { source: source.source, tree: source.tree, backend: "libsql-remote", counts: { on: 0, restart: 0, off: 0 }, restore: "NOTRUN", precisionWorkload: "NOTRUN", matchedOnOffCost: "NOTRUN", cleanupErrors: [], originalFailure: null, preservation: { result: "NOTRUN" }, receiptWrite: "not-attempted", uiResult: "FAIL" };
  let last = environment;
  try {
    for (const flow of ["on", "off"] as const) {
      const namespace = "tui-" + randomBytes(10).toString("hex");
      audit.namespaces.push(namespace);
      const directory = join(uiRoot(env), flow);
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      const storage = join(directory, "storage");
      mkdirSync(storage, { mode: 0o700 });
      const flowEnv: Bag = {
        ...cleanEnv(env), ...environment, FVOCI_E2E_TURSO_NAMESPACE: namespace, FVOCI_REALTIME_MODE: flow,
        FVOCI_BIND: "127.0.0.1:0", FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0", FVOCI_COOKIE_SECURE: "0",
        STORAGE_DRIVER: "local", FVOCI_STORAGE_DIR: storage, FVOCI_STATIC_DIR: join(repoRoot, "apps/web/dist"),
        FVOCI_COLLAB_ENGINE: manifest.binaries["collab-engine"].path, FVOCI_COLLAB_FAMILY_LEASE_MS: "30000",
        FVOCI_COLLAB_FAMILY_RENEW_MS: "5000", FVOCI_COLLAB_MAX_ROOMS: "2", RUST_LOG: "info",
        FVOCI_MAINTENANCE_TICK_SECS: "86400", FVOCI_MAINTENANCE_INTERVAL_SECS: "86400",
        FVOCI_UPLOAD_GC_INTERVAL_SECS: "86400", FVOCI_REVISION_SWEEP_INTERVAL_SECS: "86400",
      };
      last = flowEnv;
      const setupNeeded = flow === "on" && baseline.setupNeeded;
      const owner = setupNeeded ? null : await fixture(manifest, "owner", flowEnv);
      if (owner) requireCondition(owner.commit === "confirmed" && owner.freshPrimaryReadback, "UI_OWNER_READBACK_FAILED");
      writeJson(join(directory, "actor-binding.json"), {
        schema: 1, backend: "libsql-remote", setupNeeded, namespace,
        ownerEmail: namespace + "-owner@example.invalid", memberEmail: namespace + "-member@example.invalid",
        workspaceSlug: namespace, source: source.source, tree: source.tree, schemaCurrent: true,
        commit: setupNeeded ? "not-attempted" : "confirmed", lifecycleDrain: "confirmed", leases: 0,
        baselineSha256: valueDigest(baseline), owner,
      });
      const capsule = join(directory, "actor-input.private.json");
      writeJson(capsule, { manifest, environment: flowEnv, namespace, workspaceId: owner ? owner.workspaceId : null });
      const wrapper = join(directory, "member-fixture");
      const wrapperFd = openSync(wrapper, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o700);
      try {
        fchmodSync(wrapperFd, 0o700);
        writeSync(wrapperFd, "#!/bin/sh\nexec " + shellQuote(process.execPath) + " " + shellQuote(fileUrl()) + " --actor\n");
      } finally {
        closeSync(wrapperFd);
      }
      const started = await startListening(manifest, flowEnv, directory);
      audit.serverStarts += 1;
      const browserEnv: Bag = {
        PLAYWRIGHT_BASE_URL: started.base, FVOCI_E2E_SELECTED_BACKEND: "libsql-remote", FVOCI_E2E_SELECTED_FLOW: flow,
        FVOCI_E2E_TURSO_NAMESPACE: namespace, FVOCI_E2E_TURSO_SOURCE: source.source, FVOCI_E2E_TURSO_TREE: source.tree,
        FVOCI_E2E_SELECTED_FIXTURE_BIN: wrapper, FVOCI_E2E_TURSO_PRIVATE_INPUT: capsule,
        FVOCI_E2E_TURSO_ACTOR_BINDING: join(directory, "actor-binding.json"),
      };
      receipt.counts[flow] = await runBrowser(manifest, directory, browserEnv, flow === "on" ? ON : OFF, flow === "on" ? "^selected normal main:" : undefined, env);
      if (flow === "on") {
        started.child.kill("SIGTERM");
        await processes?.finish(started.child);
        closeSync(started.log);
        const restartDir = join(directory, "restart");
        mkdirSync(restartDir, { mode: 0o700 });
        const restarted = await startListening(manifest, flowEnv, restartDir);
        audit.serverStarts += 1;
        const restartEnv = { ...browserEnv, PLAYWRIGHT_BASE_URL: restarted.base, FVOCI_E2E_SELECTED_RESTART_SOURCE: source.source };
        delete restartEnv.FVOCI_E2E_SELECTED_FIXTURE_BIN;
        delete restartEnv.FVOCI_E2E_TURSO_PRIVATE_INPUT;
        receipt.counts.restart = await runBrowser(manifest, restartDir, restartEnv, ON, "^selected normal main restart:", env);
        restarted.child.kill("SIGTERM");
        await processes?.finish(restarted.child);
        closeSync(restarted.log);
      } else {
        started.child.kill("SIGTERM");
        await processes?.finish(started.child);
        closeSync(started.log);
      }
    }
    requireCondition(JSON.stringify(receipt.counts) === JSON.stringify({ on: 1, restart: 1, off: 8 }), "UI_ACTUAL_COUNTS_FAILED");
    receipt.uiResult = "PASS";
  } catch (error) {
    receipt.originalFailure = failureCode(error);
    receipt.uiResult = "FAIL";
    throw error;
  } finally {
    const final = await fixture(manifest, "baseline", last);
    writeJson(join(uiRoot(env), "preservation.private.json"), { before: baseline, after: final, audit });
    try {
      assertPreserved(baseline, final, audit);
      receipt.preservation = { result: "PASS", afterSha256: valueDigest(final) };
    } catch (error) {
      receipt.preservation = { result: "FAIL", failure: failureCode(error), afterSha256: valueDigest(final) };
      receipt.uiResult = "FAIL";
      if (!receipt.originalFailure) receipt.originalFailure = failureCode(error);
    }
    receipt.receiptWrite = "attempted";
    try {
      writeJson(join(uiRoot(env), "ui-result.private.json"), receipt);
    } catch {
      receipt.receiptWrite = "failed";
      process.stderr.write(JSON.stringify({ originalFailure: receipt.originalFailure, receiptWrite: "failed", preservation: receipt.preservation, cleanupErrors: receipt.cleanupErrors }) + "\n");
    }
  }
  requireCondition(receipt.uiResult === "PASS" && receipt.receiptWrite !== "failed", receipt.originalFailure || "UI_RECEIPT_WRITE_FAILED");
  requireCondition(receipt.cleanupErrors.length === 0, "UI_RESOURCE_CLOSURE_FAILED");
  return receipt;
}

function fileUrl(): string {
  return join(import.meta.dir, "turso-ui.ts");
}

async function startListening(manifest: Bag, environment: Bag, directory: string): Promise<{ base: string; child: ChildProcess; log: number }> {
  const logPath = join(directory, "server.private.log");
  const log = openSync(logPath, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o600);
  fchmodSync(log, 0o600);
  requireCondition(processes !== null, "UI_OWNED_PROCESS_SCOPE_REQUIRED");
  const child = processes.spawn([manifest.binaries["fvoci-migrate"].path, "--start"], "server", { env: environment as Record<string, string>, stdout: log });
  const started = Date.now();
  const deadline = started + 10_000;
  while (Date.now() < deadline) {
    const raw = readFileSync(logPath);
    const match = /fvoci-server listening on (http:\/\/127\.0\.0\.1:\d+)/.exec(raw.toString("utf8"));
    if (match) return { base: match[1]!, child, log };
    if (child.exitCode !== null) break;
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  const diagnostic = serverStartDiagnostic({ poll: () => child.exitCode }, readFileSync(logPath), started / 1000, deadline / 1000, Date.now() / 1000);
  process.stdout.write(JSON.stringify(diagnostic) + "\n");
  child.kill("SIGKILL");
  closeSync(log);
  throw new UiError("UI_SERVER_START_FAILED");
}

async function runBrowser(manifest: Bag, directory: string, environment: Bag, spec: string, grep: string | undefined, env: Env): Promise<number> {
  const reportPath = join(directory, "playwright.private.json");
  const browserEnv = { ...cleanEnv(env), ...environment, CI: "true", FVOCI_E2E_RESULT_DIR: directory, PLAYWRIGHT_JSON_OUTPUT_FILE: reportPath };
  requireCondition(!["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN", "DATABASE_URL", "DATABASE_APP_URL", "PASSWORD_PEPPER_KEYS"].some((key) => key in browserEnv), "UI_BROWSER_SECRET_ENV_REFUSED");
  const physical = manifest.physicalInputs;
  requireCondition(digest(physical.path) === physical.sha256, "UI_PHYSICAL_RECEIPT_CHANGED");
  const admitted = (privateRead(physical.path, 64 * 1024 * 1024) as Bag).files.external;
  const cli = join(repoRoot, "node_modules/playwright/cli.js");
  const packagePath = join(repoRoot, "node_modules/playwright/package.json");
  for (const path of [cli, packagePath]) {
    const info = lstatSync(path);
    requireCondition(isAbsolute(path) && !info.isSymbolicLink() && info.isFile(), "UI_PLAYWRIGHT_CLI_REFUSED");
    requireCondition(admitted[path] === digest(path), "UI_PLAYWRIGHT_CLI_REFUSED");
  }
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
  requireCondition(packageJson.version === "1.63.0" && packageJson.bin?.playwright === "cli.js", "UI_PLAYWRIGHT_CLI_REFUSED");
  const args = [manifest.bun.path, "--no-install", cli, "test", "--config", "e2e-pending/collab-playwright.config.ts", "--reporter=line,json"];
  if (grep) args.push("--grep", grep);
  args.push("e2e-pending/" + spec);
  requireCondition(processes !== null, "UI_OWNED_PROCESS_SCOPE_REQUIRED");
  const child = processes.spawn(args, "browser", { env: browserEnv, cwd: join(repoRoot, "apps/web") });
  const code = await processes.finish(child);
  requireCondition(code === 0, "UI_ACTUAL_BROWSER_FAILED");
  if (statSync(reportPath).isFile()) chmodSync(reportPath, 0o600);
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  const titles: string[] = [];
  const visit = (suites: Bag[]) => {
    for (const suite of suites ?? []) {
      for (const item of suite.specs ?? []) if (item.ok === true) titles.push(item.title);
      visit(suite.suites ?? []);
    }
  };
  visit(report.suites ?? []);
  requireCondition(report.errors?.length === 0 && titles.length > 0, "UI_BROWSER_REPORT_FAILED");
  return titles.length;
}

export async function main(argv: string[], env: Env = process.env): Promise<number> {
  try {
    if (argv.length === 1 && argv[0] === "--record-before") {
      const directory = uiRoot(env);
      writeJson(join(directory, "source-before.json"), sourceIdentity("record-before", env));
      writeJson(join(directory, "physical-before.private.json"), physicalInputs(env));
    } else if (argv.length === 1 && argv[0] === "--freeze") freeze(env);
    else if (argv.length === 1 && argv[0] === "--actor") await actor(env);
    else throw new UiError("UI_EXPLICIT_MODE_REQUIRED");
    return 0;
  } catch (error) {
    process.stderr.write((error instanceof UiError ? error.message : "UI_CONSUMER_FAILED") + "\n");
    return 78;
  }
}

async function actor(env: Env): Promise<void> {
  if (executionMode(env) === "orca-local") loadExistingLocalLease("actor", env);
  const capsule = privateRead(env.FVOCI_E2E_TURSO_PRIVATE_INPUT ?? "") as Bag;
  const namespace = env.FVOCI_E2E_TURSO_NAMESPACE ?? "";
  requireCondition(namespace === capsule.namespace && /^tui-[a-f0-9]{20}$/.test(namespace), "UI_ACTOR_NAMESPACE_REFUSED");
  const expected: Record<string, string> = {
    E2E_USER_EMAIL: namespace + "-member@example.invalid",
    E2E_USER_PASSWORD: "memberpass1",
    E2E_USER_GIVEN_NAME: "협업",
    E2E_USER_FAMILY_NAME: "멤버",
    E2E_WORKSPACE_SLUG: namespace,
    E2E_MEMBERSHIP_ROLE: "member",
    E2E_DATABASE_BACKEND: "libsql-remote",
  };
  requireCondition(Object.entries(expected).every(([key, value]) => env[key] === value), "UI_ACTOR_INPUT_REFUSED");
  const manifest = capsule.manifest;
  requireCondition(digest(manifest.binaries["fvoci-e2e-fixture"].path) === manifest.binaries["fvoci-e2e-fixture"].sha256, "UI_ACTOR_BINARY_CHANGED");
  const scope = UiProcesses.enter();
  try {
    const result = await hostedFixture(manifest, "member", capsule.environment);
    requireCondition(result.namespace === namespace && (capsule.workspaceId == null || result.workspaceId === capsule.workspaceId) && result.commit === "confirmed" && result.freshPrimaryReadback, "UI_ACTOR_RECEIPT_FAILED");
    writeJson(join(resolve(env.FVOCI_E2E_TURSO_PRIVATE_INPUT!, ".."), "member-" + result.userId + ".json"), result);
    process.stdout.write(result.userId + "\n");
  } finally {
    await scope.exit(null);
  }
}

if (import.meta.main) process.exit(await main(process.argv.slice(2)));
