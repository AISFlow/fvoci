// Credential-free admission and one explicitly selected primary consumer.
// Only --admit reads public Environment metadata. Only --consume reads the
// two runtime credential variables. Fixed codes only; inputs are not echoed.
import { createHash } from "node:crypto";
import { appendFileSync, chmodSync, closeSync, copyFileSync, existsSync, lstatSync, openSync, readFileSync, readSync, realpathSync, writeFileSync } from "node:fs";
import { resolve, sep } from "node:path";

export class AdmissionError extends Error {
  constructor(code: string) {
    super(code);
    this.name = "AdmissionError";
  }
}

export function reject(code: string): never {
  throw new AdmissionError(code);
}

export const PHASES = [
  "connection",
  "crud",
  "transactions",
  "migration",
  "inventory",
  "reset",
  "persistence",
  "restore",
  "ui-ack",
  "ui-baseline",
] as const;

export const REPOSITORY = "AISFlow/fvoci";
export const ENVIRONMENT = "fvoci-turso-test";
export const REVIEWED_REF = "refs/heads/fvoci/v060-turso-verified-connection";
export const UI_REVIEWED_REF = "refs/heads/fvoci/v060-product-integration-20261005";
export const TEST_NAME = "db::turso_test::turso_primary_connection";
export const MIGRATION_TEST_NAME = "db::turso_test::turso_primary_current12_install_resume";
export const INVENTORY_TEST_NAME = "db::turso_test::turso_primary_migration_target_inventory";
export const RESET_TEST_NAME = "db::turso_test::turso_primary_disposable_prefix11_reset";
export const DIAGNOSTIC_UNIT_NAME = "db::turso_test::migration_diagnostics_disclose_only_known_static_failures";
export const API_ROOT = "https://api.github.com/repos/AISFlow/fvoci/environments/fvoci-turso-test";

export const RESET_PRIMARY_CODES = new Set([
  "OK", "BEGIN_FAILED", "WRONG_PRODUCT_BACKEND", "WRONG_BACKEND",
  "CURRENT_LINEAGE_CHANGED", "FK_QUERY_FAILED", "FK_DECODE_FAILED",
  "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED", "LITERAL_DECODE_FAILED",
  "LITERAL_MISMATCH", "RESET_SCHEMA_REFUSED", "RESET_DDL_FAILED",
  "RESET_BLANK_IN_WRITER_FAILED", "COMMIT_UNCONFIRMED", "RESET_FRESH_BLANK_FAILED",
]);

export const INVENTORY_PRIMARY_CODES = new Set([
  "BEGIN_FAILED", "WRONG_PRODUCT_BACKEND", "WRONG_BACKEND", "FK_QUERY_FAILED",
  "FK_DECODE_FAILED", "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED",
  "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH", "CURRENT_LINEAGE_CHANGED",
  "INVENTORY_QUERY_FAILED", "INVENTORY_DECODE_FAILED", "INVENTORY_PREFIX_REFUSED",
  "INVENTORY_SCHEMA_REFUSED", "INVENTORY_SNAPSHOT_MISMATCH", "INVENTORY_HASH_INVALID",
]);

export const MIGRATION_PRIMARY_CODES = new Set([
  "BEGIN_FAILED", "CLOSE_FAILED", "COMMIT_UNCONFIRMED", "CURRENT_APPLY_FAILED",
  "CURRENT_GATE_FAILED", "CURRENT_GATE_MISMATCH", "CURRENT_LINEAGE_CHANGED",
  "DATA_DECODE_FAILED", "DATA_QUERY_FAILED", "DATA_WRITE_FAILED", "DATA_WRITE_MISMATCH",
  "DDL_FAILED", "DEFER_PRAGMA_REFUSED", "FENCE_BASELINE_NOT_EMPTY", "FENCE_ROW_UNBOUND",
  "FENCE_WRITE_FAILED", "FK_DECODE_FAILED", "FK_FAILURE_MISSING", "FK_QUERY_FAILED",
  "FK_ROLLBACK_PREFIX_CHANGED", "FOREIGN_KEYS_NOT_ONE", "GENERATION_WRITE_FAILED",
  "GENERATION_WRITE_MISMATCH", "INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED", "LEASES_NOT_ZERO",
  "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH", "LITERAL_QUERY_FAILED",
  "NEGATIVE_REFUSAL_NOT_CONFIRMED", "NEGATIVE_ROLLBACK_CHANGED_CURRENT", "NEGATIVE_WRITE_FAILED",
  "NOT_FK_ONLY", "PARENT_PRESENT", "PREFIX_APPLY_FAILED", "PREFIX_RECEIPTS_CHANGED",
  "PREFIX_VALIDATION_FAILED", "PRESERVED_DATA_MISMATCH", "RECONNECT_FAILED",
  "RESTART_APPLY_FAILED", "RESTART_RECEIPTS_OR_SCHEMA_CHANGED", "ROLLBACK_UNCONFIRMED",
  "SCHEMA_VALIDATION_FAILED", "SEED_DECODE_FAILED", "SEED_MISMATCH", "SEED_QUERY_FAILED",
  "UNEXPECTED_TARGET_DATA", "WITNESS_DECODE_FAILED", "WITNESS_MISMATCH", "WITNESS_QUERY_FAILED",
  "WRONG_BACKEND", "WRONG_FK_FAILURE",
]);
export const MIGRATION_CLOSE_CODES = new Set(["CLOSE_FAILED", "LEASES_NOT_ZERO"]);
export const MIGRATION_FK_PROOF_KINDS = new Set(["EXTENDED", "SAME_WRITER_PRIMARY_HRANA"]);

const HOST = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/;
const MODES = ["--admit", "--freeze", "--diagnostic-unit", "--consume"] as const;

export type SpawnResult = { status: number; stdout: Uint8Array };
export type SpawnFn = (args: string[], env: Record<string, string>) => Promise<SpawnResult>;

export type GuardIO = {
  env: Record<string, string | undefined>;
  spawn: SpawnFn;
  sourceDigest: () => string;
  gitRevParse: () => string;
  metadata: () => Promise<number>;
  print: (line: string) => void;
  eprint: (line: string) => void;
  cwd: string;
};

