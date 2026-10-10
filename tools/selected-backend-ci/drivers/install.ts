// Root-owned, one-shot current install lane driver: one owned network-less
// Ubuntu container runs the current selected_install_lifetime test executable
// against the current migrate/server/engine cohort. No browser, PostgreSQL,
// Turso or OFF flow. Prints only its final JSON summary on stdout and exits
// with the lane's first failure code.
import { strict as assert } from "node:assert";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, statfsSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { env, read, root, sha } from "../io.ts";
import type { Environment } from "../io.ts";
import type { Inputs } from "../types.ts";
import { loadCurrent, type Current } from "./binding.ts";
import {
  assertNoEnvFile,
  cleanupAttempt,
  cleanupScope,
  command,
  emit,
  failureCheckpoint,
  failureDigest,
  list,
  now,
  readText,
  runtimeError,
  trapInterrupts,
  writeJson,
  type Command,
  type Receipt,
} from "./common.ts";

export const installDriver = import.meta.path;
export const image =
  "ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7";
export const expectedRustTests = 4;
export const expectedProcessReceipts = 15;

// The I/O this driver owns, replaceable as one boundary in tests.
export interface Seam {
  trap: () => void;
  loadCurrent: (lane: "install", driver: string) => Promise<Current>;
  command: Command;
  diskFree: (path: string) => number;
  emit: (line: string) => void;
}
export interface State {
  current: Current;
  run: string;
  name: string;
  before: Inputs;
  server: string;
  migrate: string;
  engine: string;
  test: string;
  receipt: Receipt;
  source: Environment;
  created: boolean;
  code: number;
}

// ---- pure decisions -------------------------------------------------------

// Only synthetic run-local keyrings; the values stay in a mode 0600 input.
export function installEnvironment(hex: () => string): Record<string, string> {
  return {
    PATH: "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    PASSWORD_PEPPER_KEYS: JSON.stringify({ fixture: hex() }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "fixture",
    ENCRYPTION_KEYS: JSON.stringify({ fixture: hex() }),
    ENCRYPTION_ACTIVE_KEY_ID: "fixture",
    FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:8080",
    FVOCI_COOKIE_SECURE: "0",
    STORAGE_DRIVER: "local",
    FVOCI_STORAGE_DIR: "/fvoci/storage",
    FVOCI_COLLAB_ENGINE: "/fvoci/bin/collab-engine",
    FVOCI_COLLAB_FAMILY_LEASE_MS: "30000",
    FVOCI_COLLAB_FAMILY_RENEW_MS: "5000",
    FVOCI_STATIC_DIR: "/srv/fvoci-web",
  };
}
export const completeTestRun = (log: string) =>
  /test result: ok\. 4 passed; 0 failed; 0 ignored;/.test(log);
// Every owned child receipt has an observed, non-null status.
export function assertProcessReceipts(records: readonly unknown[]): void {
  assert.equal(records.length, expectedProcessReceipts, "actual child receipt count");
  for (const record of records)
    assert.ok(
      record !== null &&
        typeof record === "object" &&
        "status" in record &&
        record.status !== null &&
        record.status !== undefined,
    );
}
export const positiveDockerAbsence = (result: { returncode: number; stderr: string }) =>
  result.returncode !== 0 &&
  ["no such object", "no such container"].some((marker) =>
    result.stderr.toLowerCase().includes(marker),
  );
const summaryKeys = [
  "source",
  "final_exit_code",
  "actual_tests",
  "actual_owned_process_receipts",
  "owned_container_absent",
  "body_seconds",
  "failed_phase",
  "observed_failed_exit",
  "original_body_log_sha256",
  "original_failure_checkpoint_sha256",
];
// Public stdout line: fixed facts and digests only, never the private failure.
export function summary(receipt: Receipt, code: number, cleanupErrors: unknown[]): Receipt {
  const result: Receipt = Object.fromEntries(summaryKeys.map((key) => [key, receipt[key] ?? null]));
  const original = receipt.original_driver_failure;
  return Object.assign(result, {
    failure_code: code ? "SELECTED_INSTALL_DRIVER_FAILED" : null,
    final_exit_code: code,
    cleanup_failure_codes: cleanupErrors,
    original_driver_failure_sha256: original === undefined ? null : failureDigest(original),
  });
}

// ---- owned I/O ------------------------------------------------------------

