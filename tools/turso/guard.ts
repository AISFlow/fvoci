#!/usr/bin/env bun
// Trusted manual admission and one explicitly selected real primary consumer.
// Only --admit fetches public Environment metadata; only --consume reads the
// two runtime credential variables. Configuration admission is not proof of
// server identity or later CRUD support.
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, sep } from "node:path";
import {
  AdmissionError,
  contextFrom,
  decide,
  diagnosticChildEnv,
  DIAGNOSTIC_UNIT_NAME,
  eventInputs,
  parseMode,
  phaseOf,
  primaryChildEnv,
  primaryTestName,
  reject,
  requireImplemented,
  validateEnvironment,
  validateTarget,
  validateUiInputs,
  type Env,
  type Inputs,
  type Phase,
} from "./guard-policy.ts";
import {
  connectionResult,
  diagnosticListingSelected,
  diagnosticUnitResult,
  inventoryResult,
  migrationResult,
  resetResult,
  type Verdict,
} from "./guard-receipts.ts";
import * as io from "./guard-io.ts";
import { pySplitlines as splitlines, truthy } from "../web-e2e/compat.ts";
import { dumps, isRecord, jsonEqual, type Json } from "./python-compat.ts";

/** The two effects the freeze-bound steps need replaced in tests. */
export interface Seams {
  sourceDigest: () => string;
  runChild: io.RunChild;
}
const realSeams: Seams = { sourceDigest: io.sourceDigest, runChild: io.runChild };

const BINDING_FAILED = "COMPILED_TEST_BINDING_FAILED";

function runnerTemp(env: Env): string {
  if (env.RUNNER_TEMP === undefined) throw new TypeError("RUNNER_TEMP unset");
  return io.resolveLoose(env.RUNNER_TEMP);
}

function isSymlink(path: string): boolean {
  try {
    return lstatSync(path).isSymbolicLink();
  } catch {
    return false;
  }
}

function within(path: string, directory: string): boolean {
  return path === directory || path.startsWith(directory + sep);
}

// Strict UTF-8 that keeps a BOM, so a BOM or invalid byte fails the parse.
function readText(path: string): string {
  return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(readFileSync(path));
}

function readJson(path: string): unknown {
  return JSON.parse(readText(path));
}

function get(value: unknown, key: string, fallback?: Json): unknown {
  if (!isRecord(value)) throw new TypeError("not an object");
  return Object.hasOwn(value, key) ? value[key] : fallback;
}

/** Binds the one fresh db-tests lib test binary and its inputs by digest. */
export function freezeCompiledTest(
  checkoutSha: string,
  env: Env,
  seams: Pick<Seams, "sourceDigest"> = realSeams,
): void {
  const root = runnerTemp(env);
  const target = join(root, "turso-target");
  const artifactFile = join(root, "turso-compile.json");
  const artifacts = splitlines(readText(artifactFile)).map((line) => JSON.parse(line) as unknown);
  const source = join(process.cwd(), "src", "lib.rs");
  const matches = artifacts.filter((artifact) => {
    if (get(artifact, "reason") !== "compiler-artifact") return false;
    const kind = get(get(artifact, "target", {}), "kind");
    if (!jsonEqual(kind, ["lib"])) return false;
    if (get(get(artifact, "target", {}), "name") !== "fvoci_server") return false;
    if (get(get(artifact, "target", {}), "src_path") !== source) return false;
    if (get(get(artifact, "profile", {}), "test") !== true) return false;
    return (
      jsonEqual(get(artifact, "features"), ["db-tests"]) && truthy(get(artifact, "executable"))
    );
  });
  if (
    matches.length !== 1 ||
    !artifacts.some(
      (artifact) =>
        get(artifact, "reason") === "build-finished" && get(artifact, "success") === true,
    )
  ) {
    reject(BINDING_FAILED);
  }
  const artifact = matches[0] as Record<string, Json>;
  if (typeof artifact.executable !== "string") throw new TypeError("executable is not a path");
  const executable = artifact.executable;
  if (
    isSymlink(executable) ||
    !within(io.resolveLoose(executable), join(target, "debug", "deps"))
  ) {
    reject(BINDING_FAILED);
  }
  if (!io.startsWithElf(executable)) reject(BINDING_FAILED);
  const frozen = join(root, "turso-connection-libtest");
  if (existsSync(frozen)) reject(BINDING_FAILED);
  copyFileSync(executable, frozen);
  chmodSync(frozen, 0o700);
  const manifest: Record<string, Json> = {
    sha: checkoutSha,
    source_digest: seams.sourceDigest(),
    binary_sha256: io.fileDigest(frozen),
    cargo_output_sha256: io.fileDigest(artifactFile),
    native_input_sha256: io.fileDigest(join(root, "fvoci-sqlite", "consumer-inputs.json")),
    artifact,
  };
  writeFileSync(join(root, "turso-connection-build.json"), dumps(manifest));
}

