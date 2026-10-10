// Pure admission policy: dispatch trust, phase gates, target shape and the
// exact child environment for each consuming phase. No I/O here.
import { codePointLength } from "../web-e2e/compat.ts";
import { isRecord, toDict, type Json } from "./python-compat.ts";

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
export type Phase = (typeof PHASES)[number];
export const REPOSITORY = "AISFlow/fvoci";
export const ENVIRONMENT = "fvoci-turso-test";
export const REVIEWED_REF = "refs/heads/fvoci/v060-turso-verified-connection";
export const UI_REVIEWED_REF = "refs/heads/fvoci/v060-product-integration-20261005";
export const TEST_NAME = "db::turso_test::turso_primary_connection";
export const MIGRATION_TEST_NAME = "db::turso_test::turso_primary_current12_install_resume";
export const INVENTORY_TEST_NAME = "db::turso_test::turso_primary_migration_target_inventory";
export const RESET_TEST_NAME = "db::turso_test::turso_primary_disposable_prefix11_reset";
export const DIAGNOSTIC_UNIT_NAME =
  "db::turso_test::migration_diagnostics_disclose_only_known_static_failures";
export const API_ROOT = "https://api.github.com/repos/AISFlow/fvoci/environments/fvoci-turso-test";
export const MODES = ["--admit", "--freeze", "--diagnostic-unit", "--consume"] as const;
export type Mode = (typeof MODES)[number];

/** Only fixed codes reach output; never an input or SDK error. */
export class AdmissionError extends Error {
  override name = "AdmissionError";
}

export function reject(code: string): never {
  throw new AdmissionError(code);
}

export type Inputs = Record<string, Json>;
export type Env = Record<string, string | undefined>;
export interface DispatchContext {
  event_name: string | undefined;
  repository: string | undefined;
  ref: string | undefined;
  sha: string | undefined;
}

function isPhase(value: unknown): value is Phase {
  return typeof value === "string" && (PHASES as readonly string[]).includes(value);
}

export function phaseOf(inputs: Inputs): unknown {
  return Object.hasOwn(inputs, "phase") ? inputs.phase : "connection";
}

function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") reject("INVALID_BOOLEAN");
  return value;
}

function destructiveOf(inputs: Inputs): boolean {
  return boolean(Object.hasOwn(inputs, "destructive") ? inputs.destructive : false);
}

const READ_ONLY: readonly Phase[] = ["connection", "inventory", "ui-baseline"];

export function validateDispatch(
  context: DispatchContext,
  inputs: Inputs,
  checkoutSha: string,
): Phase {
  const dispatch = context.event_name === "workflow_dispatch";
  const manual = dispatch && (context.ref === "refs/heads/main" || context.ref === REVIEWED_REF);
  const uiManual = dispatch && context.ref === UI_REVIEWED_REF;
  const bootstrap = context.event_name === "push" && context.ref === REVIEWED_REF;
  if (context.repository !== REPOSITORY || !(manual || uiManual || bootstrap))
    reject("UNTRUSTED_DISPATCH");
  // An absent GITHUB_SHA is an internal failure, not a checkout mismatch.
  if (context.sha === undefined) throw new TypeError("GITHUB_SHA unset");
  if (!/^[0-9a-f]{40}$/.test(context.sha) || checkoutSha !== context.sha)
    reject("CHECKOUT_MISMATCH");
  const phase = phaseOf(inputs);
  if (!isPhase(phase)) reject("UNKNOWN_PHASE");
  if (uiManual && phase !== "ui-baseline" && phase !== "ui-ack") reject("UI_REF_PHASE_REQUIRED");
  const destructive = destructiveOf(inputs);
  if (bootstrap && phase !== "connection") reject("SECRET_MODE_REQUIRES_MANUAL");
  if (phase === "connection" && destructive) reject("CONNECTION_MUST_BE_READ_ONLY");
  if ((phase === "inventory" || phase === "ui-baseline") && destructive)
    reject("INVENTORY_MUST_BE_READ_ONLY");
  if (!READ_ONLY.includes(phase) && !destructive) reject("DESTRUCTIVE_CONFIRMATION_REQUIRED");
  return phase;
}

const HOST = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/;
const SCHEME_CHARS = /^[A-Za-z0-9+\-.]+$/;

