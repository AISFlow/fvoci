// Python value semantics the workflow checks depend on: `==` between parsed
// YAML values, `str.strip/split/splitlines` whitespace and `repr()` in error
// strings. Callers compare error strings byte for byte with the original.
import { createHash } from "node:crypto";

export type Mapping = Record<string, unknown>;

export function isMapping(value: unknown): value is Mapping {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Python `dict.get(key, fallback)` on a value known to be a mapping.
export function get(map: Mapping, key: string, fallback?: unknown): unknown {
  return Object.hasOwn(map, key) ? map[key] : fallback;
}

export function has(map: Mapping, key: string): boolean {
  return Object.hasOwn(map, key);
}

// Python `==`: bool and number compare numerically (True == 1), containers
// compare structurally, mapping key order is irrelevant.
export function pyEq(a: unknown, b: unknown): boolean {
  const numeric = (v: unknown) => typeof v === "number" || typeof v === "boolean";
  if (numeric(a) && numeric(b)) return Number(a) === Number(b);
  if (a === null || b === null) return a === b;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, index) => pyEq(item, b[index]));
  }
  if (isMapping(a) || isMapping(b)) {
    if (!isMapping(a) || !isMapping(b)) return false;
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => has(b, key) && pyEq(a[key], b[key]));
  }
  return a === b;
}

// Python `in`: substring for str, element for list, key for dict. Any other
// container raises in Python; here it is "not contained" so checks fail closed.
export function pyContains(container: unknown, item: string): boolean {
  if (typeof container === "string") return container.includes(item);
  if (Array.isArray(container)) return container.some((element) => pyEq(element, item));
  if (isMapping(container)) return has(container, item);
  return false;
}

// str.isspace() code points used by strip()/split() without arguments.
export const PY_WS =
  "\\t\\n\\v\\f\\r\\x1c-\\x1f \\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const LEADING_WS = new RegExp(`^[${PY_WS}]+`);
const TRAILING_WS = new RegExp(`[${PY_WS}]+$`);
const WS_RUN = new RegExp(`[${PY_WS}]+`);

export function pyStrip(text: string): string {
  return text.replace(LEADING_WS, "").replace(TRAILING_WS, "");
}

export function pySplit(text: string): string[] {
  const stripped = pyStrip(text);
  return stripped === "" ? [] : stripped.split(WS_RUN);
}

export function pyRstripChar(text: string, char: string): string {
  let end = text.length;
  while (end > 0 && text[end - 1] === char) end -= 1;
  return text.slice(0, end);
}

// str.splitlines(): every Python line boundary, no trailing empty line.
export function pySplitlines(text: string): string[] {
  // eslint-disable-next-line no-control-regex -- str.splitlines() also breaks at \x1c-\x1e
  const lines = text.split(/\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/);
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

const NON_PRINTABLE = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]/u;

function reprString(text: string): string {
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let out = quote;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    if (char === "\\") out += "\\\\";
    else if (char === quote) out += "\\" + quote;
    else if (char === "\n") out += "\\n";
    else if (char === "\r") out += "\\r";
    else if (char === "\t") out += "\\t";
    else if (char !== " " && NON_PRINTABLE.test(char)) {
      if (code < 0x100) out += "\\x" + code.toString(16).padStart(2, "0");
      else if (code < 0x10000) out += "\\u" + code.toString(16).padStart(4, "0");
      else out += "\\U" + code.toString(16).padStart(8, "0");
    } else out += char;
  }
  return out + quote;
}

// repr() for the YAML/JSON value space. Floats that are whole numbers print
// without ".0" because the parsed value no longer carries that distinction.
export function pyRepr(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  if (typeof value === "string") return reprString(value);
  if (typeof value === "number") {
    if (Number.isNaN(value)) return "nan";
    if (!Number.isFinite(value)) return value > 0 ? "inf" : "-inf";
    return String(value);
  }
  if (Array.isArray(value)) return "[" + value.map(pyRepr).join(", ") + "]";
  if (isMapping(value)) {
    return (
      "{" +
      Object.entries(value)
        .map(([key, item]) => reprString(key) + ": " + pyRepr(item))
        .join(", ") +
      "}"
    );
  }
  return typeof value === "bigint" ? value.toString() : typeof value;
}

export function sortedPy(values: Iterable<string>): string[] {
  return [...values].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
}

export function sha256Hex(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

export function setEq(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  return a.size === b.size && [...a].every((item) => b.has(item));
}

export function intersect(a: ReadonlySet<string>, b: ReadonlySet<string>): Set<string> {
  return new Set([...a].filter((item) => b.has(item)));
}

export function union(...sets: ReadonlySet<string>[]): Set<string> {
  return new Set(sets.flatMap((set) => [...set]));
}
