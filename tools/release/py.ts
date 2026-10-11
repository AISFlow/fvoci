// Text and formatting rules the release tools share with their callers'
// expectations: messages quote names the way the release docs and tests read
// them (['A', 'B']), files are read as UTF-8 with universal newlines, and JSON
// records keep the byte layout already published in release directories.
import { readFileSync } from "node:fs";
import { TextDecoder } from "node:util";

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export class ReleaseError extends Error {}

export function decodeUtf8(bytes: Uint8Array): string {
  return utf8.decode(bytes);
}

// Strict UTF-8, a BOM kept as U+FEFF, CRLF and lone CR read as LF.
export function readText(path: string): string {
  return universalNewlines(decodeUtf8(readFileSync(path)));
}

export function universalNewlines(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}

// Lines of text read by readText; no trailing empty line. Only LF ends a line:
// a form feed or Unicode separator stays inside the line and so fails the
// caller's line check instead of starting a new entry.
export function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

const NON_PRINTABLE = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]/u;

function hex(code: number, width: number): string {
  return code.toString(16).padStart(width, "0");
}

export function reprString(value: string): string {
  const quote = value.includes("'") && !value.includes('"') ? '"' : "'";
  let out = quote;
  for (const char of value) {
    const code = char.codePointAt(0) ?? 0;
    if (char === quote || char === "\\") out += "\\" + char;
    else if (char === "\t") out += "\\t";
    else if (char === "\n") out += "\\n";
    else if (char === "\r") out += "\\r";
    else if (code < 0x20 || code === 0x7f) out += "\\x" + hex(code, 2);
    else if (code < 0x7f || char === " " || !NON_PRINTABLE.test(char)) out += char;
    else if (code < 0x100) out += "\\x" + hex(code, 2);
    else if (code < 0x10000) out += "\\u" + hex(code, 4);
    else out += "\\U" + hex(code, 8);
  }
  return out + quote;
}

export function repr(value: unknown): string {
  if (typeof value === "string") return reprString(value);
  if (value === null || value === undefined) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  if (typeof value === "number") return String(value);
  if (Array.isArray(value)) return "[" + value.map(repr).join(", ") + "]";
  if (typeof value === "object") {
    const items = Object.entries(value).map(([k, v]) => `${reprString(k)}: ${repr(v)}`);
    return "{" + items.join(", ") + "}";
  }
  return typeof value;
}

// Sorted by code point, as names are listed in messages.
export function sorted(values: Iterable<string>): string[] {
  const points = (s: string) => Array.from(s, (c) => c.codePointAt(0) ?? 0);
  return [...values].sort((a, b) => {
    const [x, y] = [points(a), points(b)];
    for (let i = 0; i < Math.min(x.length, y.length); i++) {
      const d = (x[i] ?? 0) - (y[i] ?? 0);
      if (d) return d;
    }
    return x.length - y.length;
  });
}

// Empty strings, lists and mappings count as absent.
export function truthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  if (isRecord(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

function jsonString(value: string, ascii: boolean): string {
  const text = JSON.stringify(value);
  return ascii ? text.replace(/[\u007f-\uffff]/g, (c) => "\\u" + hex(c.charCodeAt(0), 4)) : text;
}

type DumpOptions = { indent?: number; ascii?: boolean };

// JSON with the separators ", " and ": " (", " becomes "," with an indent)
// and non-ASCII escaped unless ascii is false.
export function dumpJson(value: unknown, options: DumpOptions = {}): string {
  const ascii = options.ascii ?? true;
  const indent = options.indent;
  const dump = (node: unknown, depth: number): string => {
    if (node === null) return "null";
    if (typeof node === "string") return jsonString(node, ascii);
    if (typeof node === "number" || typeof node === "boolean") return JSON.stringify(node);
    const entries: string[] = Array.isArray(node)
      ? node.map((item) => dump(item, depth + 1))
      : Object.entries(node as object).map(
          ([k, v]) => `${jsonString(k, ascii)}: ${dump(v, depth + 1)}`,
        );
    const [open, close] = Array.isArray(node) ? ["[", "]"] : ["{", "}"];
    if (entries.length === 0) return open + close;
    if (indent === undefined) return open + entries.join(", ") + close;
    const inner = "\n" + " ".repeat(indent * (depth + 1));
    return open + inner + entries.join("," + inner) + "\n" + " ".repeat(indent * depth) + close;
  };
  return dump(value, 0);
}

// Equality of decoded JSON values; object key order is irrelevant, types are
// not (true is not 1).
export function jsonEqual(a: unknown, b: unknown): boolean {
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
    return a === b;
  }
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => jsonEqual(item, b[i]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  return (
    keys.length === Object.keys(right).length &&
    keys.every((key) => Object.hasOwn(right, key) && jsonEqual(left[key], right[key]))
  );
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function count(text: string, needle: string): number {
  return text.split(needle).length - 1;
}

export function escapeRegExp(text: string): string {
  return text.replace(/[\\^$.*+?()[\]{}|/-]/g, "\\$&");
}

// Every non-overlapping match's first group, or the whole match without one.
export function findAll(pattern: RegExp, text: string): string[] {
  return [...text.matchAll(pattern)].map((m) => m[1] ?? m[0]);
}