/** Revalidates the complete frozen receipt; compared before/after each child. */
export function frozenBinding(
  checkoutSha: string,
  env: Env,
  seams: Pick<Seams, "sourceDigest"> = realSeams,
): { executable: string; key: string } {
  const root = runnerTemp(env);
  const manifestPath = join(root, "turso-connection-build.json");
  const manifest = readJson(manifestPath);
  const executable = join(root, "turso-connection-libtest");
  const source = seams.sourceDigest();
  const binary = io.fileDigest(executable);
  const native = io.fileDigest(join(root, "fvoci-sqlite", "consumer-inputs.json"));
  const cargoOutput = io.fileDigest(join(root, "turso-compile.json"));
  if (
    get(manifest, "sha") !== checkoutSha ||
    get(manifest, "source_digest") !== source ||
    isSymlink(executable) ||
    get(manifest, "binary_sha256") !== binary ||
    get(manifest, "native_input_sha256") !== native ||
    get(manifest, "cargo_output_sha256") !== cargoOutput
  ) {
    reject(BINDING_FAILED);
  }
  if (!io.startsWithElf(executable)) reject(BINDING_FAILED);
  return {
    executable,
    key: [source, binary, native, cargoOutput, io.fileDigest(manifestPath)].join(" "),
  };
}

