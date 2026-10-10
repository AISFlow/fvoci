// JSON with the value model and output bytes of Python's `json` module, so the
// plan file, `plan_json` and `postgres_matrix` stay byte-identical with the
// outputs the gate and older runs were built against. Objects keep source key
// order (a Map, also for integer-like keys), integers keep their digits,
// NaN/Infinity parse, and output escapes every non-ASCII code unit.

export class PyInt {
  constructor(readonly digits: bigint) {}
}
export class PyFloat {
  constructor(readonly value: number) {}
}
export type PyValue =
  | null
  | boolean
  | string
  | PyInt
  | PyFloat
  | number
  | PyValue[]
  | Map<string, PyValue>
  | { readonly [key: string]: PyValue };

export class PyJsonError extends Error {}

// CPython refuses int literals longer than sys.int_info.default_max_str_digits.
const MAX_INT_DIGITS = 4300;
const NUMBER_RE = /-?(?:0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?/y;
const WS_RE = /[ \t\n\r]*/y;

export function pyLoads(text: string): PyValue {
  let pos = 0;
  const skip = () => {
    WS_RE.lastIndex = pos;
    WS_RE.exec(text);
    pos = WS_RE.lastIndex;
  };
  const fail = (what: string): never => {
    throw new PyJsonError(`${what} at char ${String(pos)}`);
  };
  const literal = (word: string, value: PyValue): PyValue => {
    if (!text.startsWith(word, pos)) fail("Expecting value");
    pos += word.length;
    return value;
  };
  const string = (): string => {
    pos++; // opening quote
    let out = "";
    for (;;) {
      if (pos >= text.length) fail("Unterminated string starting");
      const ch = text.charCodeAt(pos);
      if (ch === 0x22) {
        pos++;
        return out;
      }
      if (ch < 0x20) fail("Invalid control character");
      if (ch !== 0x5c) {
        out += text.charAt(pos);
        pos++;
        continue;
      }
      const esc = text[pos + 1];
      const simple: Record<string, string> = {
        '"': '"',
        "\\": "\\",
        "/": "/",
        b: "\b",
        f: "\f",
        n: "\n",
        r: "\r",
        t: "\t",
      };
      if (esc !== undefined && esc in simple) {
        out += simple[esc] ?? "";
        pos += 2;
        continue;
      }
      if (esc !== "u") fail("Invalid \\escape");
      const hex = text.slice(pos + 2, pos + 6);
      if (!/^[0-9a-fA-F]{4}$/.test(hex)) fail("Invalid \\uXXXX escape");
      out += String.fromCharCode(parseInt(hex, 16));
      pos += 6;
    }
  };
  const value = (): PyValue => {
    skip();
    const ch = text[pos];
    if (ch === '"') return string();
    if (ch === "{") {
      pos++;
      const map = new Map<string, PyValue>();
      skip();
      if (text[pos] === "}") {
        pos++;
        return map;
      }
      for (;;) {
        skip();
        if (text[pos] !== '"') fail("Expecting property name enclosed in double quotes");
        const key = string();
        skip();
        if (text[pos] !== ":") fail("Expecting ':' delimiter");
        pos++;
        // Duplicate keys keep the first position and the last value, as dict does.
        map.set(key, value());
        skip();
        if (text[pos] === "}") {
          pos++;
          return map;
        }
        if (text[pos] !== ",") fail("Expecting ',' delimiter");
        pos++;
      }
    }
    if (ch === "[") {
      pos++;
      const list: PyValue[] = [];
      skip();
      if (text[pos] === "]") {
        pos++;
        return list;
      }
      for (;;) {
        list.push(value());
        skip();
        if (text[pos] === "]") {
          pos++;
          return list;
        }
        if (text[pos] !== ",") fail("Expecting ',' delimiter");
        pos++;
      }
    }
    if (ch === "n") return literal("null", null);
    if (ch === "t") return literal("true", true);
    if (ch === "f") return literal("false", false);
    if (ch === "N") return literal("NaN", new PyFloat(NaN));
    if (ch === "I") return literal("Infinity", new PyFloat(Infinity));
    if (ch === "-" && text.startsWith("-Infinity", pos)) {
      pos += 9;
      return new PyFloat(-Infinity);
    }
    NUMBER_RE.lastIndex = pos;
    const m = NUMBER_RE.exec(text);
    if (!m) return fail("Expecting value");
    pos = NUMBER_RE.lastIndex;
    if (m[1] === undefined && m[2] === undefined) {
      const digits = m[0].replace("-", "");
      if (digits.length > MAX_INT_DIGITS) fail("Exceeds the limit for integer string conversion");
      return new PyInt(BigInt(m[0]));
    }
    return new PyFloat(Number(m[0]));
  };
  const result = value();
  skip();
  if (pos !== text.length) fail("Extra data");
  return result;
}

/** Python truthiness of a loaded JSON value. */
export function pyTruthy(value: PyValue | undefined): boolean {
  if (value === null || value === undefined || value === false) return false;
  if (value === true) return true;
  if (typeof value === "string") return value.length > 0;
  if (typeof value === "number") return value !== 0;
  if (value instanceof PyInt) return value.digits !== 0n;
  if (value instanceof PyFloat) return value.value !== 0;
  if (Array.isArray(value)) return value.length > 0;
  if (value instanceof Map) return value.size > 0;
  return Object.keys(value).length > 0;
}

export function isMapping(value: PyValue | undefined): value is Map<string, PyValue> {
  return value instanceof Map;
}

/** float.__repr__: shortest round-trip digits, exponent outside 1e-4 <= |x| < 1e16. */
export function pyFloatRepr(x: number): string {
  if (Number.isNaN(x)) return "NaN";
  if (x === Infinity) return "Infinity";
  if (x === -Infinity) return "-Infinity";
  if (x === 0) return Object.is(x, -0) ? "-0.0" : "0.0";
  const sign = x < 0 ? "-" : "";
  const [mantissa = "", exp = "0"] = Math.abs(x).toExponential().split("e");
  const digits = mantissa.replace(".", "");
  const decpt = Number(exp) + 1;
  if (decpt > 16 || decpt < -3) {
    const e = decpt - 1;
    const body = digits.length > 1 ? `${digits[0] ?? ""}.${digits.slice(1)}` : digits;
    const mag = String(Math.abs(e)).padStart(2, "0");
    return `${sign}${body}e${e < 0 ? "-" : "+"}${mag}`;
  }
  if (decpt <= 0) return `${sign}0.${"0".repeat(-decpt)}${digits}`;
  if (decpt >= digits.length) return `${sign}${digits}${"0".repeat(decpt - digits.length)}.0`;
  return `${sign}${digits.slice(0, decpt)}.${digits.slice(decpt)}`;
}

const SHORT_ESCAPES: Record<string, string> = {
  '"': '\\"',
  "\\": "\\\\",
  "\n": "\\n",
  "\r": "\\r",
  "\t": "\\t",
  "\b": "\\b",
  "\f": "\\f",
};

/** json.dumps string encoding with ensure_ascii=True (per UTF-16 code unit). */
export function pyQuote(text: string): string {
  let out = '"';
  for (let i = 0; i < text.length; i++) {
    const ch = text[i] as string;
    const code = text.charCodeAt(i);
    const short = SHORT_ESCAPES[ch];
    if (short !== undefined) out += short;
    else if (code < 0x20 || code > 0x7e) out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += ch;
  }
  return `${out}"`;
}

function compareCodePoints(a: string, b: string): number {
  const ia = a[Symbol.iterator]();
  const ib = b[Symbol.iterator]();
  for (;;) {
    const x = ia.next();
    const y = ib.next();
    if (x.done || y.done) return x.done && y.done ? 0 : x.done ? -1 : 1;
    const cx = x.value.codePointAt(0) ?? 0;
    const cy = y.value.codePointAt(0) ?? 0;
    if (cx !== cy) return cx - cy;
  }
}

/** sorted() order for str: by code point, not by UTF-16 code unit. */
export function pySorted(values: Iterable<string>): string[] {
  return [...values].sort(compareCodePoints);
}

export type DumpOptions = { indent?: number; compact?: boolean; sortKeys?: boolean };

/**
 * json.dumps: default separators (", ", ": "), `compact` for (",", ":"), and
 * `indent` for the pretty form, whose item separator is "," before a newline.
 */
export function pyDumps(value: PyValue, options: DumpOptions = {}): string {
  const { indent, compact = false, sortKeys = false } = options;
  const itemSep = compact || indent !== undefined ? "," : ", ";
  const keySep = compact ? ":" : ": ";
  const encode = (v: PyValue, level: number): string => {
    if (v === null) return "null";
    if (v === true) return "true";
    if (v === false) return "false";
    if (typeof v === "string") return pyQuote(v);
    if (typeof v === "number") {
      if (!Number.isInteger(v)) throw new TypeError("plain numbers must be integers");
      return String(v);
    }
    if (v instanceof PyInt) return v.digits.toString();
    if (v instanceof PyFloat) return pyFloatRepr(v.value);
    const entries: [string, PyValue][] | null = Array.isArray(v)
      ? null
      : v instanceof Map
        ? [...v.entries()]
        : Object.entries(v);
    const parts = Array.isArray(v)
      ? v.map((item) => encode(item, level + 1))
      : (sortKeys
          ? pySorted((entries ?? []).map(([k]) => k)).map(
              (k) => [k, (entries ?? []).find(([key]) => key === k)?.[1] ?? null] as const,
            )
          : (entries ?? [])
        ).map(([k, item]) => `${pyQuote(k)}${keySep}${encode(item, level + 1)}`);
    const [open, close] = Array.isArray(v) ? ["[", "]"] : ["{", "}"];
    if (parts.length === 0) return `${open}${close}`;
    if (indent === undefined) return `${open}${parts.join(itemSep)}${close}`;
    const inner = "\n" + " ".repeat(indent * (level + 1));
    const outer = "\n" + " ".repeat(indent * level);
    return `${open}${inner}${parts.join(itemSep + inner)}${outer}${close}`;
  };
  return encode(value, 0);
}
