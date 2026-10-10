// Python text and JSON semantics that callers of the replaced Python harness
// observe (ordering, str()/repr() of JSON values, json.dumps bytes). Only the
// behaviour those outputs depend on is reproduced (see the migration commit).

/** Python str ordering: by code point, not UTF-16 unit. */
export function compareCodePoints(a: string, b: string): number {
  const left = a[Symbol.iterator]();
  const right = b[Symbol.iterator]();
  for (;;) {
    const x = left.next();
    const y = right.next();
    if (x.done || y.done) return (x.done ? 0 : 1) - (y.done ? 0 : 1);
    const order = (x.value.codePointAt(0) ?? 0) - (y.value.codePointAt(0) ?? 0);
    if (order !== 0) return order;
  }
}

/** Python tuple ordering of string sequences. */
export function compareSequences(a: readonly string[], b: readonly string[]): number {
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    const order = compareCodePoints(a[index] ?? "", b[index] ?? "");
    if (order !== 0) return order;
  }
  return a.length - b.length;
}

/** `len(text)` of a Python str: code points, a lone surrogate counting as one. */
export function codePointLength(text: string): number {
  return Array.from(text).length;
}

/** `text[start:end]` of a Python str, by code point. */
export function sliceCodePoints(text: string, start: number, end?: number): string {
  if (end !== undefined && codePointLength(text) <= end && start === 0) return text;
  return Array.from(text).slice(start, end).join("");
}

/** Python's str whitespace (`\s`, `str.isspace`, `str.rstrip()`), as a regex class body. */
export const PY_SPACE =
  "\\t\\n\\v\\f\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const TRAILING_SPACE = new RegExp(`[${PY_SPACE}]+$`, "u");

/** `str.rstrip()` with no argument. */
export function pyRstrip(text: string): string {
  return text.replace(TRAILING_SPACE, "");
}

/** `str.splitlines()`: every Python line boundary, no trailing empty line. */
export function pySplitlines(text: string): string[] {
  if (text === "") return [];
  // eslint-disable-next-line no-control-regex -- Python also breaks lines at FS, GS and RS.
  const lines = text.split(/\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/u);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** A JSON number token written with a fraction or exponent: a Python float. */
export class JsonFloat {
  constructor(readonly value: number) {}
  toJSON(): number {
    return this.value;
  }
}

/**
 * Standard JSON.parse; the reviver's token source keeps Python's int/float
 * split (`1` is an int, `1.0` and `1e0` are floats) that callers type-check.
 */
export function parseJson(text: string): unknown {
  return JSON.parse(text, (_key, value: unknown, context?: { source?: string }) => {
    if (typeof value !== "number") return value;
    const source = context?.source;
    if (source === undefined) throw new Error("JSON number source is required");
    if (/[.eE]/.test(source)) return new JsonFloat(value);
    return value === 0 ? 0 : value;
  });
}

export type Dict = Record<string, unknown>;

/** A JSON object (Python dict). */
export function isDict(value: unknown): value is Dict {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    !(value instanceof JsonFloat)
  );
}

export function has(value: Dict, key: string): boolean {
  return Object.hasOwn(value, key);
}

/** `dict.get(key)`: missing and null are both Python None (null). */
export function get(value: Dict, key: string): unknown {
  return has(value, key) ? (value[key] ?? null) : null;
}

/** Python truthiness of a JSON value. */
export function truthy(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (value instanceof JsonFloat) return value.value !== 0;
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.keys(value).length > 0;
  return Boolean(value);
}

/** The numeric value of an int, float or bool (Python arithmetic), else undefined. */
export function numeric(value: unknown): number | undefined {
  if (typeof value === "number") return value;
  if (value instanceof JsonFloat) return value.value;
  if (typeof value === "boolean") return value ? 1 : 0;
  return undefined;
}