type Context = { event_name?: string; repository?: string; ref?: string; sha?: string };
type Bag = Record<string, unknown>;

export function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") reject("INVALID_BOOLEAN");
  return value;
}

function present<T>(bag: Bag, key: string, fallback: T): unknown {
  return Object.prototype.hasOwnProperty.call(bag, key) ? bag[key] : fallback;
}

export function validateDispatch(context: Context, inputs: Bag, checkoutSha: string): string {
  const manual = context.event_name === "workflow_dispatch" && (context.ref === "refs/heads/main" || context.ref === REVIEWED_REF);
  const uiManual = context.event_name === "workflow_dispatch" && context.ref === UI_REVIEWED_REF;
  const bootstrap = context.event_name === "push" && context.ref === REVIEWED_REF;
  if (context.repository !== REPOSITORY || !(manual || uiManual || bootstrap)) reject("UNTRUSTED_DISPATCH");
  const sha = context.sha ?? "";
  if (!/^[0-9a-f]{40}$/.test(sha) || checkoutSha !== sha) reject("CHECKOUT_MISMATCH");
  const phase = present(inputs, "phase", "connection");
  if (typeof phase !== "string" || !(PHASES as readonly string[]).includes(phase)) reject("UNKNOWN_PHASE");
  if (uiManual && phase !== "ui-baseline" && phase !== "ui-ack") reject("UI_REF_PHASE_REQUIRED");
  const destructive = boolean(present(inputs, "destructive", false));
  if (bootstrap && phase !== "connection") reject("SECRET_MODE_REQUIRES_MANUAL");
  if (phase === "connection" && destructive) reject("CONNECTION_MUST_BE_READ_ONLY");
  if ((phase === "inventory" || phase === "ui-baseline") && destructive) reject("INVENTORY_MUST_BE_READ_ONLY");
  if (phase !== "connection" && phase !== "inventory" && phase !== "ui-baseline" && !destructive) {
    reject("DESTRUCTIVE_CONFIRMATION_REQUIRED");
  }
  return phase;
}

type UrlParts = {
  scheme: string;
  hostname: string;
  netloc: string;
  username: string | null;
  password: string | null;
  port: number | null;
  path: string;
  query: string;
  fragment: string;
};

export function urlsplit(url: string): UrlParts {
  const hash = url.indexOf("#");
  const fragment = hash >= 0 ? url.slice(hash + 1) : "";
  const beforeHash = hash >= 0 ? url.slice(0, hash) : url;
  const queryAt = beforeHash.indexOf("?");
  const query = queryAt >= 0 ? beforeHash.slice(queryAt + 1) : "";
  const beforeQuery = queryAt >= 0 ? beforeHash.slice(0, queryAt) : beforeHash;
  const schemeEnd = beforeQuery.indexOf("://");
  if (schemeEnd <= 0) throw new Error("Invalid URL");
  const scheme = beforeQuery.slice(0, schemeEnd);
  const rest = beforeQuery.slice(schemeEnd + 3);
  const slash = rest.indexOf("/");
  const netloc = slash < 0 ? rest : rest.slice(0, slash);
  const path = slash < 0 ? "" : rest.slice(slash);
  if (netloc.startsWith("[") && !netloc.includes("]")) throw new Error("Invalid IPv6 URL");
  let username: string | null = null;
  let password: string | null = null;
  let hostport = netloc;
  const at = netloc.lastIndexOf("@");
  if (at >= 0) {
    const userinfo = netloc.slice(0, at);
    hostport = netloc.slice(at + 1);
    const colon = userinfo.indexOf(":");
    if (colon >= 0) {
      username = decodeURIComponentSafe(userinfo.slice(0, colon));
      password = decodeURIComponentSafe(userinfo.slice(colon + 1));
    } else username = decodeURIComponentSafe(userinfo);
  }
  let hostname = hostport;
  let port: number | null = null;
  if (hostname.startsWith("[")) {
    const end = hostname.indexOf("]");
    if (end < 1) throw new Error("Invalid IPv6 URL");
    const inside = hostname.slice(1, end);
    if (!inside) throw new Error("Invalid IPv6 URL");
    hostname = inside.toLowerCase();
    const tail = hostnameTail(hostname, end, hostport);
    port = tail;
  } else {
    const colon = hostport.lastIndexOf(":");
    if (colon > 0 && /^\d+$/.test(hostport.slice(colon + 1))) {
      port = Number(hostport.slice(colon + 1));
      hostname = hostport.slice(0, colon).toLowerCase();
    } else hostname = hostport.toLowerCase();
  }
  return { scheme, hostname, netloc, username, password, port, path, query, fragment };
}

function hostnameTail(_hostname: string, end: number, hostport: string): number | null {
  const after = hostport.slice(end + 1);
  if (after.startsWith(":") && /^\d+$/.test(after.slice(1))) return Number(after.slice(1));
  if (after !== "") throw new Error("Invalid IPv6 URL");
  return null;
}