/**
 * Primary URL shape: libsql or https, a lowercase *.turso.io host as the
 * whole authority, empty or root path, no query/fragment. Mirrors urlsplit:
 * the scheme is case-folded and the authority ends at the first / ? or #.
 */
function primaryUrlValid(url: string): boolean {
  if (url.includes("?") || url.includes("#")) return false;
  let rest = url;
  let scheme = "";
  const colon = url.indexOf(":");
  if (colon > 0 && /^[A-Za-z]/.test(url) && SCHEME_CHARS.test(url.slice(0, colon))) {
    scheme = url.slice(0, colon).toLowerCase();
    rest = url.slice(colon + 1);
  }
  let netloc = "";
  if (rest.startsWith("//")) {
    const end = rest.slice(2).search(/[/?#]/);
    netloc = end < 0 ? rest.slice(2) : rest.slice(2, 2 + end);
    rest = end < 0 ? "" : rest.slice(2 + end);
  }
  const hostinfo = netloc.slice(netloc.lastIndexOf("@") + 1);
  const host = (hostinfo.includes("[") ? "" : (hostinfo.split(":")[0] ?? "")).toLowerCase();
  return (
    (scheme === "libsql" || scheme === "https") &&
    HOST.test(host) &&
    host.endsWith(".turso.io") &&
    host.length <= 253 &&
    netloc === host &&
    (rest === "" || rest === "/")
  );
}

function hasControlOrSpace(text: string): boolean {
  for (const char of text) {
    const point = char.codePointAt(0) ?? 0;
    if (point <= 32 || point === 127) return true;
  }
  return false;
}

/** A consuming step's explicit configuration, without I/O. */
export function validateTarget(
  inputs: Inputs,
  settings: Env,
  secrets: Record<string, unknown>,
): Phase {
  const phase = phaseOf(inputs);
  if (!isPhase(phase)) reject("UNKNOWN_PHASE");
  const destructive = destructiveOf(inputs);
  const allow = settings.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE;
  if (phase === "connection" && destructive) reject("CONNECTION_MUST_BE_READ_ONLY");
  if ((phase === "inventory" || phase === "ui-baseline") && (destructive || allow !== "false")) {
    reject("INVENTORY_MUST_BE_READ_ONLY");
  }
  if (!READ_ONLY.includes(phase) && (!destructive || allow !== "true"))
    reject("DESTRUCTIVE_NOT_ALLOWED");
  const url = secrets.FVOCI_TEST_TURSO_DATABASE_URL ?? "";
  const token = secrets.FVOCI_TEST_TURSO_AUTH_TOKEN ?? "";
  if (typeof url !== "string" || url === "" || typeof token !== "string" || token === "")
    reject("MISSING_SECRET");
  if (
    codePointLength(url) > 2048 ||
    codePointLength(token) > 16384 ||
    hasControlOrSpace(url + token)
  ) {
    reject("INVALID_SECRET_FORMAT");
  }
  if (!primaryUrlValid(url)) reject("INVALID_PRIMARY_URL");
  return phase;
}

const IMPLEMENTED: readonly Phase[] = [
  "connection",
  "migration",
  "inventory",
  "reset",
  "ui-baseline",
  "ui-ack",
];

export function requireImplemented(phase: unknown): void {
  if (!isPhase(phase)) reject("UNKNOWN_PHASE");
  if (!IMPLEMENTED.includes(phase)) reject("NOT_IMPLEMENTED");
}

/** The preexisting Environment; its current branch policy is null by design. */
export function validateEnvironment(
  environment: Record<string, unknown>,
  idText: string | null,
): string {
  // The exact integer token is the output, so ids beyond 2^53 are not rounded.
  if (environment.name !== ENVIRONMENT || idText === null || !(Number(idText) > 0)) {
    reject("ENVIRONMENT_POLICY_DENIED");
  }
  return idText;
}

/** GitHub event payload to the inputs every mode validates. */
export function eventInputs(event: unknown): Inputs {
  if (!isRecord(event)) throw new TypeError("event is not an object");
  const inputs = toDict(Object.hasOwn(event, "inputs") ? event.inputs : {});
  // GitHub encodes boolean inputs as strings; any other value is refused.
  const flag = Object.hasOwn(inputs, "destructive") ? inputs.destructive : "false";
  if (flag !== "true" && flag !== "false") reject("INVALID_BOOLEAN");
  inputs.destructive = flag === "true";
  return inputs;
}

export type Action =
  | { kind: "bootstrap" }
  | { kind: "admit" }
  | { kind: "freeze" }
  | { kind: "diagnostic-unit" }
  | { kind: "consume"; phase: Phase };

/** Mode routing after dispatch admission; push only ever admits source. */
export function decide(
  mode: Mode,
  context: DispatchContext,
  inputs: Inputs,
  checkoutSha: string,
): Action {
  const phase = validateDispatch(context, inputs, checkoutSha);
  requireImplemented(phase);
  if (context.event_name === "push") {
    if (mode !== "--admit") reject("SECRET_MODE_REQUIRES_MANUAL");
    return { kind: "bootstrap" };
  }
  if (mode === "--admit") return { kind: "admit" };
  if (mode === "--freeze") return { kind: "freeze" };
  if (mode === "--diagnostic-unit") return { kind: "diagnostic-unit" };
  return { kind: "consume", phase };
}

export function parseMode(argv: readonly string[]): Mode {
  const mode = argv[0];
  if (argv.length !== 1 || !(MODES as readonly string[]).includes(mode as string))
    reject("EXPLICIT_MODE_REQUIRED");
  return mode as Mode;
}

export function contextFrom(env: Env): DispatchContext {
  return {
    event_name: env.GITHUB_EVENT_NAME,
    repository: env.GITHUB_REPOSITORY,
    ref: env.GITHUB_REF,
    sha: env.GITHUB_SHA,
  };
}

function pick(env: Env, keys: readonly string[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const key of keys) {
    const value = env[key];
    if (value !== undefined) out[key] = value;
  }
  return out;
}

/** The credential-free diagnostic unit sees no DB/provider/GitHub value. */
export function diagnosticChildEnv(env: Env): Record<string, string> {
  return pick(env, ["PATH", "LD_LIBRARY_PATH", "TZ"]);
}

export function primaryTestName(phase: Phase): string {
  if (phase === "connection") return TEST_NAME;
  if (phase === "inventory") return INVENTORY_TEST_NAME;
  if (phase === "reset") return RESET_TEST_NAME;
  return MIGRATION_TEST_NAME;
}

/**
 * Exact child environment per phase. No unrelated credential is forwarded;
 * reset sees only the test pair, the others the product names that
 * DatabaseSettings reads. Inventory gets its own read-only mode.
 */
export function primaryChildEnv(
  phase: Phase,
  env: Env,
  url: string,
  token: string,
): Record<string, string> {
  const child = pick(env, ["PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ"]);
  child.FVOCI_DATABASE_BACKEND = "libsql-remote";
  if (phase === "reset") {
    child.FVOCI_TEST_TURSO_DATABASE_URL = url;
    child.FVOCI_TEST_TURSO_AUTH_TOKEN = token;
  } else {
    child.FVOCI_LIBSQL_URL = url;
    child.FVOCI_LIBSQL_AUTH_TOKEN = token;
  }
  if (phase === "connection") {
    child.FVOCI_TEST_TURSO_CONNECTION_SELECTED = "1";
  } else if (phase === "inventory") {
    Object.assign(child, {
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
      FVOCI_TEST_TURSO_PHASE: "inventory",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "false",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
    });
  } else {
    if (phase === "reset") child.FVOCI_TEST_TURSO_RESET_SELECTED = "1";
    Object.assign(child, {
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
      FVOCI_TEST_TURSO_PHASE: "migration",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    });
  }
  return child;
}

/** UI inputs bound to the reviewed source and, for ack, the observed digests. */
export function validateUiInputs(phase: Phase, inputs: Inputs, checkoutSha: string): void {
  if ((Object.hasOwn(inputs, "ui_source_sha") ? inputs.ui_source_sha : undefined) !== checkoutSha)
    reject("UI_REVIEWED_SOURCE_REQUIRED");
  if (phase !== "ui-ack") return;
  for (const [key, code] of [
    ["ui_baseline_sha256", "UI_CURRENT_DATASET_BINDING_REQUIRED"],
    ["ui_target_sha256", "UI_CURRENT_TARGET_BINDING_REQUIRED"],
  ] as const) {
    const value = Object.hasOwn(inputs, key) ? inputs[key] : "";
    if (typeof value !== "string") throw new TypeError(key + " is not a string");
    if (!/^[0-9a-f]{64}$/.test(value)) reject(code);
  }
}