/** `repr(float)`: shortest round-trip digits in Python's layout. */
export function pyFloatRepr(value: number): string {
  if (Number.isNaN(value)) return "nan";
  if (!Number.isFinite(value)) return value > 0 ? "inf" : "-inf";
  const sign = value < 0 || Object.is(value, -0) ? "-" : "";
  const [mantissa = "0", exponentText = "0"] = Math.abs(value).toExponential().split("e");
  const digits = mantissa.replace(".", "");
  const exponent = Number(exponentText);
  if (exponent < -4 || exponent >= 16) {
    const fraction = digits.length > 1 ? `.${digits.slice(1)}` : "";
    const magnitude = String(Math.abs(exponent)).padStart(2, "0");
    return `${sign}${digits.slice(0, 1)}${fraction}e${exponent < 0 ? "-" : "+"}${magnitude}`;
  }
  if (exponent < 0) return `${sign}0.${"0".repeat(-exponent - 1)}${digits}`;
  const whole = digits.slice(0, exponent + 1).padEnd(exponent + 1, "0");
  return `${sign}${whole}.${digits.slice(exponent + 1) || "0"}`;
}

function integerText(value: number): string {
  return Math.abs(value) < 1e21 ? String(value) : BigInt(value).toString();
}

/**
 * `format(value, ".Nf")` for N = 0 or 1: Python rounds an exact binary tie
 * to even, where Number.prototype.toFixed rounds it up.
 */
export function formatFixed(value: number, digits: 0 | 1): string {
  if (Number.isNaN(value)) return "nan";
  if (!Number.isFinite(value)) return value > 0 ? "inf" : "-inf";
  const sign = value < 0 || Object.is(value, -0) ? "-" : "";
  const magnitude = Math.abs(value);
  const scale = digits === 0 ? 1 : 10;
  // A tie is magnitude*scale = k + 1/2: an odd multiple of 1/2 (N=0) or 1/4 (N=1).
  const quarterUnits = magnitude * (digits === 0 ? 2 : 4);
  let text: string;
  if (Number.isInteger(quarterUnits) && quarterUnits % 2 === 1 && magnitude < 2 ** 50) {
    const low = Math.floor(magnitude * scale);
    const units = low % 2 === 0 ? low : low + 1;
    const raw = String(units).padStart(digits + 1, "0");
    text = digits === 0 ? raw : `${raw.slice(0, -1)}.${raw.slice(-1)}`;
  } else if (magnitude >= 1e21) {
    text = `${integerText(magnitude)}${digits === 0 ? "" : ".0"}`;
  } else {
    text = magnitude.toFixed(digits);
  }
  return `${sign}${text}`;
}

/**
 * `json.dumps` bytes (ensure_ascii, the given separators): the standard
 * JSON.stringify escapes strings; Python's float repr and layout are kept.
 */
export function pyJsonDumps(value: unknown, itemSeparator = ", ", keySeparator = ": "): string {
  const ascii = (text: string) =>
    text.replace(
      /[\u0080-\uffff]/g,
      (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
    );
  const dump = (item: unknown): string => {
    if (item === null || item === undefined) return "null";
    if (item === true) return "true";
    if (item === false) return "false";
    if (item instanceof JsonFloat) {
      if (Number.isNaN(item.value)) return "NaN";
      if (!Number.isFinite(item.value)) return item.value > 0 ? "Infinity" : "-Infinity";
      return pyFloatRepr(item.value);
    }
    if (typeof item === "number") return integerText(item);
    if (typeof item === "string") return ascii(JSON.stringify(item));
    if (Array.isArray(item)) return `[${item.map(dump).join(itemSeparator)}]`;
    if (typeof item === "object") {
      const members = Object.entries(item).map(
        ([key, member]) => `${ascii(JSON.stringify(key))}${keySeparator}${dump(member)}`,
      );
      return `{${members.join(itemSeparator)}}`;
    }
    throw new TypeError("value is not JSON serializable");
  };
  return dump(value);
}

/** `str(value)` of a JSON value; containers print as JSON, not Python repr. */
export function display(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "number") return integerText(value);
  if (value instanceof JsonFloat) return pyFloatRepr(value.value);
  return JSON.stringify(value);
}
