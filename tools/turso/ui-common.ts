// Shared failure, JSON and private-file policy of the hosted Turso UI consumer.
import { lstatSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import process from "node:process";
import { digest, env, parseJson, root as checkout, uid } from "../selected-backend-ci/io.ts";
import { pyJsonDumps } from "../web-e2e/compat.ts";

/** Fixed codes only; no SDK, URL, token, actor password or raw trace. */
export class UiError extends Error {
  constructor(code: string) {
    super(code);
    this.name = "UiError";
  }
}

export function require(condition: unknown, code: string): asserts condition {
  if (!condition) throw new UiError(code);
}

export function failureCode(error: unknown): string {
  const value = error instanceof UiError ? error.message : "";
  return /^(?:UI|TURSO_UI|RESTORE)_[A-Z_]+$/.test(value) ? value : "UI_CONSUMER_FAILED";
}

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type Record_ = Record<string, unknown>;

export const isRecord = (value: unknown): value is Record_ =>
  typeof value === "object" && value !== null && !Array.isArray(value);

// Strict structural access: a missing key, a wrong container or an index out of
// range is a malformed input, never an implicit undefined.
export function get(value: unknown, ...path: (string | number)[]): unknown {
  let current = value;
  for (const key of path) {
    if (typeof key === "number") {
      if (!Array.isArray(current) || key < 0 || key >= current.length)
        throw new TypeError("malformed input");
      current = current[key] as unknown;
    } else {
      if (!isRecord(current) || !Object.hasOwn(current, key))
        throw new TypeError("malformed input");
      current = current[key];
    }
  }
  return current;
}
export const list = (value: unknown, ...path: (string | number)[]): unknown[] => {
  const item = get(value, ...path);
  if (!Array.isArray(item)) throw new TypeError("malformed input");
  return item;
};
export const record = (value: unknown, ...path: (string | number)[]): Record_ => {
  const item = get(value, ...path);
  if (!isRecord(item)) throw new TypeError("malformed input");
  return item;
};
export const text = (value: unknown, ...path: (string | number)[]): string => {
  const item = get(value, ...path);
  if (typeof item !== "string") throw new TypeError("malformed input");
  return item;
};

export function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (isRecord(value))
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortKeys(value[key])]),
    );
  return value;
}
/** Sorted keys, compact separators, non-ASCII kept: the digest form of a value. */
export const canonical = (value: unknown): string => JSON.stringify(sortKeys(value));
export const valueDigest = (value: unknown): string => digest(canonical(value));
/** The native fixture's stdin: the canonical query, or nothing. */
export const fixtureInput = (input?: Record_): string =>
  input && Object.keys(input).length ? canonical(input) : "";

/** Public diagnostic line in the original default `json.dumps` form. */
export const diagnosticJson = (value: unknown): string => pyJsonDumps(value);

export interface Output {
  out(line: string): void;
  err(line: string): void;
}
export const stdio: Output = {
  out: (line) => {
    process.stdout.write(line + "\n");
  },
  err: (line) => {
    process.stderr.write(line + "\n");
  },
};

// ignoreBOM: a leading U+FEFF is data (refused by JSON.parse), never silently dropped.
const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** Bun flags for every Bun child: only the given environment, only the repository bunfig. */
export const bunIsolation = (workspace: string = checkout): string[] => [
  "--no-env-file",
  "--config=" + join(workspace, "bunfig.toml"),
];
export const decodeUtf8 = (bytes: Uint8Array): string => utf8.decode(bytes);
export const parseBytes = (bytes: Uint8Array): unknown => parseJson(decodeUtf8(bytes));
export const parsePlain = (bytes: Uint8Array): unknown => JSON.parse(decodeUtf8(bytes)) as unknown;

