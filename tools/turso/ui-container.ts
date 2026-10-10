// orca-local mode: the native fixture and normal main run in the canonical
// image, released by a fixed launcher only after the paused shell is bound.
// Credentials reach the container only through the read-only private capsule.
import {
  chmodSync,
  closeSync,
  constants,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  realpathSync,
  statSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import { root as checkout, sha, uid } from "../selected-backend-ci/io.ts";
import {
  fixtureInput,
  cleanEnv,
  decodeUtf8,
  diagnosticJson,
  executionMode,
  failureCode,
  get,
  isRecord,
  parseBytes,
  parsePlain,
  record,
  require,
  token,
  UiError,
  type Record_,
} from "./ui-common.ts";
import type { Started } from "./ui-native.ts";
import {
  communicate,
  identity,
  mkfifo,
  now,
  pause,
  poll,
  procIdentity,
  readable,
  SIGKILL,
  SIGTERM,
  wait,
  WaitTimeout,
  type Child,
  type Identity,
  type ProcRow,
  type Scope,
} from "./ui-processes.ts";
import { loadLocalLease, type LocalLease, type Manifest } from "./ui-record.ts";
import { awaitListening, portClosed, publishStartDiagnostic, requireSetup } from "./ui-server.ts";
import { SERVER_BUDGET } from "./ui-start-diagnostic.ts";
import { pySplitlines } from "../web-e2e/compat.ts";

export const QUALIFIED_CANONICAL_IMAGE =
  "sha256:396a5f8e43e8de4b2e1567f2c8a8e841bf45037a4e4ff7cb76dc384951025f35";
export const QUALIFIED_SHELL = "/bin/sh";
// 12GiB is the hosted fixture allocation, not a project minimum.
export const DAEMON_CAPS: Readonly<Record<string, string>> = {
  "memory.max": "12884901888",
  "cpu.max": "max 100000",
  "pids.max": "128",
  "memory.swap.max": "0",
};
export const SECRET_ENV_KEYS = ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN"];
export const NATIVE_CAPSULE_KEYS = [
  ...SECRET_ENV_KEYS,
  "PASSWORD_PEPPER_KEYS",
  "PASSWORD_PEPPER_ACTIVE_KEY_ID",
  "FVOCI_E2E_TURSO_NAMESPACE",
  "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE",
  "FVOCI_TEST_TURSO_DESTRUCTIVE",
  "E2E_DATABASE_BACKEND",
  "FVOCI_E2E_TURSO_UI_SELECTED",
  "FVOCI_DATABASE_BACKEND",
  "FVOCI_REALTIME_MODE",
  "FVOCI_BIND",
  "FVOCI_PUBLIC_ORIGIN",
  "FVOCI_COOKIE_SECURE",
  "STORAGE_DRIVER",
  "FVOCI_STORAGE_DIR",
  "FVOCI_STATIC_DIR",
  "FVOCI_COLLAB_ENGINE",
  "FVOCI_COLLAB_FAMILY_LEASE_MS",
  "FVOCI_COLLAB_FAMILY_RENEW_MS",
  "FVOCI_COLLAB_MAX_ROOMS",
  "RUST_LOG",
  "FVOCI_MAINTENANCE_TICK_SECS",
  "FVOCI_MAINTENANCE_INTERVAL_SECS",
  "FVOCI_UPLOAD_GC_INTERVAL_SECS",
  "FVOCI_REVISION_SWEEP_INTERVAL_SECS",
];
export const FIXED_LAUNCHER =
  "#!/bin/sh\n" +
  "set -eu\n" +
  "set -a\n" +
  '. "$1"\n' +
  "set +a\n" +
  "shift\n" +
  "newline='\n'\n" +
  "read -r fvoci_stat < /proc/$$/stat || exit 78\n" +
  "exec 3>/fvoci-private/stat.ready\n" +
  '[ "${#fvoci_stat}" -le 511 ] && case $fvoci_stat in *"$newline"*) false ;; *) true ;; esac || exit 78\n' +
  "printf '%s\\n' \"$fvoci_stat\" >&3\n" +
  "exec 3>&-\n" +
  "exec 3</fvoci-private/exec.go\n" +
  "read -r fvoci_go <&3 || exit 78\n" +
  "exec 3<&-\n" +
  '[ "$fvoci_go" = GO ] || exit 78\n' +
  'exec "$@"\n';
export const LAUNCHER_DST = "/fvoci-current/launcher.sh";
export const CAPSULE_DST = "/fvoci-private/native-env.sh";
export const BINARY_DST = "/fvoci-current/bin";
export const DIST_DST = "/fvoci-current/dist";
export const ONE_SHOT_LIVE = "unsupported-before-execution";
export const BLOCKED = "BLOCKED";
export const FIXTURE_BUDGET = 120;
export const STAT_READ_BOUND = 512;
export const STAT_BODY_MAX = 511;
export const STAT_FIFO_NAME = "stat.ready";
export const GO_FIFO_NAME = "exec.go";
export const STAT_FIFO_DST = "/fvoci-private/stat.ready";
export const GO_FIFO_DST = "/fvoci-private/exec.go";
export const GO_MARKER = new TextEncoder().encode("GO\n");
// Linux O_CLOEXEC; node:fs exposes no constant for it.
const O_CLOEXEC = 0o2000000;

export interface Bound {
  maintenance: { pid: number; startTicks: string };
  retirement: { pid: number; startTicks: string };
  clientPid: number;
  comm: "sh";
  caps: Record<string, string>;
  uid: number[];
  gid: number[];
  allocation?: number;
}
/** Server allocation state carried from start to stop. */
export interface ServerHandle {
  child: Child;
  containerId?: string;
  goFd?: number | null;
  daemonSample?: Bound;
  maintenanceIdentity?: { pid: number; startTicks: string };
  hostIdentity?: { pid: number; startTicks: string };
}

const isInt = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value);

/** The orca-local lease loader; tests and the admission wiring set it. */
export const lease: { load: LocalLease | undefined } = { load: undefined };