// rglob('*process.json') without following directory links; a missing
// retained copy has no receipts, so the count assertion reports it.
export function processReceipts(directory: string): string[] {
  const result: string[] = [];
  if (!existsSync(directory)) return result;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.name.endsWith("process.json")) result.push(path);
    if (entry.isDirectory()) result.push(...processReceipts(path));
  }
  return result;
}
// shutil.disk_usage(path).free
export function diskFree(path: string): number {
  const facts = statfsSync(path);
  return facts.bavail * facts.frsize;
}

export async function body(state: State, seam: Seam): Promise<void> {
  const { current, run, name, receipt, server, migrate, engine, test } = state;
  const required = async (args: string[], log?: string) => {
    const result = await seam.command(args, { log, required: false });
    if (result.returncode)
      throw runtimeError(
        `owned prerequisite failed code=${String(result.returncode)}: ${JSON.stringify(args.slice(0, 3))}`,
      );
    return result;
  };
  const binaries = current.build.binaries;
  const owner = env("FVOCI_CI_OWNER", state.source);
  // prettier-ignore
  await required(["docker", "create", "--name", name, "--network", "none", "--user", "0",
    "--label", "fvoci.owner=" + owner, "--label", "fvoci.test-run=v060-current-install",
    "--entrypoint", "/bin/sleep", image, "1800"]);
  state.created = true;
  await required(["docker", "start", name]);
  writeJson(join(run, "runtime-abi-inputs.json"), current.abi);
  receipt.runtime_abi = "Ubuntu 26.04 image libraries; host ABI hashes are provenance only";
  // prettier-ignore
  await required(["docker", "exec", name, "/bin/sh", "-ec", "ldd --version | head -1; /bin/true"],
    join(run, "actual-runtime-abi.log"));
  const setup =
    "mkdir -p " +
    dirname(server) +
    " /fvoci/bin /fvoci/run /fvoci/inputs /fvoci/storage /srv/fvoci-web; " +
    "chown 0:1000 /fvoci/run; chmod 0710 /fvoci/run; " +
    "chown 1000:1000 /fvoci/storage; chmod 0700 /fvoci/storage; chmod 0700 /fvoci/inputs";
  await required(["docker", "exec", name, "/bin/sh", "-ec", setup]);
  const privateInput = join(run, "environment.private.json");
  for (const [source, destination] of [
    [server, server],
    [migrate, migrate],
    [engine, "/fvoci/bin/collab-engine"],
    [test, "/fvoci/bin/install-test"],
    [privateInput, "/fvoci/inputs/environment.json"],
  ] as const)
    await required(["docker", "cp", source, name + ":" + destination]);
  await required(["docker", "cp", join(root, "apps/web/dist") + "/.", name + ":/srv/fvoci-web"]);
  const copied = [
    server,
    migrate,
    "/fvoci/bin/collab-engine",
    "/fvoci/bin/install-test",
    "/fvoci/inputs/environment.json",
  ];
  const executables = copied.slice(0, -1);
  // prettier-ignore
  await required(["docker", "exec", name, "stat", "-c", "%n %u %g %a", ...copied],
    join(run, "copied-files-before.log"));
  await required(["docker", "exec", name, "chown", "0:0", ...copied]);
  await required(["docker", "exec", name, "chmod", "0755", ...executables]);
  // prettier-ignore
  const ldd = (await required(["docker", "exec", name, "/bin/sh", "-ec",
    '. /etc/os-release; test "$ID" = ubuntu; test "$VERSION_ID" = 26.04; for binary do ldd "$binary"; done',
    "fvoci-runtime-abi", ...executables])).stdout;
  writeFileSync(join(run, "native-runtime-abi.log"), ldd);
  assert.ok(!ldd.includes("not found"), "Ubuntu26 runtime ELF dependencies missing");
  await required(["docker", "exec", name, "chmod", "0600", "/fvoci/inputs/environment.json"]);
  await required(["docker", "exec", name, "chown", "-R", "0:0", "/srv/fvoci-web"]);
  // prettier-ignore
  await required(["docker", "exec", name, "stat", "-c", "%n %u %g %a", ...copied],
    join(run, "copied-files-after.log"));
  // prettier-ignore
  await required(["docker", "exec", name, "sha256sum", ...executables],
    join(run, "copied-executable-hashes.log"));
  receipt.copied_inode_ownership_correction =
    "Only exact newly copied own container files; original host binaries untouched; actual before/after UID GID mode logs retained";
  // prettier-ignore
  await required(["docker", "exec", name, "/bin/sh", "-ec",
    "chmod 0600 /fvoci/inputs/environment.json; id; ldd --version | head -1; " +
    "command -v kill; test ! -e /usr/bin/node; test ! -e /usr/bin/bun"], join(run, "prerequisite.log"));
  // prettier-ignore
  const args = ["docker", "exec",
    "--env", "FVOCI_SELECTED_INSTALL_RUN_ROOT=/fvoci/run",
    "--env", "FVOCI_SELECTED_INSTALL_ENV_FILE=/fvoci/inputs/environment.json",
    "--env", "FVOCI_SELECTED_INSTALL_MIGRATE_SHA256=" + String(binaries[migrate]?.sha256),
    "--env", "FVOCI_SELECTED_INSTALL_SERVER_SHA256=" + String(binaries[server]?.sha256),
    name, "/fvoci/bin/install-test", "--test-threads=1", "--nocapture"];
  receipt.phase = "install-body";
  Object.assign(receipt, { command: args, body_start_utc: now() });
  writeJson(join(run, "progress.json"), receipt);
  const started = performance.now();
  const log = join(run, "test.log");
  const result = await seam.command(args, { log, required: false });
  state.code = result.returncode;
  if (state.code !== 0) failureCheckpoint(receipt, run, state.code, { bodyLog: log });
  Object.assign(receipt, {
    body_end_utc: now(),
    exit_code: state.code,
    body_seconds: (performance.now() - started) / 1000,
    log_sha256: sha(log),
  });
  // prettier-ignore
  const copy = await seam.command(["docker", "cp", name + ":/fvoci/run", join(run, "retained-run")],
    { required: false });
  receipt.durable_receipt_copy_exit = copy.returncode;
  const raw = readText(log);
  if (state.code !== 0) return;
  assert.ok(completeTestRun(raw), "exact executed counts");
  assertProcessReceipts(processReceipts(join(run, "retained-run")).map((path) => read(path)));
  Object.assign(receipt, {
    actual_tests: expectedRustTests,
    ignored: 0,
    actual_owned_process_receipts: expectedProcessReceipts,
  });
}