/** The exact credential-free unit; it receives no DB/provider/GitHub value or selector. */
export async function runDiagnosticUnit(
  checkoutSha: string,
  env: Env,
  seams: Seams = realSeams,
): Promise<Verdict> {
  const binding = frozenBinding(checkoutSha, env, seams);
  const childEnv = diagnosticChildEnv(env);
  const listed = await seams.runChild(
    [binding.executable, DIAGNOSTIC_UNIT_NAME, "--list", "--exact"],
    childEnv,
  );
  if (!diagnosticListingSelected(listed.status, listed.output))
    reject("TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED");
  if (frozenBinding(checkoutSha, env, seams).key !== binding.key) reject(BINDING_FAILED);
  const result = await seams.runChild(
    [binding.executable, DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"],
    childEnv,
  );
  if (frozenBinding(checkoutSha, env, seams).key !== binding.key) reject(BINDING_FAILED);
  return diagnosticUnitResult(result.status, result.output);
}

/** One real primary child for the routed phase; never retried. */
export async function runPrimary(
  expected: Phase,
  checkoutSha: string,
  inputs: Inputs,
  env: Env,
  seams: Seams = realSeams,
): Promise<Verdict> {
  if (phaseOf(inputs) !== expected) reject("WRONG_CONSUMER_PHASE");
  const phase = expected;
  requireImplemented(phase);
  if (env.FVOCI_DATABASE_BACKEND !== "libsql-remote") reject("BACKEND_SELECTOR_REQUIRED");
  // The non-UI consume step publishes the test pair, not product FVOCI_LIBSQL_*.
  const url = env.FVOCI_TEST_TURSO_DATABASE_URL ?? "";
  const token = env.FVOCI_TEST_TURSO_AUTH_TOKEN ?? "";
  validateTarget(
    inputs,
    { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: env.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE ?? "" },
    { FVOCI_TEST_TURSO_DATABASE_URL: url, FVOCI_TEST_TURSO_AUTH_TOKEN: token },
  );
  // Inventory/reset bind the full freeze receipt before/after their one child.
  const whole = phase === "inventory" || phase === "reset";
  const binding = whole ? frozenBinding(checkoutSha, env, seams) : null;
  const root = runnerTemp(env);
  const manifest = readJson(join(root, "turso-connection-build.json"));
  const executable = join(root, "turso-connection-libtest");
  if (
    get(manifest, "sha") !== checkoutSha ||
    get(manifest, "source_digest") !== seams.sourceDigest() ||
    isSymlink(executable) ||
    get(manifest, "binary_sha256") !== io.fileDigest(executable) ||
    get(manifest, "native_input_sha256") !==
      io.fileDigest(join(root, "fvoci-sqlite", "consumer-inputs.json"))
  ) {
    reject(BINDING_FAILED);
  }
  if (!io.startsWithElf(executable)) reject(BINDING_FAILED);
  // Raw SDK/test errors can contain endpoint/query/token values: captured in
  // memory only, never written, uploaded or reflected. No retry.
  const result = await seams.runChild(
    [executable, primaryTestName(phase), "--ignored", "--exact", "--test-threads=1", "--nocapture"],
    primaryChildEnv(phase, env, url, token),
  );
  if (binding && frozenBinding(checkoutSha, env, seams).key !== binding.key) reject(BINDING_FAILED);
  if (phase === "reset") return resetResult(result.status, result.output);
  if (phase === "inventory") return inventoryResult(result.status, result.output);
  if (phase === "migration") return migrationResult(result.status, result.output);
  return connectionResult(result.status, result.output);
}

export type UiConsume = (phase: "ui-baseline" | "ui-ack", inputs: Inputs) => Promise<void>;

async function loadUiConsume(): Promise<UiConsume> {
  // Loaded only for UI phases so the non-UI path never depends on it.
  const module: string = "./ui.ts";
  const ui = (await import(module)) as { consume: UiConsume };
  return ui.consume;
}

export async function runUi(
  phase: "ui-baseline" | "ui-ack",
  checkoutSha: string,
  inputs: Inputs,
  env: Env,
  consume: () => Promise<UiConsume> = loadUiConsume,
): Promise<void> {
  if (env.FVOCI_DATABASE_BACKEND !== "libsql-remote") reject("BACKEND_SELECTOR_REQUIRED");
  validateTarget(
    inputs,
    { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: env.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE ?? "" },
    {
      FVOCI_TEST_TURSO_DATABASE_URL: env.FVOCI_LIBSQL_URL ?? "",
      FVOCI_TEST_TURSO_AUTH_TOKEN: env.FVOCI_LIBSQL_AUTH_TOKEN ?? "",
    },
  );
  validateUiInputs(phase, inputs, checkoutSha);
  const run = await consume();
  try {
    await run(phase, inputs);
  } catch (error) {
    if (error instanceof Error && error.name === "UiError") reject(error.message);
    throw error;
  }
}

export interface MainEffects {
  checkoutSha: () => string;
  fetchEnvironment: () => Promise<io.EnvironmentMetadata>;
  uiConsume?: () => Promise<UiConsume>;
  seams?: Seams;
}
const realEffects: MainEffects = {
  checkoutSha: io.checkoutSha,
  fetchEnvironment: io.fetchEnvironmentMetadata,
};

/** Writes fixed codes only; returns the process exit status. */
export async function main(
  argv: readonly string[],
  env: Env,
  effects: MainEffects = realEffects,
): Promise<number> {
  const out = (line: string) => process.stdout.write(line + "\n");
  try {
    const mode = parseMode(argv);
    if (env.GITHUB_EVENT_PATH === undefined) throw new TypeError("GITHUB_EVENT_PATH unset");
    const inputs = eventInputs(readJson(env.GITHUB_EVENT_PATH));
    const sha = effects.checkoutSha();
    const action = decide(mode, contextFrom(env), inputs, sha);
    let verdict: Verdict | null = null;
    if (action.kind === "bootstrap") {
      out("BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN");
    } else if (action.kind === "admit") {
      let metadata: io.EnvironmentMetadata;
      try {
        metadata = await effects.fetchEnvironment();
      } catch {
        reject("ENVIRONMENT_METADATA_UNAVAILABLE");
      }
      const id = validateEnvironment(metadata.value, metadata.idText);
      if (env.GITHUB_OUTPUT === undefined) throw new TypeError("GITHUB_OUTPUT unset");
      appendFileSync(env.GITHUB_OUTPUT, "environment_id=" + id + "\n");
      out("ENVIRONMENT_ADMISSION_OK_RUNTIME_NOT_RUN");
    } else if (action.kind === "freeze") {
      freezeCompiledTest(sha, env, effects.seams);
      out("COMPILED_TEST_FROZEN_RUNTIME_NOT_RUN");
    } else if (action.kind === "diagnostic-unit") {
      verdict = await runDiagnosticUnit(sha, env, effects.seams);
    } else if (action.phase === "ui-baseline" || action.phase === "ui-ack") {
      await runUi(action.phase, sha, inputs, env, effects.uiConsume);
    } else {
      verdict = await runPrimary(action.phase, sha, inputs, env, effects.seams);
    }
    if (verdict) {
      for (const line of verdict.lines) out(line);
      if (verdict.code) reject(verdict.code);
    }
    return 0;
  } catch (error) {
    // No stack or message: even parsing errors must not reflect hostile inputs.
    process.stderr.write(
      (error instanceof AdmissionError ? error.message : "ADMISSION_FAILED") + "\n",
    );
    return 78;
  }
}

if (import.meta.main) {
  process.exitCode = await main(process.argv.slice(2), process.env);
}