export function allocationCaps(grant?: unknown): Record<string, string> {
  if (executionMode() === "github-ci") return { ...DAEMON_CAPS };
  require(isRecord(grant), "UI_LOCAL_ALLOCATION_REFUSED");
  const runtime = grant.canonicalRuntime;
  require(isRecord(runtime) && runtime.networkAuthorized === true, "UI_LOCAL_ALLOCATION_REFUSED");
  return { ...DAEMON_CAPS, "memory.max": "max", "memory.swap.max": "max" };
}

export function readShellProof(runtime: unknown): string {
  const proof = isRecord(runtime) ? runtime.shellProof : undefined;
  require(isRecord(proof), "UI_CANONICAL_SHELL_UNAVAILABLE");
  const path = proof.path,
    hash = proof.sha256;
  require(typeof path === "string" &&
    path.startsWith("/") &&
    !path.startsWith("//"), "UI_CANONICAL_SHELL_UNAVAILABLE");
  let link: boolean;
  try {
    link = lstatSync(path).isSymbolicLink();
  } catch {
    link = false;
  }
  require(isAbsolute(path) && !link, "UI_CANONICAL_SHELL_UNAVAILABLE");
  require(typeof hash === "string" &&
    /^[0-9a-f]{64}$/.test(hash), "UI_CANONICAL_SHELL_UNAVAILABLE");
  require(sha(path) === hash, "UI_CANONICAL_SHELL_UNAVAILABLE");
  const config = record(JSON.parse(readFileSync(path, "utf8")), "Config");
  require(config.Image === QUALIFIED_CANONICAL_IMAGE &&
    get(runtime, "imageId") === QUALIFIED_CANONICAL_IMAGE, "UI_CANONICAL_SHELL_UNAVAILABLE");
  const cmd = Array.isArray(config.Cmd) ? config.Cmd : [];
  require(cmd.includes(QUALIFIED_SHELL), "UI_CANONICAL_SHELL_UNAVAILABLE");
  const environment = Array.isArray(config.Env) ? config.Env : [];
  require(!environment.some((item) =>
    String(item).startsWith("FVOCI_LIBSQL_"),
  ), "UI_DOCKER_ENV_SECRET_REFUSED");
  return QUALIFIED_SHELL;
}

export function openReferencedGrant(
  mode: string,
  load: LocalLease | undefined = lease.load,
): Record_ {
  require(executionMode() === "orca-local", "UI_EXECUTION_MODE_REFUSED");
  const grant = loadLocalLease(mode, load) as unknown as Record_;
  const runtime = grant.canonicalRuntime;
  require(isRecord(runtime) && runtime.networkAuthorized === true, "UI_LOCAL_NETWORK_NOT_GRANTED");
  require(readShellProof(runtime) === QUALIFIED_SHELL, "UI_CANONICAL_SHELL_UNAVAILABLE");
  return grant;
}

/** POSIX sh quoting of one word (shlex.quote). */
export function shellQuote(value: string): string {
  if (!value) return "''";
  if (!/[^\w@%+=:,./-]/.test(value)) return value;
  return "'" + value.replaceAll("'", "'\"'\"'") + "'";
}

export function capsuleText(environment: Record<string, string>): string {
  require(!Object.hasOwn(environment, "LOCPATH"), "UI_LOCPATH_REFUSED");
  const leaked = Object.keys(environment).filter(
    (key) =>
      !NATIVE_CAPSULE_KEYS.includes(key) &&
      (key.startsWith("FVOCI_LIBSQL_") ||
        key.startsWith("PASSWORD_") ||
        key.includes("TOKEN") ||
        key.includes("SECRET") ||
        key === "LOCPATH"),
  );
  require(!leaked.length, "UI_DOCKER_ENV_SECRET_REFUSED");
  const lines: string[] = [];
  for (const key of NATIVE_CAPSULE_KEYS) {
    if (!Object.hasOwn(environment, key)) continue;
    const value = environment[key];
    require(typeof value === "string" &&
      !value.includes("\0") &&
      !value.includes("\n"), "UI_DOCKER_ENV_SECRET_REFUSED");
    lines.push(key + "=" + shellQuote(value) + "\n");
  }
  require(SECRET_ENV_KEYS.every((key) =>
    lines.some((line) => line.startsWith(key + "=")),
  ), "UI_DOCKER_ENV_SECRET_REFUSED");
  return lines.join("");
}

export function writeReadonlyCapsule(
  directory: string,
  environment: Record<string, string>,
): string {
  const path = join(directory, "native-env.sh");
  const fd = openSync(path, "wx", 0o600);
  try {
    writeSync(fd, capsuleText(environment));
  } finally {
    closeSync(fd);
  }
  require((statSync(path).mode & 0o777) === 0o600, "UI_PRIVATE_INPUT_REFUSED");
  return path;
}

export function writeLauncher(directory: string): string {
  const path = join(directory, "launcher.sh");
  const fd = openSync(path, "wx", 0o700);
  try {
    writeSync(fd, FIXED_LAUNCHER);
  } finally {
    closeSync(fd);
  }
  chmodSync(path, 0o500);
  require(readFileSync(path, "utf8") === FIXED_LAUNCHER, "UI_CANONICAL_SHELL_UNAVAILABLE");
  return path;
}

export function containerArgv(
  name: string,
  launcher: string,
  capsule: string,
  binary: string,
  mountArgs: string[],
  command: string[],
  grant?: unknown,
): string[] {
  require(launcher.startsWith("/") && capsule.startsWith("/"), "UI_CURRENT_ARTIFACT_MISSING");
  const caps = allocationCaps(grant);
  const limit = caps["memory.max"] as string;
  const memory = limit === "max" ? [] : ["--memory", limit, "--memory-swap", limit];
  const argv = [
    "docker",
    "create",
    "--name",
    name,
    "--read-only",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--user",
    "1000:1000",
    ...memory,
    "--pids-limit",
    caps["pids.max"] as string,
    "--tmpfs",
    "/tmp:rw,noexec,nosuid,size=67108864,uid=1000,gid=1000",
    "--network",
    "host",
    "--entrypoint",
    QUALIFIED_SHELL,
    ...mountArgs,
    QUALIFIED_CANONICAL_IMAGE,
    LAUNCHER_DST,
    CAPSULE_DST,
    binary,
    ...command,
  ];
  require(!argv.includes("--env-file") && !argv.includes("-e"), "UI_DOCKER_ENV_SECRET_REFUSED");
  require(!["--cpus", "--cpu-quota", "--cpu-period", "--cpuset-cpus"].some((flag) =>
    argv.includes(flag),
  ), "UI_DAEMON_CAP_REFUSED");
  return argv;
}

