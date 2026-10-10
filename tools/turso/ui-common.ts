// Shared failure, JSON and private-file policy of the hosted Turso UI consumer.
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import process from "node:process";
import { env, parseJson, uid } from "../selected-backend-ci/io.ts";

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
  return /^(?:UI|TURSO_UI)_[A-Z_]+$/.test(value) ? value : "UI_CONSUMER_FAILED";
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
export const valueDigest = (value: unknown): string =>
  createHash("sha256").update(canonical(value)).digest("hex");
export const textDigest = (value: string): string =>
  createHash("sha256").update(value).digest("hex");

/** Public diagnostic line in the original default separator form (", " and ": "). */
export function diagnosticJson(value: unknown): string {
  if (Array.isArray(value)) return "[" + value.map(diagnosticJson).join(", ") + "]";
  if (isRecord(value))
    return (
      "{" +
      Object.entries(value)
        .map(([key, item]) => JSON.stringify(key) + ": " + diagnosticJson(item))
        .join(", ") +
      "}"
    );
  if (typeof value === "string")
    return JSON.stringify(value).replace(
      /[\u007f-\uffff]/g,
      (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"),
    );
  return JSON.stringify(value ?? null);
}

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

const utf8 = new TextDecoder("utf-8", { fatal: true });
export const decodeUtf8 = (bytes: Uint8Array): string => utf8.decode(bytes);
export const parseBytes = (bytes: Uint8Array): unknown => parseJson(decodeUtf8(bytes));

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