export function privateRead(path: string, cap = 1024 * 1024): unknown {
  const info = lstatSync(path);
  require(isAbsolute(path) &&
    info.isFile() &&
    info.nlink === 1 &&
    info.uid === uid() &&
    (info.mode & 0o777) === 0o600 &&
    info.size <= cap, "UI_PRIVATE_INPUT_REFUSED");
  return parseBytes(readFileSync(path));
}

export function root(): string {
  const path = join(env("RUNNER_TEMP"), "turso-ui");
  try {
    mkdirSync(path, 0o700);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST" || !statSync(path).isDirectory())
      throw error;
  }
  const facts = statSync(path);
  require(!lstatSync(path).isSymbolicLink() &&
    facts.uid === uid() &&
    (facts.mode & 0o777) === 0o700, "UI_ROOT_REFUSED");
  return path;
}

export function executionMode(): "github-ci" | "orca-local" {
  const mode = process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci";
  require(mode === "github-ci" || mode === "orca-local", "UI_EXECUTION_MODE_REFUSED");
  return mode;
}

export function cleanEnv(): Record<string, string> {
  const names = [
    "PATH",
    "LANG",
    "LD_LIBRARY_PATH",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "TZ",
    "RUNNER_TEMP",
    "PLAYWRIGHT_BROWSERS_PATH",
    "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
  ];
  const result: Record<string, string> = {};
  for (const name of names) {
    const value = process.env[name];
    if (value !== undefined) result[name] = value;
  }
  return result;
}

export const token = (bytes: number): string =>
  Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("hex");

/** Public requests are confined to this actual owned normal server. */
export async function requestPublic(base: string, path: string, body?: Record_, cookie?: string) {
  require(/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/.test(base) &&
    path.startsWith("/api/v1/") &&
    !path.includes(".."), "UI_FINAL_PUBLIC_TARGET_REFUSED");
  const response = await fetch(base + path, {
    method: body ? "POST" : "GET",
    redirect: "manual",
    signal: AbortSignal.timeout(10000),
    headers: {
      Origin: base,
      ...(body ? { "Content-Type": "application/json" } : {}),
      ...(cookie ? { Cookie: cookie } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  require(response.ok, "UI_FINAL_PUBLIC_PREPARATION_FAILED");
  const value: unknown = await response.json();
  return {
    value,
    cookie: response.headers
      .getSetCookie()
      .map((value) => value.split(";", 1)[0])
      .join("; "),
  };
}

export async function inviteFinalActor(
  base: string,
  workspace: string,
  ownerCookie: string,
  email: string,
  password: string,
  role: "owner" | "member",
  name: string,
) {
  const invitation = await requestPublic(
    base,
    "/api/v1/workspaces/" + workspace + "/invitations",
    { email, role },
    ownerCookie,
  );
  const accepted = new URL(String(get(invitation.value, "acceptUrl")), base);
  require(accepted.origin === base &&
    /^\/invite\/[a-zA-Z0-9_-]+$/.test(accepted.pathname), "UI_FINAL_INVITATION_TARGET_REFUSED");
  const inviteToken = accepted.pathname.slice("/invite/".length);
  const preview = await requestPublic(base, "/api/v1/invitations/" + inviteToken);
  const consents = list(preview.value, "requiredLegal").map((item) => ({
    kind: get(item, "kind"),
    version: get(item, "version"),
  }));
  const issued = await requestPublic(base, "/api/v1/invitations/" + inviteToken + "/accept", {
    email,
    password,
    givenName: "Selected",
    familyName: name,
    consents,
  });
  require(issued.cookie.length > 0, "UI_FINAL_OWNER_INVITATION_FAILED");
  const owner = await requestPublic(base, "/api/v1/auth/login", { email, password });
  require(owner.cookie.length > 0 &&
    get(owner.value, "userId") ===
      get(issued.value, "userId"), "UI_FINAL_PREPARED_OWNER_LOGIN_FAILED");
  const userId = String(get(owner.value, "userId"));
  const { uuidHex } = await import("./ui-audit.ts");
  uuidHex(userId);
  return { userId, email, password, cookie: owner.cookie };
}