// The original packet already exists; every removal and observation below is
// attempted and recorded, and none replaces the first outcome.
export function finalize(state: State, seam: Seam): Promise<number> {
  return cleanupScope(async () => {
    const { receipt, name, run, before, current } = state;
    const cleanupErrors: unknown[] = [...list(receipt, "diagnostic_errors")];
    const attempt = <T>(label: string, operation: () => T | Promise<T>) =>
      cleanupAttempt(receipt, cleanupErrors, label, operation);
    const exec = (args: string[]) => seam.command(args, { required: false });
    if (state.created) {
      const retained = join(run, "retained-run");
      const copied = await attempt("owned-receipt-copy-failed", async () =>
        existsSync(retained)
          ? 0
          : (await exec(["docker", "cp", name + ":/fvoci/run", retained])).returncode,
      );
      if (copied !== 0) cleanupErrors.push("owned-receipt-copy-unconfirmed");
      const remove = () => exec(["docker", "rm", "-f", "-v", name]);
      let removed = await attempt("owned-container-removal-failed", remove);
      removed ??= await attempt("exceptional-owned-removal-failed", remove);
      receipt.owned_container_cleanup_exit = removed?.returncode ?? null;
      const absent = await attempt("owned-container-absence-failed", () =>
        exec(["docker", "inspect", name]),
      );
      if (absent !== null) receipt.owned_container_absent = positiveDockerAbsence(absent);
      if (removed === null || removed.returncode || receipt.owned_container_absent !== true)
        cleanupErrors.push("owned-container-cleanup-unconfirmed");
    }
    const binaries = current.build.binaries;
    const unchanged = await attempt("install-post-input-observation-failed", () => {
      const changedInputs = [
        ...Object.entries(before.tracked)
          .filter(([path, digest]) => sha(join(root, path)) !== digest)
          .map(([path]) => path),
        ...Object.entries(before.external)
          .filter(([path, digest]) => sha(path) !== digest)
          .map(([path]) => path),
      ];
      const changedBinaries = Object.keys(binaries).filter(
        (path) => sha(path) !== binaries[path]?.sha256,
      );
      Object.assign(receipt, {
        full_current_source_external_unchanged: !changedInputs.length,
        actual_binary_hashes_unchanged: !changedBinaries.length,
        changed_inputs: changedInputs,
        changed_binaries: changedBinaries,
      });
      return !changedInputs.length && !changedBinaries.length;
    });
    if (unchanged !== true) {
      Object.assign(receipt, {
        full_current_source_external_unchanged: false,
        actual_binary_hashes_unchanged: false,
      });
      cleanupErrors.push("install-post-input-unconfirmed");
    }
    receipt.end_utc = await attempt("end-clock-observation-failed", now);
    receipt.free_after = await attempt("final-disk-observation-failed", () => seam.diskFree(run));
    receipt.final_source = await attempt("final-source-observation-failed", async () =>
      (
        await seam.command(["git", "-c", "safe.directory=" + root, "-C", root, "rev-parse", "HEAD"])
      ).stdout.trim(),
    );
    let code = state.code;
    if (cleanupErrors.length) code ||= 1;
    Object.assign(receipt, { final_exit_code: code, cleanup_errors: cleanupErrors });
    await attempt("final-receipt-write-failed", () => {
      writeJson(join(run, "receipt.json"), receipt);
    });
    if (cleanupErrors.length) code ||= 1;
    try {
      seam.emit(JSON.stringify(summary(receipt, code, cleanupErrors)));
    } catch {
      code ||= 1;
    }
    return code;
  });
}