function decodeURIComponentSafe(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function controlled(value: string): boolean {
  for (const char of value) {
    const code = char.codePointAt(0)!;
    if (code <= 32 || code === 127) return true;
  }
  return false;
}

export function validateTarget(inputs: Bag, settings: Bag, secrets: Bag): string {
  const phase = present(inputs, "phase", "connection");
  if (typeof phase !== "string" || !(PHASES as readonly string[]).includes(phase)) reject("UNKNOWN_PHASE");
  const destructive = boolean(present(inputs, "destructive", false));
  const allow = settings.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE;
  if (phase === "connection" && destructive) reject("CONNECTION_MUST_BE_READ_ONLY");
  if ((phase === "inventory" || phase === "ui-baseline") && (destructive || allow !== "false")) {
    reject("INVENTORY_MUST_BE_READ_ONLY");
  }
  if (phase !== "connection" && phase !== "inventory" && phase !== "ui-baseline" && (destructive !== true || allow !== "true")) {
    reject("DESTRUCTIVE_NOT_ALLOWED");
  }
  const url = present(secrets, "FVOCI_TEST_TURSO_DATABASE_URL", "");
  const token = present(secrets, "FVOCI_TEST_TURSO_AUTH_TOKEN", "");
  if (typeof url !== "string" || !url || typeof token !== "string" || !token) reject("MISSING_SECRET");
  if ([...url].length > 2048 || [...token].length > 16384 || controlled(url + token)) reject("INVALID_SECRET_FORMAT");
  let parsed: UrlParts;
  try {
    parsed = urlsplit(url);
  } catch {
    reject("INVALID_PRIMARY_URL");
  }
  const host = parsed.hostname || "";
  const valid = (parsed.scheme === "libsql" || parsed.scheme === "https")
    && HOST.test(host)
    && host.endsWith(".turso.io")
    && host.length <= 253
    && parsed.netloc === host
    && parsed.username === null
    && parsed.password === null
    && parsed.port === null
    && (parsed.path === "" || parsed.path === "/")
    && !parsed.query
    && !parsed.fragment
    && !url.includes("?")
    && !url.includes("#");
  if (!valid) reject("INVALID_PRIMARY_URL");
  return phase;
}

export function requireImplemented(phase: string): void {
  if (!(PHASES as readonly string[]).includes(phase)) reject("UNKNOWN_PHASE");
  if (!["connection", "migration", "inventory", "reset", "ui-baseline", "ui-ack"].includes(phase)) {
    reject("NOT_IMPLEMENTED");
  }
}

export function validateEnvironment(environment: Bag): void {
  const id = environment.id;
  if (environment.name !== ENVIRONMENT || typeof id !== "number" || !Number.isInteger(id) || id <= 0) {
    reject("ENVIRONMENT_POLICY_DENIED");
  }
}

export function pyString(value: string): string {
  let out = '"';
  for (let index = 0; index < value.length; index++) {
    const char = value[index]!;
    const code = value.charCodeAt(index);
    if (char === '"') out += '\\"';
    else if (char === "\\") out += "\\\\";
    else if (char === "\b") out += "\\b";
    else if (char === "\f") out += "\\f";
    else if (char === "\n") out += "\\n";
    else if (char === "\r") out += "\\r";
    else if (char === "\t") out += "\\t";
    else if (code < 0x20 || code > 0x7e) out += "\\u" + code.toString(16).padStart(4, "0");
    else out += char;
  }
  return out + '"';
}

export function pyDumps(value: unknown): string {
  if (value === null) return "null";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return JSON.stringify(value);
  if (typeof value === "string") return pyString(value);
  if (Array.isArray(value)) return "[" + value.map((item) => pyDumps(item)).join(", ") + "]";
  if (typeof value === "object") {
    return "{" + Object.entries(value as Record<string, unknown>).map(([key, item]) => pyString(key) + ": " + pyDumps(item)).join(", ") + "}";
  }
  throw new Error("unsupported json");
}

export function pyDumpsSorted(value: Record<string, string>): string {
  const keys = Object.keys(value).sort();
  return "{" + keys.map((key) => pyString(key) + ": " + pyString(value[key]!)).join(", ") + "}";
}

export function fileDigest(path: string): string {
  const hash = createHash("sha256");
  const fd = openSync(path, "r");
  try {
    const buffer = Buffer.alloc(1024 * 1024);
    while (true) {
      const count = readSync(fd, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest("hex");
}

function gitEnvironment(env: Record<string, string | undefined>): Record<string, string> {
  return { PATH: env.PATH ?? "", GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" };
}

function gitSync(args: string[], env: Record<string, string | undefined>): { status: number; stdout: Uint8Array } {
  const result = Bun.spawnSync(args, { env: gitEnvironment(env), stdout: "pipe", stderr: "ignore" });
  return { status: result.exitCode ?? 1, stdout: result.stdout };
}

export function sourceDigest(env: Record<string, string | undefined> = process.env): string {
  if (gitSync(["git", "diff", "--quiet", "HEAD"], env).status !== 0) reject("SOURCE_CHANGED");
  const listed = gitSync(["git", "ls-files", "-z"], env);
  if (listed.status !== 0) throw new Error("git ls-files");
  const names = new TextDecoder("utf-8", { fatal: true }).decode(listed.stdout).split("\0").filter(Boolean);
  const hashes: Record<string, string> = {};
  for (const name of names) hashes[name] = createHash("sha256").update(readFileSync(name)).digest("hex");
  return createHash("sha256").update(pyDumpsSorted(hashes)).digest("hex");
}

function isSymlink(path: string): boolean {
  try {
    return lstatSync(path).isSymbolicLink();
  } catch {
    return false;
  }
}

function pythonResolve(path: string): string {
  const absolute = resolve(path);
  try {
    return realpathSync(absolute);
  } catch {
    const parts = absolute.split(sep).filter((part, index) => part || index === 0);
    for (let index = parts.length; index > 0; index--) {
      const prefix = parts.slice(0, index).join(sep) || sep;
      try {
        const real = realpathSync(prefix);
        const rest = parts.slice(index);
        return rest.length ? [real, ...rest].join(sep) : real;
      } catch {
        continue;
      }
    }
    return absolute;
  }
}

function inside(child: string, parent: string): boolean {
  const resolvedChild = pythonResolve(child);
  const resolvedParent = pythonResolve(parent);
  return resolvedChild === resolvedParent || resolvedChild.startsWith(resolvedParent.endsWith(sep) ? resolvedParent : resolvedParent + sep);
}

function decode(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

function picked(env: Record<string, string | undefined>, keys: string[]): Record<string, string> {
  const child: Record<string, string> = {};
  for (const key of keys) if (env[key] !== undefined) child[key] = env[key]!;
  return child;
}

export async function realSpawn(args: string[], env: Record<string, string>): Promise<SpawnResult> {
  const proc = Bun.spawn(args, { env, stdout: "pipe", stderr: "pipe" });
  const chunks: Uint8Array[] = [];
  const pump = async (stream: ReadableStream<Uint8Array> | null | undefined) => {
    if (!stream) return;
    const reader = stream.getReader();
    try {
      while (true) {
        const next = await reader.read();
        if (next.done) break;
        if (next.value) chunks.push(next.value);
      }
    } finally {
      reader.releaseLock();
    }
  };
  const status = (await Promise.all([proc.exited, pump(proc.stdout), pump(proc.stderr)]))[0];
  const size = chunks.reduce((total, chunk) => total + chunk.length, 0);
  const stdout = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    stdout.set(chunk, offset);
    offset += chunk.length;
  }
  return { status, stdout };
}

function runnerTemp(env: Record<string, string | undefined>): string {
  const root = env.RUNNER_TEMP;
  if (!root) throw new Error("RUNNER_TEMP");
  return resolve(root);
}

export function freezeCompiledTest(checkoutSha: string, io: Pick<GuardIO, "env" | "sourceDigest" | "cwd">): void {
  const root = runnerTemp(io.env);
  const target = root + "/turso-target";
  const artifactFile = root + "/turso-compile.json";
  const artifacts = readFileSync(artifactFile, "utf8").split(/\r?\n/).filter((line) => line.length > 0).map((line) => JSON.parse(line) as Bag);
  const sourcePath = resolve(io.cwd, "src", "lib.rs");
  const matches = artifacts.filter((artifact) => artifact.reason === "compiler-artifact"
    && JSON.stringify((artifact.target as Bag | undefined)?.kind) === JSON.stringify(["lib"])
    && (artifact.target as Bag | undefined)?.name === "fvoci_server"
    && (artifact.target as Bag | undefined)?.src_path === sourcePath
    && (artifact.profile as Bag | undefined)?.test === true
    && JSON.stringify(artifact.features) === JSON.stringify(["db-tests"])
    && artifact.executable);
  const finished = artifacts.some((artifact) => artifact.reason === "build-finished" && artifact.success === true);
  if (matches.length !== 1 || !finished) reject("COMPILED_TEST_BINDING_FAILED");
  const artifact = matches[0]!;
  const executable = String(artifact.executable);
  if (isSymlink(executable) || !inside(executable, target + "/debug/deps")) reject("COMPILED_TEST_BINDING_FAILED");
  if (!readFileSync(executable).subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) reject("COMPILED_TEST_BINDING_FAILED");
  const frozen = root + "/turso-connection-libtest";
  if (existsSync(frozen)) reject("COMPILED_TEST_BINDING_FAILED");
  copyFileSync(executable, frozen);
  chmodSync(frozen, 0o700);
  const manifest = {
    sha: checkoutSha,
    source_digest: io.sourceDigest(),
    binary_sha256: fileDigest(frozen),
    cargo_output_sha256: createHash("sha256").update(readFileSync(artifactFile)).digest("hex"),
    native_input_sha256: fileDigest(root + "/fvoci-sqlite/consumer-inputs.json"),
    artifact,
  };
  writeFileSync(root + "/turso-connection-build.json", pyDumps(manifest));
}

type Binding = [string, string, string, string, string];

export function diagnosticUnitBinding(checkoutSha: string, io: Pick<GuardIO, "env" | "sourceDigest">): [string, Binding] {
  const root = runnerTemp(io.env);
  const manifestPath = root + "/turso-connection-build.json";
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Bag;
  const executable = root + "/turso-connection-libtest";
  const source = io.sourceDigest();
  const binary = fileDigest(executable);
  const native = fileDigest(root + "/fvoci-sqlite/consumer-inputs.json");
  const cargoOutput = fileDigest(root + "/turso-compile.json");
  if (manifest.sha !== checkoutSha || manifest.source_digest !== source || isSymlink(executable)
    || manifest.binary_sha256 !== binary || manifest.native_input_sha256 !== native
    || manifest.cargo_output_sha256 !== cargoOutput) {
    reject("COMPILED_TEST_BINDING_FAILED");
  }
  if (!readFileSync(executable).subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) reject("COMPILED_TEST_BINDING_FAILED");
  return [executable, [source, binary, native, cargoOutput, fileDigest(manifestPath)]];
}

function sameBinding(left: [string, Binding], right: [string, Binding]): boolean {
  return left[0] === right[0] && left[1].every((item, index) => item === right[1][index]);
}

export async function runDiagnosticUnit(checkoutSha: string, io: GuardIO): Promise<void> {
  const bound = diagnosticUnitBinding(checkoutSha, io);
  const child = picked(io.env, ["PATH", "LD_LIBRARY_PATH", "TZ"]);
  const listed = await io.spawn([bound[0], DIAGNOSTIC_UNIT_NAME, "--list", "--exact"], child);
  const listing = decode(listed.stdout);
  const matches = [...listing.matchAll(/^([^\r\n]+): (test|benchmark)\r?$/gm)].map((match) => [match[1], match[2]]);
  if (listed.status !== 0 || JSON.stringify(matches) !== JSON.stringify([[DIAGNOSTIC_UNIT_NAME, "test"]])) {
    reject("TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED");
  }
  if (!sameBinding(diagnosticUnitBinding(checkoutSha, io), bound)) reject("COMPILED_TEST_BINDING_FAILED");
  const result = await io.spawn([bound[0], DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"], child);
  if (!sameBinding(diagnosticUnitBinding(checkoutSha, io), bound)) reject("COMPILED_TEST_BINDING_FAILED");
  const output = decode(result.stdout);
  const cases = [...output.matchAll(/^test (\S+) \.\.\. (ok|FAILED|ignored)\r?$/gm)].map((match) => [match[1], match[2]]);
  const summaries = output.split(/\r?\n/).filter((line) => line.startsWith("test result:"));
  if (result.status !== 0 || JSON.stringify(cases) !== JSON.stringify([[DIAGNOSTIC_UNIT_NAME, "ok"]])
    || summaries.length !== 1
    || !/^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;[^\r\n]*$/.test(summaries[0]!)) {
    reject("TURSO_DIAGNOSTIC_UNIT_FAILED");
  }
  io.print("TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN");
}

function full(pattern: string, text: string): RegExpMatchArray | null {
  return new RegExp("^(?:" + pattern + ")$").exec(text);
}

export function resetResult(result: { status: number }, output: string, io: Pick<GuardIO, "print">): void {
  const receipt = "FVOCI_TURSO_RESET_RECEIPT primary=([A-Z_]+) rollback=(NOT_STARTED|RETURNED_OK|UNCONFIRMED) commit=(NOT_STARTED|RETURNED_OK|UNCONFIRMED) blank=(NOT_RUN|CONFIRMED|FAILED) steps=(0|[1-9][0-9]{0,2}) close=(OK|FAILED) drain=(LOCAL_OK|UNCONFIRMED) leases=(ZERO|FAILED)";
  const frame = "\\n*running 1 test\\r?\\ntest " + escapeRegExp(RESET_TEST_NAME) + " \\.\\.\\. ";
  const success = full(frame + receipt + "\\r?\\nok\\r?\\n\\r?\\ntest result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+(?:\\.[0-9]+)?s\\r?\\n*", output);
  const healthy = ["OK", "NOT_STARTED", "RETURNED_OK", "CONFIRMED", "126", "OK", "LOCAL_OK", "ZERO"];
  if (result.status === 0 && success && success.slice(1).join("\0") === healthy.join("\0")) {
    io.print("TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO");
    io.print("TURSO_RESET_PASS tests=1 ignored=0");
    return;
  }
  if (result.status !== 0 && output.length <= 32768) {
    const failed = full(frame + receipt + "\\r?\\n\\r?\\nFVOCI_TURSO_RESET_RETURN\\r?\\n([\\s\\S]{0,16384}?)FAILED\\r?\\n\\r?\\nfailures:\\r?\\n\\r?\\nfailures:\\r?\\n[ \\t]+"
      + escapeRegExp(RESET_TEST_NAME) + "\\r?\\n\\r?\\ntest result: FAILED\\. 0 passed; 1 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+(?:\\.[0-9]+)?s\\r?\\n*", output);
    if (failed) {
      const [primary, rollback, commit, blank, steps, close, drain, leases, opaque] = failed.slice(1);
      const markers = ["FVOCI_TURSO_", "test result:", "running ", "test ", "failures:"];
      const settled = (commit === "NOT_STARTED" && blank === "NOT_RUN")
        || (commit === "UNCONFIRMED" && primary === "COMMIT_UNCONFIRMED" && blank === "NOT_RUN" && rollback === "NOT_STARTED")
        || (commit === "RETURNED_OK" && rollback === "NOT_STARTED" && ((primary === "OK" && blank === "CONFIRMED") || (primary === "RESET_FRESH_BLANK_FAILED" && blank === "FAILED")));
      const completed = Number(steps);
      const beforeEffect = !["OK", "RESET_DDL_FAILED", "RESET_BLANK_IN_WRITER_FAILED", "COMMIT_UNCONFIRMED", "RESET_FRESH_BLANK_FAILED"].includes(primary!);
      const stage = (beforeEffect && completed === 0 && commit === "NOT_STARTED")
        || (primary === "RESET_DDL_FAILED" && completed < 126 && commit === "NOT_STARTED")
        || (primary === "RESET_BLANK_IN_WRITER_FAILED" && completed === 126 && commit === "NOT_STARTED")
        || (["OK", "COMMIT_UNCONFIRMED", "RESET_FRESH_BLANK_FAILED"].includes(primary!) && completed === 126);
      const rollbackShape = ((primary === "BEGIN_FAILED" || primary === "WRONG_PRODUCT_BACKEND") && rollback === "NOT_STARTED")
        || (commit !== "NOT_STARTED" && rollback === "NOT_STARTED")
        || (primary !== "BEGIN_FAILED" && primary !== "WRONG_PRODUCT_BACKEND" && commit === "NOT_STARTED" && (rollback === "RETURNED_OK" || rollback === "UNCONFIRMED"));
      if (RESET_PRIMARY_CODES.has(primary!) && completed <= 126 && settled && stage && rollbackShape
        && ((close === "OK") === (drain === "LOCAL_OK"))
        && (primary !== "COMMIT_UNCONFIRMED" || commit === "UNCONFIRMED")
        && (!["OK", "RESET_FRESH_BLANK_FAILED"].includes(primary!) || commit === "RETURNED_OK")
        && !(primary === "OK" && close === "OK" && leases === "ZERO")
        && count(output, "FVOCI_TURSO_RESET_RECEIPT") === 1
        && count(output, "FVOCI_TURSO_RESET_RETURN") === 1
        && !markers.some((marker) => opaque!.includes(marker))
        && !/(?:^|\n)FAILED\r?(?:\n|$)/.test(opaque!)) {
        io.print("TURSO_RESET_FAILURE " + ["primary=" + primary, "rollback=" + rollback, "commit=" + commit, "blank=" + blank, "steps=" + steps, "close=" + close, "drain=" + drain, "leases=" + leases].join(" "));
      }
    }
  }
  reject("TURSO_RESET_FAILED");
}

function count(text: string, marker: string): number {
  let found = 0;
  let from = 0;
  while (true) {
    const index = text.indexOf(marker, from);
    if (index < 0) return found;
    found += 1;
    from = index + marker.length;
  }
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function inventoryFailureDiagnostic(result: { status: number }, output: string, io: Pick<GuardIO, "print">): void {
  if (result.status === 0 || output.length > 32768 || ["FVOCI_TURSO_INVENTORY_RECEIPT", "FVOCI_TURSO_INVENTORY_DIAGNOSTIC", "FVOCI_TURSO_INVENTORY_RETURN"].some((marker) => count(output, marker) !== 1)) return;
  const match = full("(?:\\r?\\n)*running 1 test\\r?\\ntest " + escapeRegExp(INVENTORY_TEST_NAME)
    + " \\.\\.\\. FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED) leases=(ZERO|FAILED)\\r?\\n\\r?\\n"
    + "FVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=([A-Z_]+) rollback=([A-Z_]+) close=([A-Z_]+) leases=(ZERO|FAILED)\\r?\\n"
    + "FVOCI_TURSO_INVENTORY_RETURN\\r?\\n((?:[^\\n]*\\n)*?)FAILED\\r?\\n(?:\\r?\\n)*failures:\\r?\\n(?:\\r?\\n)*failures:\\r?\\n    "
    + escapeRegExp(INVENTORY_TEST_NAME) + "\\r?\\n(?:\\r?\\n)*test result: FAILED\\. 0 passed; 1 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+\\.[0-9]+s\\r?\\n(?:\\r?\\n)*", output);
  if (!match) return;
  const [rollbackReceipt, closeReceipt, leaseReceipt, primary, rollback, close, leases, harness] = match.slice(1);
  if (harness!.length > 16384 || ["FVOCI_TURSO_", "test ", "test result:", "running ", "failures:"].some((marker) => harness!.includes(marker)) || /(?:^|\n)FAILED\r?(?:\n|$)/.test(harness!)) return;
  const rollbackMap: Record<string, string> = { OK: "OK", NOT_STARTED: "NOT_STARTED", ROLLBACK_UNCONFIRMED: "FAILED" };
  if ((!INVENTORY_PRIMARY_CODES.has(primary!) && primary !== "OK")
    || !["OK", "NOT_STARTED", "ROLLBACK_UNCONFIRMED"].includes(rollback!)
    || !["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"].includes(close!)
    || leases !== leaseReceipt
    || rollbackReceipt !== rollbackMap[rollback!]
    || (close === "OK") !== (closeReceipt === "OK")
    || (primary === "BEGIN_FAILED" || primary === "WRONG_PRODUCT_BACKEND") !== (rollback === "NOT_STARTED")
    || (primary === "OK" && rollback === "OK" && close === "OK" && leases === "ZERO")) return;
  io.print("TURSO_INVENTORY_FAILURE classification=REFUSED prefix=NONE schema_sha256=NONE rollback=" + rollbackReceipt + " close=" + closeReceipt + " leases=" + leaseReceipt);
  io.print("TURSO_INVENTORY_DIAGNOSTIC primary=" + primary + " rollback=" + rollback + " close=" + close + " leases=" + leases);
}

export function inventoryResult(result: { status: number }, output: string, io: Pick<GuardIO, "print">): void {
  const match = full("(?:\\r?\\n)*running 1 test\\r?\\ntest " + escapeRegExp(INVENTORY_TEST_NAME)
    + " \\.\\.\\. FVOCI_TURSO_INVENTORY_RECEIPT classification=(BLANK|PREFIX|CURRENT) prefix=(0|[1-9]|1[0-2]) schema_sha256=([0-9a-f]{64}) rollback=OK close=OK leases=ZERO\\r?\\nok\\r?\\n"
    + "(?:\\r?\\n)*test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+\\.[0-9]+s\\r?\\n(?:\\r?\\n)*", output);
  if (result.status !== 0 || !match) {
    inventoryFailureDiagnostic(result, output, io);
    reject("TURSO_INVENTORY_FAILED");
  }
  const [classification, prefix, schemaHash] = match.slice(1);
  const value = Number(prefix);
  if (!((classification === "BLANK" && prefix === "0") || (classification === "PREFIX" && value >= 1 && value <= 11) || (classification === "CURRENT" && prefix === "12"))) {
    reject("TURSO_INVENTORY_FAILED");
  }
  io.print("TURSO_INVENTORY_RECEIPT classification=" + classification + " prefix=" + prefix + " schema_sha256=" + schemaHash + " rollback=OK close=OK leases=ZERO");
  io.print("TURSO_INVENTORY_PASS tests=1 ignored=0");
}

export function migrationResult(result: { status: number }, output: string, io: Pick<GuardIO, "print">): void {
  const pattern = /FVOCI_TURSO_MIGRATION_RECEIPT primary=(OK|FAILED) prefix=(OK|NOT_CONFIRMED) fk_rollback=(OK|NOT_CONFIRMED) fk_proof=(EXTENDED|SAME_WRITER_PRIMARY_HRANA|NOT_CONFIRMED) current=(OK|NOT_CONFIRMED) restart=(OK|NOT_CONFIRMED) close=(OK|FAILED) leases=(ZERO|FAILED)(?:\r?\n|$)/g;
  const receipt = [...output.matchAll(pattern)].map((match) => match.slice(1));
  if (receipt.length !== 1) reject("TURSO_MIGRATION_RECEIPT_MISSING");
  const fields = receipt[0]!;
  io.print("TURSO_MIGRATION_RECEIPT " + fields.join(" "));
  const lines = output.split("\n");
  const diagnostics = lines.flatMap((line, index) => line.includes("FVOCI_TURSO_MIGRATION_DIAGNOSTIC")
    ? [index < lines.length - 1 && line.endsWith("\r") ? line.slice(0, -1) : line]
    : []);
  if (diagnostics.length) {
    if (diagnostics.length !== 1 || fields[0] !== "FAILED") reject("TURSO_MIGRATION_FAILED");
    const diagnostic = /^FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=([A-Z_]+) close=([A-Z_]+)$/.exec(diagnostics[0]!);
    if (!diagnostic) reject("TURSO_MIGRATION_FAILED");
    const primary = diagnostic[1]!;
    const close = diagnostic[2]!;
    if ((!MIGRATION_PRIMARY_CODES.has(primary) && primary !== "OK") || (!MIGRATION_CLOSE_CODES.has(close) && close !== "OK")
      || (primary === "OK" && close === "OK") || (close === "OK") !== (fields[6] === "OK")) {
      reject("TURSO_MIGRATION_FAILED");
    }
    io.print("TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close);
  }
  if (result.status !== 0 || fields.slice(0, 3).join("\0") !== ["OK", "OK", "OK"].join("\0") || !MIGRATION_FK_PROOF_KINDS.has(fields[3]!)
    || fields.slice(4).join("\0") !== ["OK", "OK", "OK", "ZERO"].join("\0")
    || !/test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;/.test(output)
    || !new RegExp("test " + escapeRegExp(MIGRATION_TEST_NAME) + " \\.\\.\\. ").test(output)) {
    reject("TURSO_MIGRATION_FAILED");
  }
  io.print("TURSO_MIGRATION_FK_PROOF kind=" + fields[3]);
  io.print("TURSO_MIGRATION_PASS tests=1 ignored=0");
}

function bindingStill(checkoutSha: string, io: GuardIO, executable: string): void {
  const root = runnerTemp(io.env);
  const manifest = JSON.parse(readFileSync(root + "/turso-connection-build.json", "utf8")) as Bag;
  if (manifest.sha !== checkoutSha || manifest.source_digest !== io.sourceDigest() || isSymlink(executable)
    || manifest.binary_sha256 !== fileDigest(executable)
    || manifest.native_input_sha256 !== fileDigest(root + "/fvoci-sqlite/consumer-inputs.json")) {
    reject("COMPILED_TEST_BINDING_FAILED");
  }
  if (!readFileSync(executable).subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) reject("COMPILED_TEST_BINDING_FAILED");
}

async function runPrimary(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  const phase = String(present(inputs, "phase", "connection"));
  requireImplemented(phase);
  if (io.env.FVOCI_DATABASE_BACKEND !== "libsql-remote") reject("BACKEND_SELECTOR_REQUIRED");
  const settings = { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: io.env.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE ?? "" };
  const credentials = {
    FVOCI_TEST_TURSO_DATABASE_URL: io.env.FVOCI_TEST_TURSO_DATABASE_URL ?? "",
    FVOCI_TEST_TURSO_AUTH_TOKEN: io.env.FVOCI_TEST_TURSO_AUTH_TOKEN ?? "",
  };
  validateTarget(inputs, settings, credentials);
  const inventoryBinding = phase === "inventory" || phase === "reset" ? diagnosticUnitBinding(checkoutSha, io) : null;
  const root = runnerTemp(io.env);
  const executable = root + "/turso-connection-libtest";
  bindingStill(checkoutSha, io, executable);
  const child = picked(io.env, ["PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ"]);
  child.FVOCI_DATABASE_BACKEND = "libsql-remote";
  let testName = MIGRATION_TEST_NAME;
  if (phase === "reset") {
    child.FVOCI_TEST_TURSO_DATABASE_URL = credentials.FVOCI_TEST_TURSO_DATABASE_URL;
    child.FVOCI_TEST_TURSO_AUTH_TOKEN = credentials.FVOCI_TEST_TURSO_AUTH_TOKEN;
    Object.assign(child, {
      FVOCI_TEST_TURSO_RESET_SELECTED: "1",
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
      FVOCI_TEST_TURSO_PHASE: "migration",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    });
    testName = RESET_TEST_NAME;
  } else {
    child.FVOCI_LIBSQL_URL = credentials.FVOCI_TEST_TURSO_DATABASE_URL;
    child.FVOCI_LIBSQL_AUTH_TOKEN = credentials.FVOCI_TEST_TURSO_AUTH_TOKEN;
    if (phase === "connection") {
      child.FVOCI_TEST_TURSO_CONNECTION_SELECTED = "1";
      testName = TEST_NAME;
    } else if (phase === "inventory") {
      Object.assign(child, {
        FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
        FVOCI_TEST_TURSO_PHASE: "inventory",
        FVOCI_TEST_TURSO_DESTRUCTIVE: "false",
        FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
      });
      testName = INVENTORY_TEST_NAME;
    } else {
      Object.assign(child, {
        FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
        FVOCI_TEST_TURSO_PHASE: "migration",
        FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
        FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
      });
    }
  }
  const result = await io.spawn([executable, testName, "--ignored", "--exact", "--test-threads=1", "--nocapture"], child);
  if (phase === "inventory" || phase === "reset") {
    if (!inventoryBinding || !sameBinding(diagnosticUnitBinding(checkoutSha, io), inventoryBinding)) reject("COMPILED_TEST_BINDING_FAILED");
    const output = decode(result.stdout);
    if (phase === "reset") resetResult({ status: result.status }, output, io);
    else inventoryResult({ status: result.status }, output, io);
    return;
  }
  const output = decode(result.stdout);
  if (phase === "migration") {
    migrationResult({ status: result.status }, output, io);
    return;
  }
  const known = new Set(["OK", "CONNECT_FAILED", "BEGIN_FAILED", "WRONG_BACKEND", "FK_QUERY_FAILED", "FK_DECODE_FAILED", "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED", "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH"]);
  const found = [...output.matchAll(/FVOCI_TURSO_RECEIPT primary=([A-Z_]+) rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED|NOT_STARTED) leases=(ZERO|FAILED|NOT_OBSERVED)/g)].map((match) => match.slice(1));
  if (found.length === 1 && known.has(found[0]![0]!)) io.print("TURSO_CONNECTION_RECEIPT " + found[0]!.join(" "));
  else reject("TURSO_CONNECTION_RECEIPT_MISSING");
  const receipt = found[0]!;
  if (result.status !== 0 || receipt.join("\0") !== ["OK", "OK", "OK", "ZERO"].join("\0")
    || !/test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out;/.test(output)
    || !new RegExp("test " + escapeRegExp(TEST_NAME) + " \\.\\.\\. ").test(output)) {
    reject("TURSO_CONNECTION_FAILED");
  }
  io.print("TURSO_CONNECTION_PASS tests=1 ignored=0");
}

export function runConnection(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  if (String(present(inputs, "phase", "connection")) !== "connection") reject("WRONG_CONSUMER_PHASE");
  return runPrimary(checkoutSha, inputs, io);
}

export function runMigration(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  if (inputs.phase !== "migration") reject("WRONG_CONSUMER_PHASE");
  return runPrimary(checkoutSha, inputs, io);
}

export function runInventory(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  if (inputs.phase !== "inventory") reject("WRONG_CONSUMER_PHASE");
  return runPrimary(checkoutSha, inputs, io);
}

export function runReset(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  if (inputs.phase !== "reset") reject("WRONG_CONSUMER_PHASE");
  return runPrimary(checkoutSha, inputs, io);
}

export async function runUi(checkoutSha: string, inputs: Bag, io: GuardIO): Promise<void> {
  const phase = String(inputs.phase);
  if (io.env.FVOCI_DATABASE_BACKEND !== "libsql-remote") reject("BACKEND_SELECTOR_REQUIRED");
  const settings = { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: io.env.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE ?? "" };
  const credentials = {
    FVOCI_TEST_TURSO_DATABASE_URL: io.env.FVOCI_LIBSQL_URL ?? "",
    FVOCI_TEST_TURSO_AUTH_TOKEN: io.env.FVOCI_LIBSQL_AUTH_TOKEN ?? "",
  };
  validateTarget(inputs, settings, credentials);
  if (inputs.ui_source_sha !== checkoutSha) reject("UI_REVIEWED_SOURCE_REQUIRED");
  if (phase === "ui-ack" && !/^[0-9a-f]{64}$/.test(String(inputs.ui_baseline_sha256 ?? ""))) reject("UI_CURRENT_DATASET_BINDING_REQUIRED");
  if (phase === "ui-ack" && !/^[0-9a-f]{64}$/.test(String(inputs.ui_target_sha256 ?? ""))) reject("UI_CURRENT_TARGET_BINDING_REQUIRED");
  const ui = await import("./turso-ui.ts");
  try {
    await ui.consume(phase, inputs, io.env);
  } catch (error) {
    if (error instanceof ui.UiError) reject(error.message);
    throw error;
  }
}

async function defaultMetadata(): Promise<number> {
  const opener = async (suffix: string) => {
    const url = API_ROOT + suffix;
    const response = await fetch(url, {
      method: "GET",
      redirect: "manual",
      headers: { Accept: "application/vnd.github+json", "X-GitHub-Api-Version": "2026-03-10" },
      signal: AbortSignal.timeout(15000),
    });
    if (response.status !== 200 || response.url !== url) reject("ENVIRONMENT_METADATA_UNAVAILABLE");
    const reader = response.body?.getReader();
    if (!reader) reject("ENVIRONMENT_METADATA_UNAVAILABLE");
    const chunks: Uint8Array[] = [];
    let total = 0;
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      total += next.value.length;
      if (total > 262144) reject("ENVIRONMENT_METADATA_UNAVAILABLE");
      chunks.push(next.value);
    }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    const value = JSON.parse(new TextDecoder().decode(bytes)) as unknown;
    if (!value || typeof value !== "object" || Array.isArray(value)) reject("ENVIRONMENT_METADATA_UNAVAILABLE");
    return value as Bag;
  };
  try {
    const body = await opener("");
    validateEnvironment(body);
    return body.id as number;
  } catch (error) {
    if (error instanceof AdmissionError) throw error;
    reject("ENVIRONMENT_METADATA_UNAVAILABLE");
  }
}

export function defaultIO(overrides: Partial<GuardIO> = {}): GuardIO {
  const env = overrides.env ?? process.env;
  return {
    env,
    spawn: overrides.spawn ?? realSpawn,
    sourceDigest: overrides.sourceDigest ?? (() => sourceDigest(env)),
    gitRevParse: overrides.gitRevParse ?? (() => {
      const result = gitSync(["git", "rev-parse", "HEAD"], env);
      if (result.status !== 0) throw new Error("git rev-parse");
      return new TextDecoder().decode(result.stdout).trim();
    }),
    metadata: overrides.metadata ?? defaultMetadata,
    print: overrides.print ?? ((line) => process.stdout.write(line + "\n")),
    eprint: overrides.eprint ?? ((line) => process.stderr.write(line + "\n")),
    cwd: overrides.cwd ?? process.cwd(),
  };
}

export async function environmentMetadata(fetchBody?: () => Promise<Bag>): Promise<number> {
  if (!fetchBody) return defaultMetadata();
  const value = await fetchBody();
  validateEnvironment(value);
  return value.id as number;
}

export async function main(argv: string[], overrides: Partial<GuardIO> = {}): Promise<number> {
  const io = defaultIO(overrides);
  try {
    if (argv.length !== 1 || !(MODES as readonly string[]).includes(argv[0]!)) reject("EXPLICIT_MODE_REQUIRED");
    const eventPath = io.env.GITHUB_EVENT_PATH;
    if (!eventPath) throw new Error("event");
    const event = JSON.parse(readFileSync(eventPath, "utf8")) as Bag;
    const inputs = { ...((event.inputs as Bag | undefined) ?? {}) };
    const flag = present(inputs, "destructive", "false");
    if (flag !== "true" && flag !== "false") reject("INVALID_BOOLEAN");
    inputs.destructive = flag === "true";
    const checkoutSha = io.gitRevParse();
    const phase = validateDispatch({
      event_name: io.env.GITHUB_EVENT_NAME,
      repository: io.env.GITHUB_REPOSITORY,
      ref: io.env.GITHUB_REF,
      sha: io.env.GITHUB_SHA,
    }, inputs, checkoutSha);
    requireImplemented(phase);
    if (io.env.GITHUB_EVENT_NAME === "push") {
      if (argv[0] !== "--admit") reject("SECRET_MODE_REQUIRES_MANUAL");
      io.print("BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN");
      return 0;
    }
    if (argv[0] === "--admit") {
      const environmentId = await io.metadata();
      const output = io.env.GITHUB_OUTPUT;
      if (!output) throw new Error("output");
      appendFileSync(output, "environment_id=" + environmentId + "\n");
      io.print("ENVIRONMENT_ADMISSION_OK_RUNTIME_NOT_RUN");
    } else if (argv[0] === "--freeze") {
      freezeCompiledTest(checkoutSha, io);
      io.print("COMPILED_TEST_FROZEN_RUNTIME_NOT_RUN");
    } else if (argv[0] === "--diagnostic-unit") {
      await runDiagnosticUnit(checkoutSha, io);
    } else if (phase === "connection") await runConnection(checkoutSha, inputs, io);
    else if (phase === "inventory") await runInventory(checkoutSha, inputs, io);
    else if (phase === "reset") await runReset(checkoutSha, inputs, io);
    else if (phase === "ui-baseline" || phase === "ui-ack") await runUi(checkoutSha, inputs, io);
    else await runPrimary(checkoutSha, inputs, io);
    return 0;
  } catch (error) {
    io.eprint(error instanceof AdmissionError ? error.message : "ADMISSION_FAILED");
    return 78;
  }
}

if (import.meta.main) {
  process.exit(await main(process.argv.slice(2)));
}