export function admitPublishedConfig(
  argv: unknown[],
  envItems: unknown[],
  secretValues: string[],
): void {
  const keys: string[] = [];
  for (const item of envItems) {
    const value = String(item),
      at = value.indexOf("=");
    require(at >= 0, "UI_DOCKER_ENV_SECRET_REFUSED");
    keys.push(value.slice(0, at));
  }
  require(!SECRET_ENV_KEYS.some((key) => keys.includes(key)), "UI_DOCKER_ENV_SECRET_REFUSED");
  const published = [...argv.map(String), ...envItems.map(String)];
  for (const value of secretValues)
    require(value &&
      !published.some((item) => item.includes(value)), "UI_DOCKER_ENV_SECRET_REFUSED");
  require(!argv.includes("--env-file") && !argv.includes("-e"), "UI_DOCKER_ENV_SECRET_REFUSED");
}

export interface Creation {
  phase: "created";
  liveDaemon: string;
  qualification: string;
  cgroupCaps: string;
  shell: string;
  image: string;
}
export function creationIdentity(
  inspect: unknown,
  argv: string[],
  secretValues: string[],
  kind: string,
): Creation {
  const config = record(inspect, "Config"),
    host = record(inspect, "HostConfig");
  require(get(inspect, "State", "Pid") === 0 &&
    get(inspect, "State", "Running") === false, "UI_DAEMON_PID_REFUSED");
  require(config.Image === QUALIFIED_CANONICAL_IMAGE, "UI_CANONICAL_SHELL_UNAVAILABLE");
  const entrypoint = Array.isArray(config.Entrypoint) ? config.Entrypoint : [];
  require(entrypoint.length === 1 &&
    entrypoint[0] === QUALIFIED_SHELL, "UI_CANONICAL_SHELL_UNAVAILABLE");
  admitPublishedConfig(
    Array.isArray(config.Cmd) ? config.Cmd : [],
    Array.isArray(config.Env) ? config.Env : [],
    secretValues,
  );
  admitPublishedConfig(argv, [], secretValues);
  // HostConfig is a create-shape refusal only. It is not cgroup cap proof.
  const capDrop = Array.isArray(host.CapDrop) ? host.CapDrop : [];
  require(host.ReadonlyRootfs === true &&
    capDrop.length === 1 &&
    capDrop[0] === "ALL", "UI_DAEMON_CAP_REFUSED");
  require(config.User === "1000:1000" && host.Privileged !== true, "UI_DAEMON_CAP_REFUSED");
  // Present CPU fields must be the Docker zero values. Missing fields are not proof.
  if (Object.hasOwn(host, "CpuQuota"))
    require(isInt(host.CpuQuota) && host.CpuQuota === 0, "UI_DAEMON_CAP_REFUSED");
  if (Object.hasOwn(host, "NanoCpus"))
    require(isInt(host.NanoCpus) && host.NanoCpus === 0, "UI_DAEMON_CAP_REFUSED");
  if (Object.hasOwn(host, "CpusetCpus")) require(host.CpusetCpus === "", "UI_DAEMON_CAP_REFUSED");
  return {
    phase: "created",
    liveDaemon: kind === "fixture" ? ONE_SHOT_LIVE : "pending-listen",
    qualification: BLOCKED,
    cgroupCaps: "not-observed",
    shell: QUALIFIED_SHELL,
    image: QUALIFIED_CANONICAL_IMAGE,
  };
}