export async function main(
  seam: Seam = installSeam,
  driver = installDriver,
  source: Environment = process.env,
): Promise<number> {
  const current = await seam.loadCurrent("install", driver);
  const run = current.run;
  assert.ok(!existsSync(run), "literal one-shot owned run; preserve original failures");
  mkdirSync(run, { mode: 0o700 });
  const binaries = current.build.binaries;
  const pick = (match: (path: string) => boolean) => {
    const path = Object.keys(binaries).find(match);
    assert.ok(path !== undefined, "missing current cohort executable");
    return path;
  };
  const server = pick((path) => path.endsWith("/fvoci-server")),
    migrate = pick((path) => path.endsWith("/fvoci-migrate")),
    engine = pick((path) => path.endsWith("/collab-engine")),
    test = pick((path) => binaries[path]?.target.name === "selected_install_lifetime");
  writeJson(join(run, "source-inputs-before.json"), current.before);
  const environment = installEnvironment(() => randomBytes(32).toString("hex"));
  const privateInput = join(run, "environment.private.json");
  writeFileSync(privateInput, JSON.stringify(environment), { flag: "wx", mode: 0o600 });
  const name = "fvoci-v060-install-current-" + randomBytes(4).toString("hex");
  const state: State = {
    current,
    run,
    name,
    before: current.before,
    server,
    migrate,
    engine,
    test,
    receipt: {},
    source,
    created: false,
    code: 1,
  };
  state.receipt = {
    source: current.manifest.source,
    tree: current.before.tree,
    start_utc: now(),
    pid: process.pid,
    image,
    container: name,
    root_owner: env("FVOCI_CI_OWNER", source),
    scope:
      "actual SQLite normal migrate --start process/install/lifetime controls, not browser/PG/Turso/OFF/fullCI",
    test_executable: { path: test, sha256: binaries[test]?.sha256 },
    actual_binary_inputs: binaries,
    environment_names: Object.keys(environment).sort(),
    environment_private_file: privateInput,
    original_failure:
      "root-d49-install-runtime/test.log ownership4FAIL and owned-copy/test.log ABI1PASS3FAIL preserved; historical copiedowner/ABI failures retained; this execution uses Ubuntu26 image libraries",
    expected_rust_tests: expectedRustTests,
    expected_owned_child_processes: expectedProcessReceipts,
    free_before: seam.diskFree(run),
    retry: 0,
  };
  writeJson(join(run, "start.json"), state.receipt);
  Object.assign(state.receipt, { phase: "container-prepare", owned_container_absent: null });
  seam.trap();
  try {
    await body(state, seam);
  } catch (error) {
    failureCheckpoint(state.receipt, run, state.receipt.exit_code ?? null, {
      error,
    });
    state.code ||= 1;
  }
  return finalize(state, seam);
}

export const installSeam: Seam = {
  trap: trapInterrupts,
  loadCurrent,
  command,
  diskFree,
  emit,
};

if (import.meta.main) {
  assertNoEnvFile();
  process.exit(await main());
}