export function finishedOneShot(creation: Record_, returncode: unknown) {
  require(creation.qualification === BLOCKED &&
    creation.liveDaemon === ONE_SHOT_LIVE, "UI_DAEMON_OBSERVATION_UNSUPPORTED");
  require(creation.cgroupCaps === "not-observed", "UI_DAEMON_CAP_REFUSED");
  require(isInt(returncode), "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
  return { productExit: returncode, liveDaemon: ONE_SHOT_LIVE, qualification: BLOCKED };
}

export function runningDaemonSample(
  inspect: unknown,
  caps: Record_,
  procRow: Record_,
  nspid: unknown,
  clientPid: number,
  binary: string,
  waited: unknown,
  grant?: unknown,
) {
  require(waited === true, "UI_DAEMON_WAIT_MISSING");
  const state = record(inspect, "State");
  const pid = state.Pid;
  require(isInt(pid) &&
    pid > 0 &&
    pid === procRow.pid &&
    pid !== clientPid, "UI_DAEMON_PID_REFUSED");
  require(isInt(nspid) && nspid > 0 && nspid !== pid, "UI_NAMESPACE_PID_REFUSED");
  require(state.Running === true && state.OOMKilled === false, "UI_DAEMON_STATE_REFUSED");
  require(!Object.hasOwn(caps, "HostConfig") &&
    !Object.hasOwn(caps, "CapDrop"), "UI_DAEMON_CAP_REFUSED");
  const expected = allocationCaps(grant);
  for (const [key, required] of Object.entries(expected))
    require(caps[key] === required, "UI_DAEMON_CAP_REFUSED");
  require(procRow.comm === "fvoci-server", "UI_NORMAL_MAIN_IDENTITY_FAILED");
  const ticks = procRow.startTicks;
  require((typeof ticks === "string" || typeof ticks === "number") &&
    /^[0-9]+$/.test(String(ticks)), "UI_DAEMON_PID_REFUSED");
  if (procRow.exeInspection === "observed")
    require(procRow.exe === binary, "UI_CANONICAL_IMAGE_BINARY_REFUSED");
  else require(procRow.exeInspection === "UNAVAILABLE", "UI_NORMAL_MAIN_IDENTITY_FAILED");
  return {
    daemonPid: pid,
    pid: nspid,
    startTicks: ticks,
    waited: true,
    qualification: "daemon-observed",
    caps: Object.fromEntries(Object.keys(expected).map((key) => [key, caps[key]])),
  };
}

export function budgetDeadline(seconds: number, clock: () => number = now): number {
  require(seconds === FIXTURE_BUDGET || seconds === SERVER_BUDGET, "UI_SERVER_START_FAILED");
  return clock() + seconds;
}
export function budgetRemain(deadline: number, clock: () => number = now): number {
  const remain = deadline - clock();
  require(remain > 0, "UI_SERVER_START_FAILED");
  return remain;
}
export const boundedClientTimeout = (deadline: number) => Math.min(10, budgetRemain(deadline));

export function acceptStatPayload(payload: Uint8Array): string {
  const newlines = payload.filter((byte) => byte === 0x0a).length;
  require(payload instanceof Uint8Array &&
    payload[payload.length - 1] === 0x0a &&
    newlines === 1 &&
    payload.length - 1 >= 1 &&
    payload.length - 1 <= STAT_BODY_MAX &&
    payload.length <= STAT_READ_BOUND, "UI_DAEMON_PID_REFUSED");
  return decodeUtf8(payload.subarray(0, -1));
}

export function statFields(value: unknown): [string, string] {
  require(typeof value === "string" &&
    value.includes(" ") &&
    value.includes(")"), "UI_DAEMON_PID_REFUSED");
  const first = value.slice(0, value.indexOf(" "));
  const tail = value
    .slice(value.lastIndexOf(")") + 1)
    .split(/\s+/)
    .filter(Boolean);
  require(/^[0-9]+$/.test(first) &&
    tail.length > 19 &&
    /^[0-9]+$/.test(tail[19] as string), "UI_DAEMON_PID_REFUSED");
  return [first, tail[19] as string];
}

export const statOpenFlags = () => constants.O_RDONLY | constants.O_NONBLOCK | O_CLOEXEC;
export const goOpenFlags = () => constants.O_RDWR | constants.O_NONBLOCK | O_CLOEXEC;
export const openStatReader = (path: string) => openSync(path, statOpenFlags());
export const openGoHolder = (path: string) => openSync(path, goOpenFlags());

/** select(2) on the stat FIFO until readable or the budget ends; one bounded read. */
export async function readStatOnce(fd: number, deadline: number): Promise<string> {
  for (;;) {
    budgetRemain(deadline);
    if (readable(fd)) break;
    await pause(20);
    require(now() < deadline, "UI_SERVER_START_FAILED");
  }
  const buffer = new Uint8Array(STAT_READ_BOUND);
  const n = readSync(fd, buffer, 0, STAT_READ_BOUND, null);
  return acceptStatPayload(buffer.subarray(0, n));
}

export function writeGoOnce(
  fd: number,
  writer: (fd: number, data: Uint8Array) => number = writeSync,
): void {
  const written = writer(fd, GO_MARKER);
  require(written === GO_MARKER.length, "UI_DAEMON_OBSERVATION_UNSUPPORTED");
}

export function preparePrivateFifos(directory: string): [string, string] {
  const made: string[] = [];
  for (const name of [STAT_FIFO_NAME, GO_FIFO_NAME]) {
    const path = join(directory, name);
    mkfifo(path, 0o600);
    chmodSync(path, 0o600);
    const info = statSync(path);
    require(info.isFIFO() &&
      (info.mode & 0o777) === 0o600 &&
      info.uid === uid(), "UI_PRIVATE_INPUT_REFUSED");
    made.push(path);
  }
  return made as [string, string];
}

export function unlinkPrivateFifos(directory: string): void {
  for (const name of [STAT_FIFO_NAME, GO_FIFO_NAME]) {
    try {
      unlinkSync(join(directory, name));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }
}

const fieldsAfter = (value: string, label: string) => {
  const line = pySplitlines(value).find((item) => item.startsWith(label));
  return line === undefined ? undefined : line.split(/\s+/).filter(Boolean).slice(1);
};
export function parseStatusFields(value: string, label: string): number[] {
  const fields = fieldsAfter(value, label);
  require(fields !== undefined &&
    fields.length === 4 &&
    fields.every((item) => /^[0-9]+$/.test(item)), "UI_DAEMON_PID_REFUSED");
  return fields.map(Number);
}
export function parseNspid(value: string): number[] {
  const fields = fieldsAfter(value, "NSpid:");
  require(fields !== undefined &&
    fields.length === 2 &&
    fields.every((item) => /^[0-9]+$/.test(item)), "UI_NAMESPACE_PID_REFUSED");
  return fields.map(Number);
}
export const readProcStatus = (pid: number) =>
  readFileSync(join("/proc", String(pid), "status"), "utf8");
export function readDaemonCaps(pid: number): Record<string, string> {
  const lines = readFileSync(join("/proc", String(pid), "cgroup"), "utf8").split("\n");
  const line = lines.find((item) => item.startsWith("0:"));
  if (line === undefined) throw new TypeError("no unified cgroup");
  const cgroup = "/sys/fs/cgroup" + line.split(":").slice(2).join(":");
  return Object.fromEntries(
    Object.keys(DAEMON_CAPS).map((key) => [key, readFileSync(join(cgroup, key), "utf8").trim()]),
  );
}

/** The daemon observations the local proofs read; tests replace them. */
export interface DaemonProc {
  identity(pid: number): Identity;
  procIdentity(pid: number): ProcRow;
  readProcStatus(pid: number): string;
  readDaemonCaps(pid: number): Record<string, string>;
}
export const linuxProc: DaemonProc = { identity, procIdentity, readProcStatus, readDaemonCaps };

export function bindPausedShell(
  statText: string,
  inspect: unknown,
  procRow: Record_,
  nspidFields: unknown,
  caps: Record_,
  clientPid: number,
  credentials: { uid?: unknown; gid?: unknown },
  grant?: unknown,
): Bound {
  const [first, ticks] = statFields(statText);
  const state = record(inspect, "State");
  const pid = state.Pid;
  require(isInt(pid) && pid > 0 && pid !== clientPid, "UI_DAEMON_PID_REFUSED");
  require(Array.isArray(nspidFields) && nspidFields.length === 2, "UI_NAMESPACE_PID_REFUSED");
  const [hostNs, namespace] = nspidFields as unknown[];
  require(isInt(hostNs) &&
    isInt(namespace) &&
    pid === hostNs &&
    first === String(namespace) &&
    pid !== namespace, "UI_NAMESPACE_PID_REFUSED");
  require(procRow.pid === pid && procRow.startTicks === ticks, "UI_DAEMON_PID_REFUSED");
  require(procRow.comm === "sh", "UI_NORMAL_MAIN_IDENTITY_FAILED");
  require(state.Running === true && state.OOMKilled === false, "UI_DAEMON_STATE_REFUSED");
  require(isRecord(caps) &&
    !Object.hasOwn(caps, "HostConfig") &&
    !Object.hasOwn(caps, "CapDrop"), "UI_DAEMON_CAP_REFUSED");
  const expected = allocationCaps(grant);
  for (const [key, required] of Object.entries(expected))
    require(caps[key] === required, "UI_DAEMON_CAP_REFUSED");
  const fixed = (value: unknown) =>
    Array.isArray(value) && value.length === 4 && value.every((item) => item === 1000);
  require(fixed(credentials.uid) && fixed(credentials.gid), "UI_DAEMON_PID_REFUSED");
  return {
    maintenance: { pid: namespace, startTicks: ticks },
    retirement: { pid, startTicks: ticks },
    clientPid,
    comm: "sh",
    caps: Object.fromEntries(Object.keys(expected).map((key) => [key, caps[key] as string])),
    uid: [...(credentials.uid as number[])],
    gid: [...(credentials.gid as number[])],
  };
}

export function allocationIndex(scope: Pick<Scope, "allocations">, child: Child): number {
  const index = scope.allocations.findIndex((a) => a.process === child);
  if (index < 0) throw new TypeError("unknown allocation");
  return index;
}

export function captureOwnedDaemon(
  scope: Pick<Scope, "allocations" | "capture">,
  client: Child,
  row: ProcRow,
): number {
  const index = allocationIndex(scope, client);
  scope.capture(row, "container-init", index);
  return index;
}

export async function dockerClient(
  scope: Scope,
  args: string[],
  timeout: number,
): Promise<Uint8Array> {
  require(args.length > 0 &&
    args[0] === "docker" &&
    !args.includes("--env-file") &&
    !args.includes("-e"), "UI_DOCKER_CLIENT_FAILED");
  const child = scope.spawn(args, "docker-client", {
    env: cleanEnv(),
    stdout: "pipe",
    stderr: "ignore",
  });
  let stdout: Uint8Array = new Uint8Array(),
    original: unknown = null;
  try {
    stdout = await communicate(child, "", timeout);
    if (poll(child) !== 0) throw new UiError("UI_DOCKER_CLIENT_FAILED");
  } catch (error) {
    original = error;
  }
  if (poll(child) === null) child.kill(SIGKILL);
  try {
    await scope.finish(child);
  } catch (error) {
    // A finished client's closure failure is the failure; otherwise the client's own is.
    if (poll(child) === 0) throw error as Error;
  }
  if (original !== null) throw original as Error;
  return stdout;
}

/** The docker client the local paths call; tests replace it. */
export const docker = { client: dockerClient };

const inspectState = async (scope: Scope, containerId: string, timeout: number) =>
  record(
    parseBytes(await docker.client(scope, ["docker", "inspect", containerId], timeout)),
    0,
    "State",
  );

export async function samplePausedDaemon(
  scope: Scope,
  containerId: string,
  statText: string,
  client: Child,
  deadline: number,
  grant: unknown,
  proc: DaemonProc = linuxProc,
): Promise<Bound> {
  const state = await inspectState(scope, containerId, boundedClientTimeout(deadline));
  const pid = state.Pid as number;
  const status = proc.readProcStatus(pid);
  const bound = bindPausedShell(
    statText,
    { State: state },
    proc.identity(pid) as unknown as Record_,
    parseNspid(status),
    proc.readDaemonCaps(pid),
    client.pid,
    { uid: parseStatusFields(status, "Uid:"), gid: parseStatusFields(status, "Gid:") },
    grant,
  );
  bound.allocation = captureOwnedDaemon(scope, client, proc.procIdentity(pid));
  return bound;
}

export async function releaseAttached(
  scope: Scope,
  directory: string,
  containerId: string,
  client: Child,
  deadline: number,
  grant: unknown,
  proc: DaemonProc = linuxProc,
): Promise<[Bound, number]> {
  const statFd = openStatReader(join(directory, STAT_FIFO_NAME));
  let goFd: number | null = null;
  try {
    goFd = openGoHolder(join(directory, GO_FIFO_NAME));
    const statText = await readStatOnce(statFd, deadline);
    const sample = await samplePausedDaemon(
      scope,
      containerId,
      statText,
      client,
      deadline,
      grant,
      proc,
    );
    writeGoOnce(goFd);
    return [sample, goFd];
  } catch (error) {
    if (goFd !== null) closeSync(goFd);
    throw error;
  } finally {
    closeSync(statFd);
  }
}

export interface DaemonExit {
  Pid: unknown;
  ExitCode: unknown;
  OOMKilled: unknown;
}
export async function readDaemonExit(
  scope: Scope,
  containerId: unknown,
  timeout: number,
): Promise<DaemonExit | null> {
  require(typeof containerId === "string" &&
    /^[0-9a-f]{64}$/.test(containerId), "UI_DOCKER_CLIENT_FAILED");
  require(timeout > 0 && timeout <= 10, "UI_SERVER_START_FAILED");
  const child = scope.spawn(["docker", "wait", containerId], "docker-wait", {
    env: cleanEnv(),
    stdout: "pipe",
    stderr: "ignore",
  });
  let timedOut = false,
    original: unknown = null;
  try {
    await communicate(child, "", timeout);
  } catch (error) {
    if (error instanceof WaitTimeout) {
      timedOut = true;
      child.kill(SIGKILL);
      try {
        await wait(child, 10);
      } catch {
        original = new UiError("UI_DOCKER_CLIENT_FAILED");
      }
    } else original = error;
  }
  if (poll(child) === null) {
    child.kill(SIGKILL);
    await wait(child, 10);
  }
  try {
    await scope.finish(child);
  } catch (error) {
    // A killed wait client is expected to fail its normal closure.
    if (!(error instanceof UiError) || !timedOut) throw error as Error;
  }
  if (original !== null) throw original as Error;
  if (timedOut || poll(child) !== 0) return null;
  const state = await inspectState(scope, containerId, timeout);
  return { Pid: state.Pid, ExitCode: state.ExitCode, OOMKilled: state.OOMKilled };
}

export function nativeRows(scope: Pick<Scope, "allocations" | "entries">, child: Child) {
  const index = allocationIndex(scope, child);
  return [index, [...scope.entries.values()].filter((e) => e.allocation === index)] as const;
}

export function requireNativeRetired(
  scope: Scope,
  child: Child,
  retirement: { pid: number; startTicks: string },
): void {
  require(scope.retired(retirement), "UI_PROCESS_CLOSURE_FAILED");
  const [, rows] = nativeRows(scope, child);
  const native = rows.filter((entry) => entry.identity.pid !== child.pid);
  require(native.length > 0 &&
    native.every((entry) => scope.retired(entry.identity)), "UI_PROCESS_CLOSURE_FAILED");
}

export function allocationRetired(scope: Scope, child: Child): number {
  const [index, rows] = nativeRows(scope, child);
  const allocation = scope.allocations[index];
  require(rows.length > 0 &&
    allocation?.closed === true &&
    !allocation.forced &&
    rows.every((entry) => scope.retired(entry.identity)), "UI_PROCESS_CLOSURE_FAILED");
  return index;
}

export function nativeFailureCode(value: Record_, exitCode: unknown): string | null {
  if (!isInt(exitCode) || exitCode === 0) return null;
  const code = Object.hasOwn(value, "originalFailure")
    ? value.originalFailure
    : "UI_NATIVE_FIXTURE_FAILED";
  require(typeof code === "string" &&
    /^(?:TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED)$/.test(code), "UI_NATIVE_FAILURE_CODE_REFUSED");
  return code;
}

export function qualifyDaemonExit(state: unknown, forced: unknown): [boolean, number | null] {
  if (!isRecord(state) || state.Pid !== 0) return [false, null];
  const code = state.ExitCode,
    oom = state.OOMKilled;
  if (!isInt(code) || typeof oom !== "boolean" || typeof forced !== "boolean") return [false, null];
  return [!forced && code === 0 && !oom, code];
}

export async function proveNormalDaemon(
  scope: Scope,
  child: Child,
  sample: Bound,
  state: DaemonExit | null,
  forced: boolean,
) {
  const [qualified, productExit] = qualifyDaemonExit(state ?? {}, forced);
  require(qualified, "UI_DAEMON_OBSERVATION_UNSUPPORTED");
  requireNativeRetired(scope, child, sample.retirement);
  const clientCode = await scope.finish(child);
  require(clientCode === 0, "UI_PROCESS_CLOSURE_FAILED");
  return {
    productExit,
    clientExit: clientCode,
    qualification: "retired",
    liveDaemon: "retired",
    caps: sample.caps,
    uid: sample.uid,
    gid: sample.gid,
    maintenance: sample.maintenance,
    retirement: sample.retirement,
    allocation: allocationRetired(scope, child),
  };
}

export function verifyOwnClosure(scope: Scope, child: Child | null, directory: string): void {
  require([STAT_FIFO_NAME, GO_FIFO_NAME].every(
    (name) => !existsSync(join(directory, name)),
  ), "UI_PROCESS_CLOSURE_FAILED");
  if (child === null) return;
  require(poll(child) !== null, "UI_PROCESS_CLOSURE_FAILED");
  const [index, rows] = nativeRows(scope, child);
  require(scope.allocations[index]?.closed === true &&
    rows.every((entry) => scope.retired(entry.identity)), "UI_PROCESS_CLOSURE_FAILED");
}

export async function cleanupOwned(
  scope: Scope,
  containerId: string | null | undefined,
  child: Child | null,
  directory: string,
  fds: (number | null | undefined)[],
): Promise<string[]> {
  const errors: string[] = [];
  const seen = new Set<number>();
  for (const fd of fds) {
    if (fd === null || fd === undefined || seen.has(fd)) continue;
    seen.add(fd);
    try {
      closeSync(fd);
    } catch {
      errors.push("UI_PROCESS_CLOSURE_FAILED");
    }
  }
  try {
    if (child !== null && poll(child) === null)
      await docker.client(scope, ["docker", "stop", "-t", "10", containerId as string], 10);
  } catch {
    errors.push("UI_DOCKER_CLIENT_FAILED");
  }
  try {
    if (child !== null) await scope.finish(child);
  } catch {
    errors.push("UI_PROCESS_CLOSURE_FAILED");
  }
  try {
    if (containerId !== null && containerId !== undefined)
      await docker.client(scope, ["docker", "rm", containerId], 10);
  } catch {
    errors.push("UI_DOCKER_CLIENT_FAILED");
  }
  try {
    unlinkPrivateFifos(directory);
  } catch {
    errors.push("UI_PROCESS_CLOSURE_FAILED");
  }
  try {
    verifyOwnClosure(scope, child, directory);
  } catch {
    errors.push("UI_PROCESS_CLOSURE_FAILED");
  }
  return errors;
}

export function reportCleanup(
  scope: Pick<Scope, "io">,
  original: unknown,
  errors: string[],
): unknown {
  if (errors.length)
    scope.io.output.err(
      diagnosticJson({
        originalFailure: original !== null && original !== undefined ? failureCode(original) : null,
        cleanupErrors: errors,
      }),
    );
  return original;
}

export async function releaseFailedPublish(
  scope: Scope,
  directory: string,
  containerId: string | null,
  original: unknown,
): Promise<unknown> {
  const errors: string[] = [];
  if (containerId !== null) {
    try {
      await docker.client(scope, ["docker", "rm", containerId], 10);
    } catch {
      errors.push("UI_DOCKER_CLIENT_FAILED");
    }
  }
  try {
    unlinkPrivateFifos(directory);
  } catch {
    errors.push("UI_PROCESS_CLOSURE_FAILED");
  }
  reportCleanup(scope, original, errors);
  return original;
}

export function mountSpec(src: string, dst: string, readonly: boolean): string {
  require(src.startsWith("/") &&
    dst.startsWith("/") &&
    !/[,\n]/.test(src + dst), "UI_CURRENT_ARTIFACT_MISSING");
  return "type=bind,src=" + src + ",dst=" + dst + (readonly ? ",readonly" : "");
}

export async function publishContainer(
  scope: Scope | null,
  directory: string,
  environment: Record<string, string>,
  manifest: Pick<Manifest, "binaries">,
  binaryName: string,
  command: string[],
  kind: string,
  grantFor: (kind: string) => Record_ = (k) => openReferencedGrant(k),
): Promise<[string, Creation, Record_]> {
  const grant = grantFor(kind);
  const secretValues = SECRET_ENV_KEYS.map((key) => get(environment, key) as string);
  const storage = environment.FVOCI_STORAGE_DIR;
  require(typeof storage === "string" &&
    storage.startsWith("/") &&
    !storage.includes(","), "UI_CURRENT_ARTIFACT_MISSING");
  const prepared: Record<string, string> = {};
  for (const key of NATIVE_CAPSULE_KEYS)
    if (Object.hasOwn(environment, key)) prepared[key] = environment[key] as string;
  prepared.FVOCI_COLLAB_ENGINE = BINARY_DST + "/collab-engine";
  prepared.FVOCI_STATIC_DIR = DIST_DST;
  prepared.FVOCI_STORAGE_DIR = storage;
  const capsule = writeReadonlyCapsule(directory, prepared);
  const launcher = writeLauncher(directory);
  let containerId: string | null = null;
  try {
    const [statFifo, goFifo] = preparePrivateFifos(directory);
    const binary = get(manifest, "binaries", binaryName, "path") as string;
    const mounts = [
      mountSpec(launcher, LAUNCHER_DST, true),
      mountSpec(capsule, CAPSULE_DST, true),
      mountSpec(statFifo, STAT_FIFO_DST, false),
      mountSpec(goFifo, GO_FIFO_DST, false),
      mountSpec(dirname(realpathSync(binary)), BINARY_DST, true),
      mountSpec(join(checkout, "apps/web/dist"), DIST_DST, true),
      mountSpec(storage, storage, false),
    ];
    const argv = containerArgv(
      "fvoci-tui-" + token(6),
      launcher,
      capsule,
      BINARY_DST + "/" + binaryName,
      mounts,
      command,
      grant,
    );
    admitPublishedConfig(argv, [], secretValues);
    require(scope !== null, "UI_OWNED_PROCESS_SCOPE_REQUIRED");
    containerId = new TextDecoder().decode(await docker.client(scope, argv, 10)).trim();
    require(/^[0-9a-f]{64}$/.test(containerId), "UI_DOCKER_CLIENT_FAILED");
    const inspected = parseBytes(
      await docker.client(scope, ["docker", "inspect", containerId], 10),
    );
    const creation = creationIdentity(get(inspected, 0), argv, secretValues, kind);
    require(creation.qualification === BLOCKED &&
      get(grant, "canonicalRuntime", "imageId") ===
        creation.image, "UI_DAEMON_OBSERVATION_UNSUPPORTED");
    scope.io.write(join(directory, "creation-identity.private.json"), creation);
    return [containerId, creation, grant];
  } catch (original) {
    await releaseFailedPublish(scope as Scope, directory, containerId, original);
    throw original as Error;
  }
}

export async function localFixture(
  scope: Scope,
  manifest: Pick<Manifest, "binaries">,
  mode: string,
  environment: Record<string, string>,
  input?: Record_,
  proc: DaemonProc = linuxProc,
): Promise<Record_> {
  const directory = join(scope.io.root(), "local-fixture-" + mode);
  mkdirSync(directory, 0o700);
  const [containerId, creation, grant] = await publishContainer(
    scope,
    directory,
    environment,
    manifest,
    "fvoci-e2e-fixture",
    [mode],
    "fixture",
  );
  let child: Child | null = null,
    goFd: number | null = null,
    original: unknown = null,
    value: Record_ | undefined;
  const receiptErrors: string[] = [];
  try {
    require(creation.qualification === BLOCKED &&
      creation.liveDaemon === ONE_SHOT_LIVE &&
      creation.cgroupCaps === "not-observed", "UI_DAEMON_OBSERVATION_UNSUPPORTED");
    const deadline = budgetDeadline(FIXTURE_BUDGET);
    child = scope.spawn(
      ["docker", "start", "--attach", "--interactive", containerId],
      "fixture-" + mode,
      { env: cleanEnv(), stdin: "pipe", stdout: "pipe", stderr: "ignore" },
    );
    const [sample, fd] = await releaseAttached(
      scope,
      directory,
      containerId,
      child,
      deadline,
      grant,
      proc,
    );
    goFd = fd;
    const payload = fixtureInput(input);
    const stdout = await communicate(child, payload, budgetRemain(deadline));
    const state = await readDaemonExit(scope, containerId, 10);
    require(stdout.length < 64 * 1024 * 1024, "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
    const parsed = parsePlain(stdout);
    require(isRecord(parsed), "UI_NATIVE_FIXTURE_OUTPUT_REFUSED");
    const nativeExit = isRecord(state) ? state.ExitCode : null;
    const failure = nativeFailureCode(parsed, nativeExit);
    if (failure !== null) {
      try {
        scope.io.write(join(directory, "fixture-failure-" + token(6) + ".private.json"), {
          mode,
          exit: nativeExit,
          receipt: parsed,
        });
      } catch {
        receiptErrors.push("UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED");
      }
      throw new UiError(failure);
    }
    const proof: Record_ = await proveNormalDaemon(scope, child, sample, state, false);
    require(get(parsed, "lifecycleDrain") === "confirmed" &&
      get(parsed, "leases") === 0 &&
      get(parsed, "serverCloseReceipt") === "not-exposed-by-sdk", "UI_NATIVE_DRAIN_FAILED");
    proof.body = parsed;
    require(proof.qualification === "retired", "UI_DAEMON_OBSERVATION_UNSUPPORTED");
    scope.io.write(join(directory, "daemon-completion.private.json"), proof);
    value = parsed;
  } catch (error) {
    original = error;
  }
  const errors = await cleanupOwned(scope, containerId, child, directory, [goFd]);
  errors.push(...receiptErrors);
  reportCleanup(scope, original, errors);
  if (original !== null) throw original as Error;
  if (errors.length) throw new UiError("UI_PROCESS_CLOSURE_FAILED");
  return value as Record_;
}

export async function stopAttachedDaemon(
  scope: Scope,
  server: ServerHandle,
  base: string,
  directory: string,
): Promise<Record_> {
  let original: unknown = null,
    stopped: Record_ | undefined;
  try {
    const host = server.hostIdentity;
    require(isRecord(host) && host.pid !== server.child.pid, "UI_DAEMON_PID_REFUSED");
    const entry = scope.entries.get(String(host.pid) + ":" + host.startTicks);
    if (entry === undefined) throw new TypeError("uncaptured daemon");
    let forced = false;
    if (!scope.retired(host)) scope.send(entry, SIGTERM);
    let state = await readDaemonExit(scope, server.containerId, 10);
    if (state === null || state.Pid !== 0) {
      forced = true;
      if (!scope.retired(host)) scope.send(entry, SIGKILL);
      state = await readDaemonExit(scope, server.containerId, 10);
      if (state === null || state.Pid !== 0) {
        const id = server.containerId as string;
        await docker.client(scope, ["docker", "stop", "-t", "10", id], 10);
        const inspected = await inspectState(scope, id, 10);
        state = {
          Pid: inspected.Pid,
          ExitCode: inspected.ExitCode,
          OOMKilled: inspected.OOMKilled,
        };
      }
    }
    const proof = await proveNormalDaemon(
      scope,
      server.child,
      server.daemonSample as Bound,
      state,
      forced,
    );
    require(await portClosed(base), "UI_SERVER_CLOSURE_FAILED");
    stopped = {
      serverExit: proof.productExit,
      clientExit: proof.clientExit,
      qualification: proof.qualification,
      portClosed: true,
      recordedIdentitiesRetired: true,
      forced: false,
      caps: proof.caps,
      uid: proof.uid,
      gid: proof.gid,
    };
    const rows = [...scope.entries.values()]
      .filter((item) => item.allocation === proof.allocation)
      .map((item) => item.identity);
    scope.io.write(join(directory, "server-identities-" + token(6) + ".private.json"), {
      stopped,
      identities: rows,
    });
  } catch (error) {
    original = error;
  }
  const errors = await cleanupOwned(scope, server.containerId, server.child, directory, [
    server.goFd,
  ]);
  server.goFd = null;
  reportCleanup(scope, original, errors);
  if (original !== null) throw original as Error;
  if (errors.length) throw new UiError("UI_PROCESS_CLOSURE_FAILED");
  return stopped as Record_;
}

export interface LocalStartSteps {
  publish: typeof publishContainer;
  release: typeof releaseAttached;
  cleanup: typeof cleanupOwned;
  clock: () => number;
}
const localStartSteps: LocalStartSteps = {
  publish: publishContainer,
  release: releaseAttached,
  cleanup: cleanupOwned,
  clock: now,
};

export async function localServerStart(
  scope: Scope,
  manifest: Pick<Manifest, "binaries">,
  environment: Record<string, string>,
  directory: string,
  expectedSetup = false,
  steps: LocalStartSteps = localStartSteps,
): Promise<Started> {
  const [containerId, creation, grant] = await steps.publish(
    scope,
    directory,
    environment,
    manifest,
    "fvoci-migrate",
    ["--start"],
    "server",
  );
  let log: number | null = null,
    server: ServerHandle | null = null,
    goFd: number | null = null,
    started: number | null = null,
    deadline: number | null = null,
    logpath: string | null = null;
  try {
    require(creation.qualification === BLOCKED &&
      creation.cgroupCaps === "not-observed", "UI_DAEMON_OBSERVATION_UNSUPPORTED");
    logpath = join(directory, "server.private.log");
    log = openSync(logpath, "wx", 0o600);
    fchmodSync(log, 0o600);
    deadline = budgetDeadline(SERVER_BUDGET, steps.clock);
    started = deadline - SERVER_BUDGET;
    const child = scope.spawn(["docker", "start", "--attach", containerId], "server", {
      env: cleanEnv(),
      stdin: "ignore",
      stdout: log,
      stderr: log,
    });
    server = { child, containerId };
    const [sample, fd] = await steps.release(scope, directory, containerId, child, deadline, grant);
    goFd = fd;
    server.goFd = goFd;
    goFd = null;
    server.daemonSample = sample;
    server.maintenanceIdentity = sample.maintenance;
    server.hostIdentity = sample.retirement;
    const base = await awaitListening(logpath, child, deadline, steps.clock);
    await requireSetup(base, expectedSetup);
    return { server, base, log };
  } catch (original) {
    const diagnostic =
      failureCode(original) === "UI_SERVER_START_FAILED"
        ? publishStartDiagnostic(
            scope,
            server?.child ?? null,
            logpath,
            started,
            deadline,
            steps.clock,
          )
        : null;
    const errors = await steps.cleanup(scope, containerId, server?.child ?? null, directory, [
      goFd,
      server?.goFd,
    ]);
    if (server !== null) server.goFd = null;
    try {
      if (log !== null) closeSync(log);
    } catch {
      errors.push("UI_SERVER_LOG_CLOSE_FAILED");
    }
    if (diagnostic !== null) {
      try {
        scope.io.write(join(directory, "start-failure.private.json"), {
          originalFailure: failureCode(original),
          cleanupErrors: errors,
          startDiagnostic: diagnostic,
        });
      } catch {
        errors.push("UI_PROCESS_RECEIPT_WRITE_FAILED");
      }
    }
    reportCleanup(scope, original, errors);
    throw original as Error;
  }
}
