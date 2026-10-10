/**
 * Semantic comparison of two PostgreSQL catalog dumps.
 *
 * What this checks: tables, columns, constraints, indexes, triggers, policies,
 * sequences, functions, views, extensions, seeds, row counts, and, when both
 * dumps include it, the app role. Values are catalog facts already extracted
 * by the dump (types, defaults, pg_get_* definitions, ACLs), never raw DDL.
 *
 * What it guards: the migration ledger is the only allowed difference, and
 * only after the old side is the retired (version, applied_at) shape and the
 * new side is contiguous fvoci-postgres-060 receipts with NOT NULL
 * (version, lineage, sql_sha256, applied_at) and distinct 64-hex digests.
 * Ledger ACLs, RLS, triggers, policies, column ACLs, and the app role's
 * identity and grants are not part of that exception. Duplicate object names,
 * unreadable input, and malformed JSON fail closed.
 *
 * What callers read: exit 0 when nothing semantic remains, exit 1 for a
 * semantic difference or a bad input, exit 2 for usage errors, plus the
 * markdown report on stdout and, with --report, in that file. The report
 * layout (RESULT, DIFF/MISSING/EXTRA/COLUMN-ORDER/LEDGER/NOTE, and the
 * historical literal rendering of values) is the output format.
 */
import { readFileSync, writeFileSync, writeSync } from "node:fs";
import { basename } from "node:path";

const LEDGER_TABLE = "schema_migrations";
const NEW_LINEAGE = "fvoci-postgres-060";
const NEW_LEDGER_COLUMNS = ["version", "lineage", "sql_sha256", "applied_at"];
const OLD_LEDGER_COLUMNS = ["version", "applied_at"];
const HEX64 = /^[0-9a-f]{64}$/;
const ROLE_KEYS = [
  "role",
  "attributes",
  "schema_privileges",
  "table_privileges",
  "column_privileges",
  "routine_privileges",
  "sequence_usage",
  "schema_usage",
];
const ATTRIBUTE_KEYS = [
  "exists",
  "superuser",
  "inherit",
  "createrole",
  "createdb",
  "login",
  "replication",
  "bypassrls",
  "member_of",
];
const TABLE_PRIV_KEYS = ["schema", "table", "privilege", "grantor", "grantable"];
const COLUMN_PRIV_KEYS = ["schema", "table", "column", "privilege", "grantor", "grantable"];
const ELEVATED_FLAGS = ["superuser", "bypassrls", "createrole", "createdb", "replication"];
const TABLE_NESTED: Record<string, string> = {
  columns: "name",
  constraints: "name",
  indexes: "name",
  triggers: "name",
  policies: "name",
};
const NONPRINTABLE_B64 =
  "AAAAAAAfAAB/AACgAACtAACtAAN4AAN5AAOAAAODAAOLAAOLAAONAAONAAOiAAOiAAUwAAUwAAVXAAVYAAWLAAWMAAWQAAWQAAXIAAXPAAXrAAXuAAX1AAYFAAYcAAYcAAbdAAbdAAcOAAcPAAdLAAdMAAeyAAe/AAf7AAf8AAguAAgvAAg/AAg/AAhcAAhdAAhfAAhfAAhrAAhvAAiPAAiXAAjiAAjiAAmEAAmEAAmNAAmOAAmRAAmSAAmpAAmpAAmxAAmxAAmzAAm1AAm6AAm7AAnFAAnGAAnJAAnKAAnPAAnWAAnYAAnbAAneAAneAAnkAAnlAAn/AAoAAAoEAAoEAAoLAAoOAAoRAAoSAAopAAopAAoxAAoxAAo0AAo0AAo3AAo3AAo6AAo7AAo9AAo9AApDAApGAApJAApKAApOAApQAApSAApYAApdAApdAApfAAplAAp3AAqAAAqEAAqEAAqOAAqOAAqSAAqSAAqpAAqpAAqxAAqxAAq0AAq0AAq6AAq7AArGAArGAArKAArKAArOAArPAArRAArfAArkAArlAAryAAr4AAsAAAsAAAsEAAsEAAsNAAsOAAsRAAsSAAspAAspAAsxAAsxAAs0AAs0AAs6AAs7AAtFAAtGAAtJAAtKAAtOAAtUAAtYAAtbAAteAAteAAtkAAtlAAt4AAuBAAuEAAuEAAuLAAuNAAuRAAuRAAuWAAuYAAubAAubAAudAAudAAugAAuiAAulAAunAAurAAutAAu6AAu9AAvDAAvFAAvJAAvJAAvOAAvPAAvRAAvWAAvYAAvlAAv7AAv/AAwNAAwNAAwRAAwRAAwpAAwpAAw6AAw7AAxFAAxFAAxJAAxJAAxOAAxUAAxXAAxXAAxbAAxcAAxeAAxfAAxkAAxlAAxwAAx2AAyNAAyNAAyRAAyRAAypAAypAAy0AAy0AAy6AAy7AAzFAAzFAAzJAAzJAAzOAAzUAAzXAAzcAAzfAAzfAAzkAAzlAAzwAAzwAAz0AAz/AA0NAA0NAA0RAA0RAA1FAA1FAA1JAA1JAA1QAA1TAA1kAA1lAA2AAA2AAA2EAA2EAA2XAA2ZAA2yAA2yAA28AA28AA2+AA2/AA3HAA3JAA3LAA3OAA3VAA3VAA3XAA3XAA3gAA3lAA3wAA3xAA31AA4AAA47AA4+AA5cAA6AAA6DAA6DAA6FAA6FAA6LAA6LAA6kAA6kAA6mAA6mAA6+AA6/AA7FAA7FAA7HAA7HAA7PAA7PAA7aAA7bAA7gAA7/AA9IAA9IAA9tAA9wAA+YAA+YAA+9AA+9AA/NAA/NAA/bAA//ABDGABDGABDIABDMABDOABDPABJJABJJABJOABJPABJXABJXABJZABJZABJeABJfABKJABKJABKOABKPABKxABKxABK2ABK3ABK/ABK/ABLBABLBABLGABLHABLXABLXABMRABMRABMWABMXABNbABNcABN9ABN/ABOaABOfABP2ABP3ABP+ABP/ABaAABaAABadABafABb5ABb/ABcWABceABc3ABc/ABdUABdfABdtABdtABdxABdxABd0ABd/ABfeABffABfqABfvABf6ABf/ABgOABgOABgaABgfABh5ABh/ABirABivABj2ABj/ABkfABkfABksABkvABk8ABk/ABlBABlDABluABlvABl1ABl/ABmsABmvABnKABnPABnbABndABocABodABpfABpfABp9ABp+ABqKABqPABqaABqfABquABqvABrPABr/ABtNABtPABt/ABt/ABv0ABv7ABw4ABw6ABxKABxMAByJAByPABy7ABy8ABzIABzPABz7ABz/AB8WAB8XAB8eAB8fAB9GAB9HAB9OAB9PAB9YAB9YAB9aAB9aAB9cAB9cAB9eAB9eAB9+AB9/AB+1AB+1AB/FAB/FAB/UAB/VAB/cAB/cAB/wAB/xAB/1AB/1AB//ACAPACAoACAvACBfACBvACByACBzACCPACCPACCdACCfACDBACDPACDxACD/ACGMACGPACQnACQ/ACRLACRfACt0ACt1ACuWACuWACz0ACz4AC0mAC0mAC0oAC0sAC0uAC0vAC1oAC1uAC1xAC1+AC2XAC2fAC2nAC2nAC2vAC2vAC23AC23AC2/AC2/AC3HAC3HAC3PAC3PAC3XAC3XAC3fAC3fAC5eAC5/AC6aAC6aAC70AC7/AC/WAC/vAC/8ADAAADBAADBAADCXADCYADEAADEEADEwADEwADGPADGPADHkADHvADIfADIfAKSNAKSPAKTHAKTPAKYsAKY/AKb4AKb/AKfLAKfPAKfSAKfSAKfUAKfUAKfaAKfxAKgtAKgvAKg6AKg/AKh4AKh/AKjGAKjNAKjaAKjfAKlUAKleAKl9AKl/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";

type Value =
  | { k: "null" }
  | { k: "bool"; v: boolean }
  | { k: "int"; v: bigint }
  | { k: "float"; v: number }
  | { k: "str"; v: string }
  | { k: "arr"; v: Value[] }
  | { k: "tuple"; v: Value[] }
  | { k: "obj"; v: Array<[string, Value]> };

class Fail extends Error {
  constructor(
    readonly code: number,
    readonly stderr: string,
    readonly stdout = "",
  ) {
    super(stderr);
  }
}

class ParseError extends Error {}

const NULL: Value = { k: "null" };

function str(value: string): Value {
  return { k: "str", v: value };
}

function int(value: number): Value {
  return { k: "int", v: BigInt(value) };
}

function arr(values: Value[]): Value {
  return { k: "arr", v: values };
}

function typeName(value: Value): string {
  switch (value.k) {
    case "null":
      return "NoneType";
    case "bool":
      return "bool";
    case "int":
      return "int";
    case "float":
      return "float";
    case "str":
      return "str";
    case "arr":
      return "list";
    case "tuple":
      return "tuple";
    case "obj":
      return "dict";
  }
}

function stdout(text: string) {
  writeSync(1, text);
}

function stderr(text: string) {
  writeSync(2, text);
}

const nonprintable = loadRanges(NONPRINTABLE_B64);

// An indexed element the caller has already bounds-checked (an in-range index, a regex
// group that always participates). Throws rather than letting undefined flow on.
function present<T>(value: T | undefined): T {
  if (value === undefined) throw new Error("indexed element is missing");
  return value;
}

function loadRanges(encoded: string): Uint32Array {
  const raw = Uint8Array.from(atob(encoded), (ch) => ch.charCodeAt(0));
  const pairs = raw.length / 6;
  const ranges = new Uint32Array(pairs * 2);
  const byte = (index: number) => present(raw[index]);
  for (let i = 0; i < pairs; i++) {
    const offset = i * 6;
    ranges[i * 2] = (byte(offset) << 16) | (byte(offset + 1) << 8) | byte(offset + 2);
    ranges[i * 2 + 1] = (byte(offset + 3) << 16) | (byte(offset + 4) << 8) | byte(offset + 5);
  }
  return ranges;
}

function isNonPrintable(codePoint: number): boolean {
  let lo = 0;
  let hi = nonprintable.length / 2;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const start = present(nonprintable[mid * 2]);
    const end = present(nonprintable[mid * 2 + 1]);
    if (codePoint < start) hi = mid;
    else if (codePoint > end) lo = mid + 1;
    else return true;
  }
  return false;
}

function hex(value: number, width: number): string {
  return value.toString(16).padStart(width, "0");
}

// The code point of one character from string iteration, which is never empty.
function codePointOf(ch: string | undefined): number {
  const codePoint = ch?.codePointAt(0);
  if (codePoint === undefined) throw new Error("code point of an empty character");
  return codePoint;
}

function cmpStr(left: string, right: string): number {
  const leftPoints = Array.from(left);
  const rightPoints = Array.from(right);
  const count = Math.min(leftPoints.length, rightPoints.length);
  for (let i = 0; i < count; i++) {
    const diff = codePointOf(leftPoints[i]) - codePointOf(rightPoints[i]);
    if (diff) return diff;
  }
  return leftPoints.length - rightPoints.length;
}

function pyStrRepr(value: string): string {
  const quote = value.includes("'") && !value.includes('"') ? '"' : "'";
  let out = quote;
  for (const ch of value) {
    const codePoint = codePointOf(ch);
    if (ch === "\\" || ch === quote) out += `\\${ch}`;
    else if (ch === "\t") out += "\\t";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (codePoint < 0x20 || codePoint === 0x7f || isNonPrintable(codePoint)) {
      if (codePoint < 0x100) out += `\\x${hex(codePoint, 2)}`;
      else if (codePoint < 0x10000) out += `\\u${hex(codePoint, 4)}`;
      else out += `\\U${hex(codePoint, 8)}`;
    } else out += ch;
  }
  return out + quote;
}

function jsonEscape(value: string): string {
  let out = '"';
  for (const ch of value) {
    const codePoint = codePointOf(ch);
    if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (ch === "\b") out += "\\b";
    else if (ch === "\f") out += "\\f";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch === "\t") out += "\\t";
    else if (codePoint < 0x20) out += `\\u${hex(codePoint, 4)}`;
    else if (codePoint > 0xffff) {
      const shifted = codePoint - 0x10000;
      out += `\\u${hex(0xd800 + (shifted >> 10), 4)}\\u${hex(0xdc00 + (shifted & 0x3ff), 4)}`;
    } else if (codePoint > 0x7f) out += `\\u${hex(codePoint, 4)}`;
    else out += ch;
  }
  return `${out}"`;
}

const pow5: bigint[] = [1n];
const pow10: bigint[] = [1n];

function p5(n: number): bigint {
  while (pow5.length <= n) pow5.push(present(pow5[pow5.length - 1]) * 5n);
  return present(pow5[n]);
}

function p10(n: number): bigint {
  while (pow10.length <= n) pow10.push(present(pow10[pow10.length - 1]) * 10n);
  return present(pow10[n]);
}

function components(n: number): { mant: bigint; e: number } {
  const buf = new DataView(new ArrayBuffer(8));
  buf.setFloat64(0, Math.abs(n));
  const bits = buf.getBigUint64(0);
  const exp = Number((bits >> 52n) & 0x7ffn);
  const frac = bits & ((1n << 52n) - 1n);
  if (exp === 0) return { mant: frac, e: -1022 - 52 };
  return { mant: frac | (1n << 52n), e: exp - 1023 - 52 };
}

function cmpPow10(mant: bigint, e: number, k: number): number {
  if (k >= 0) {
    if (e >= 0) {
      const left = mant << BigInt(e);
      const right = p10(k);
      return left < right ? -1 : left > right ? 1 : 0;
    }
    const right = p10(k) << BigInt(-e);
    return mant < right ? -1 : mant > right ? 1 : 0;
  }
  const nk = -k;
  const shift = e + nk;
  if (shift >= 0) {
    const left = (mant * p5(nk)) << BigInt(shift);
    return left < 1n ? -1 : left > 1n ? 1 : 0;
  }
  const left = mant * p5(nk);
  const right = 1n << BigInt(-shift);
  return left < right ? -1 : left > right ? 1 : 0;
}

function floorLog10(mant: bigint, e: number): number {
  const approx = Math.log10(Number(mant)) + e * Math.LOG10E * Math.LN2;
  let k = Math.floor(approx);
  if (!Number.isFinite(k)) k = 0;
  let guard = 0;
  while (cmpPow10(mant, e, k) < 0) {
    k--;
    if (++guard > 8) throw new Error("float exponent");
  }
  while (cmpPow10(mant, e, k + 1) >= 0) {
    k++;
    if (++guard > 8) throw new Error("float exponent");
  }
  return k;
}

function divRoundHalfEven(numer: bigint, denom: bigint): bigint {
  const q = numer / denom;
  const r = numer % denom;
  const twice = r * 2n;
  if (twice > denom) return q + 1n;
  if (twice < denom) return q;
  return q % 2n === 0n ? q : q + 1n;
}

function roundSig(
  mant: bigint,
  e: number,
  sig: number,
  exp10: number,
): { digits: bigint; exp10: number } {
  const p = exp10 - sig + 1;
  let numer: bigint;
  let denom: bigint;
  if (p >= 0) {
    if (e >= p) {
      numer = mant << BigInt(e - p);
      denom = p5(p);
    } else {
      numer = mant;
      denom = p5(p) << BigInt(p - e);
    }
  } else {
    const np = -p;
    if (e - p >= 0) {
      numer = (mant * p5(np)) << BigInt(e - p);
      denom = 1n;
    } else {
      numer = mant * p5(np);
      denom = 1n << BigInt(p - e);
    }
  }
  let digits = denom === 1n ? numer : divRoundHalfEven(numer, denom);
  let exp = exp10;
  if (digits >= p10(sig)) {
    digits /= 10n;
    exp += 1;
  }
  return { digits, exp10: exp };
}

function formatDigits(digits: bigint, sig: number, exp10: number): string {
  const body = digits.toString().padStart(sig, "0");
  if (exp10 >= 16 || exp10 <= -5) {
    const mantissa = body.length === 1 ? body : `${body.slice(0, 1)}.${body.slice(1)}`;
    const sign = exp10 < 0 ? "-" : "+";
    return `${mantissa}e${sign}${Math.abs(exp10).toString().padStart(2, "0")}`;
  }
  if (exp10 >= 0) {
    const point = exp10 + 1;
    if (body.length <= point) return `${body}${"0".repeat(point - body.length)}.0`;
    return `${body.slice(0, point)}.${body.slice(point)}`;
  }
  return `0.${"0".repeat(-exp10 - 1)}${body}`;
}

function pyFloatRepr(n: number): string {
  if (Object.is(n, -0)) return "-0.0";
  if (Number.isNaN(n)) return "nan";
  if (n === Infinity) return "inf";
  if (n === -Infinity) return "-inf";
  const sign = n < 0 ? "-" : "";
  const { mant, e } = components(n);
  if (mant === 0n) return `${sign}0.0`;
  const exp10 = floorLog10(mant, e);
  for (let sig = 1; sig <= 17; sig++) {
    const rounded = roundSig(mant, e, sig, exp10);
    const text = sign + formatDigits(rounded.digits, sig, rounded.exp10);
    if (Object.is(Number(text), n)) return text;
  }
  throw new Fail(1, `ValueError: cannot render float ${String(n)}\n`);
}

function pyRepr(value: Value): string {
  switch (value.k) {
    case "null":
      return "None";
    case "bool":
      return value.v ? "True" : "False";
    case "int":
      return value.v.toString();
    case "float":
      return pyFloatRepr(value.v);
    case "str":
      return pyStrRepr(value.v);
    case "arr":
      return `[${value.v.map(pyRepr).join(", ")}]`;
    case "tuple": {
      const inner = value.v.map(pyRepr).join(", ");
      return value.v.length === 1 ? `(${inner},)` : `(${inner})`;
    }
    case "obj":
      return `{${value.v.map(([key, item]) => `${pyStrRepr(key)}: ${pyRepr(item)}`).join(", ")}}`;
  }
}

function pyStr(value: Value): string {
  return value.k === "str" ? value.v : pyRepr(value);
}

function pyJson(value: Value): string {
  switch (value.k) {
    case "null":
      return "null";
    case "bool":
      return value.v ? "true" : "false";
    case "int":
      return value.v.toString();
    case "float":
      if (Number.isNaN(value.v)) return "NaN";
      if (value.v === Infinity) return "Infinity";
      if (value.v === -Infinity) return "-Infinity";
      return pyFloatRepr(value.v);
    case "str":
      return jsonEscape(value.v);
    case "arr":
    case "tuple":
      return `[${value.v.map(pyJson).join(", ")}]`;
    case "obj": {
      const entries = [...value.v].sort((left, right) => cmpStr(left[0], right[0]));
      return `{${entries.map(([key, item]) => `${jsonEscape(key)}: ${pyJson(item)}`).join(", ")}}`;
    }
  }
}

function floatToBigInt(n: number): bigint | null {
  if (!Number.isFinite(n)) return null;
  if (Object.is(n, -0)) return 0n;
  const { mant, e } = components(Math.abs(n));
  if (mant === 0n) return 0n;
  let exact: bigint;
  if (e >= 0) exact = mant << BigInt(e);
  else {
    const shift = BigInt(-e);
    if ((mant & ((1n << shift) - 1n)) !== 0n) return null;
    exact = mant >> shift;
  }
  return n < 0 ? -exact : exact;
}

function numericBig(value: Value): bigint | null {
  if (value.k === "int") return value.v;
  if (value.k === "bool") return value.v ? 1n : 0n;
  if (value.k === "float") return floatToBigInt(value.v);
  return null;
}

function isNumeric(value: Value): boolean {
  return value.k === "int" || value.k === "float" || value.k === "bool";
}

function pyEq(left: Value, right: Value): boolean {
  if (isNumeric(left) && isNumeric(right)) {
    if (left.k === "float" && right.k === "float") {
      if (Number.isNaN(left.v) || Number.isNaN(right.v)) return false;
      return left.v === right.v;
    }
    const leftInt = numericBig(left);
    const rightInt = numericBig(right);
    if (leftInt !== null && rightInt !== null) return leftInt === rightInt;
    return false;
  }
  if (left.k !== right.k) return false;
  switch (left.k) {
    case "null":
      return true;
    case "str":
      return right.k === "str" && left.v === right.v;
    case "arr":
    case "tuple":
      return (
        right.k === left.k &&
        left.v.length === right.v.length &&
        left.v.every((item, i) => pyEq(item, present(right.v[i])))
      );
    case "obj": {
      if (right.k !== "obj" || left.v.length !== right.v.length) return false;
      const other = new Map(right.v);
      if (other.size !== right.v.length) return false;
      return left.v.every(([key, item]) => {
        const found = other.get(key);
        return found !== undefined && pyEq(item, found);
      });
    }
    default:
      return false;
  }
}

function truthy(value: Value): boolean {
  switch (value.k) {
    case "null":
      return false;
    case "bool":
      return value.v;
    case "int":
      return value.v !== 0n;
    case "float":
      return Number.isNaN(value.v) || value.v !== 0;
    case "str":
      return value.v.length > 0;
    case "arr":
    case "tuple":
    case "obj":
      return value.v.length > 0;
  }
}

function isTrue(value: Value): boolean {
  return value.k === "bool" && value.v;
}

function isFalse(value: Value): boolean {
  return value.k === "bool" && !value.v;
}

function cmpPy(left: Value, right: Value): number | null {
  if (left.k === "null" && right.k === "null") return 0;
  if (left.k === "str" && right.k === "str") return cmpStr(left.v, right.v);
  if (isNumeric(left) && isNumeric(right)) {
    const leftInt = numericBig(left);
    const rightInt = numericBig(right);
    if (leftInt === null || rightInt === null) return null;
    if (leftInt < rightInt) return -1;
    if (leftInt > rightInt) return 1;
    return 0;
  }
  if ((left.k === "arr" || left.k === "tuple") && left.k === right.k) {
    const count = Math.min(left.v.length, right.v.length);
    for (let i = 0; i < count; i++) {
      const diff = cmpPy(present(left.v[i]), present(right.v[i]));
      if (diff === null) return null;
      if (diff) return diff;
    }
    return left.v.length - right.v.length;
  }
  if (pyEq(left, right)) return 0;
  return null;
}

function cmpOrThrow(left: Value, right: Value): number {
  const diff = cmpPy(left, right);
  if (diff === null) {
    throw new Fail(
      1,
      `TypeError: '<' not supported between instances of '${typeName(left)}' and '${typeName(right)}'\n`,
    );
  }
  return diff;
}

function objGet(value: Value, key: string): Value | undefined {
  if (value.k !== "obj")
    throw new Fail(1, `AttributeError: '${typeName(value)}' object has no attribute 'get'\n`);
  return value.v.find(([name]) => name === key)?.[1];
}

function getOrNull(value: Value, key: string): Value {
  return objGet(value, key) ?? NULL;
}

function hasKey(value: Value, key: string): boolean {
  if (value.k !== "obj")
    throw new Fail(1, `TypeError: argument of type '${typeName(value)}' is not iterable\n`);
  return value.v.some(([name]) => name === key);
}

function requireKey(value: Value, key: string): Value {
  if (value.k !== "obj")
    throw new Fail(1, `TypeError: '${typeName(value)}' object is not subscriptable\n`);
  const found = value.v.find(([name]) => name === key);
  if (!found) throw new Fail(1, `KeyError: ${pyStrRepr(key)}\n`);
  return found[1];
}

function asArray(value: Value): Value[] {
  if (value.k !== "arr")
    throw new Fail(1, `TypeError: '${typeName(value)}' object is not iterable\n`);
  return value.v;
}

function orArray(value: Value): Value[] {
  if (!truthy(value)) return [];
  return asArray(value);
}

function sameOrdered(left: string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((item, index) => item === right[index]);
}

function sameStrings(left: string[], right: readonly string[]): boolean {
  if (left.length !== right.length) return false;
  const seen = new Set(right);
  return seen.size === right.length && left.every((item) => seen.has(item));
}

function stringList(values: readonly string[]): string {
  return pyRepr(arr([...values].map(str)));
}

function tupleList(rows: string[][]): string {
  return pyRepr(arr(rows.map((row) => ({ k: "tuple", v: row.map(str) }))));
}

class JsonParser {
  private i = 0;

  constructor(private readonly s: string) {}

  parseDocument(): Value {
    if (this.s.charCodeAt(0) === 0xfeff) throw new ParseError("unexpected UTF-8 BOM");
    this.skip();
    if (this.i >= this.s.length) throw new ParseError("expecting value");
    const value = this.parseValue();
    this.skip();
    if (this.i < this.s.length) throw new ParseError("extra data");
    return value;
  }

  private parseValue(): Value {
    this.skip();
    if (this.i >= this.s.length) throw new ParseError("expecting value");
    const ch = this.s[this.i];
    if (ch === "{") return this.parseObject();
    if (ch === "[") return this.parseArray();
    if (ch === '"') return str(this.parseString());
    if (this.take("true")) return { k: "bool", v: true };
    if (this.take("false")) return { k: "bool", v: false };
    if (this.take("null")) return NULL;
    if (this.take("NaN")) return { k: "float", v: Number.NaN };
    if (this.take("Infinity")) return { k: "float", v: Infinity };
    if (this.take("-Infinity")) return { k: "float", v: -Infinity };
    if (ch === "-" || this.isDigit(this.i)) return this.parseNumber();
    throw new ParseError("expecting value");
  }

  private parseObject(): Value {
    this.i++;
    this.skip();
    const entries: Array<[string, Value]> = [];
    if (this.s[this.i] === "}") {
      this.i++;
      return { k: "obj", v: entries };
    }
    while (this.i < this.s.length) {
      this.skip();
      if (this.s[this.i] !== '"') throw new ParseError("expecting property name");
      const key = this.parseString();
      this.skip();
      if (this.s[this.i] !== ":") throw new ParseError("expecting ':'");
      this.i++;
      const item = this.parseValue();
      const at = entries.findIndex(([name]) => name === key);
      if (at >= 0) present(entries[at])[1] = item;
      else entries.push([key, item]);
      this.skip();
      if (this.s[this.i] === ",") {
        this.i++;
        continue;
      }
      if (this.s[this.i] === "}") {
        this.i++;
        return { k: "obj", v: entries };
      }
      throw new ParseError("expecting ',' or '}'");
    }
    throw new ParseError("expecting ',' or '}'");
  }

  private parseArray(): Value {
    this.i++;
    this.skip();
    const items: Value[] = [];
    if (this.s[this.i] === "]") {
      this.i++;
      return arr(items);
    }
    while (this.i < this.s.length) {
      items.push(this.parseValue());
      this.skip();
      if (this.s[this.i] === ",") {
        this.i++;
        continue;
      }
      if (this.s[this.i] === "]") {
        this.i++;
        return arr(items);
      }
      throw new ParseError("expecting ',' or ']'");
    }
    throw new ParseError("expecting ',' or ']'");
  }

  private parseString(): string {
    this.i++;
    let out = "";
    while (this.i < this.s.length) {
      const ch = this.s.charAt(this.i);
      if (ch === '"') {
        this.i++;
        return out;
      }
      if (this.s.charCodeAt(this.i) < 0x20) throw new ParseError("invalid control character");
      if (ch !== "\\") {
        out += ch;
        this.i++;
        continue;
      }
      const esc = this.s[this.i + 1];
      if (esc === undefined) throw new ParseError("unterminated string");
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
      const replacement = simple[esc];
      if (replacement !== undefined) {
        out += replacement;
        this.i += 2;
        continue;
      }
      if (esc !== "u") throw new ParseError("invalid escape");
      const hexits = this.s.slice(this.i + 2, this.i + 6);
      if (!/^[0-9a-fA-F]{4}$/.test(hexits)) throw new ParseError("invalid unicode escape");
      let code = Number.parseInt(hexits, 16);
      this.i += 6;
      if (code >= 0xd800 && code <= 0xdbff && this.s.startsWith("\\u", this.i)) {
        const lowHex = this.s.slice(this.i + 2, this.i + 6);
        if (!/^[0-9a-fA-F]{4}$/.test(lowHex)) throw new ParseError("invalid unicode escape");
        const low = Number.parseInt(lowHex, 16);
        if (low >= 0xdc00 && low <= 0xdfff) {
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
          this.i += 6;
        }
      }
      out += code > 0xffff ? String.fromCodePoint(code) : String.fromCharCode(code);
    }
    throw new ParseError("unterminated string");
  }

  private parseNumber(): Value {
    const start = this.i;
    if (this.s[this.i] === "-") this.i++;
    if (!this.isDigit(this.i)) throw new ParseError("expecting value");
    if (this.s[this.i] === "0") this.i++;
    else while (this.isDigit(this.i)) this.i++;
    let isFloat = false;
    if (this.s[this.i] === "." && this.isDigit(this.i + 1)) {
      isFloat = true;
      this.i++;
      while (this.isDigit(this.i)) this.i++;
    }
    if (this.s[this.i] === "e" || this.s[this.i] === "E") {
      let k = this.i + 1;
      if (this.s[k] === "+" || this.s[k] === "-") k++;
      if (this.isDigit(k)) {
        isFloat = true;
        this.i = k + 1;
        while (this.isDigit(this.i)) this.i++;
      }
    }
    const token = this.s.slice(start, this.i);
    if (!isFloat) {
      // CPython's default sys.get_int_max_str_digits() is 4300. The sign is not a digit.
      // A longer integer token raises during json.loads and the comparison must not continue.
      const digits = token.startsWith("-") ? token.length - 1 : token.length;
      if (digits > 4300) {
        throw new ParseError(
          `Exceeds the limit (4300 digits) for integer string conversion: value has ${String(digits)} digits; use sys.set_int_max_str_digits() to increase the limit`,
        );
      }
      return { k: "int", v: BigInt(token) };
    }
    return { k: "float", v: Number(token) };
  }

  private take(word: string): boolean {
    if (!this.s.startsWith(word, this.i)) return false;
    this.i += word.length;
    return true;
  }

  private isDigit(index: number): boolean {
    const ch = this.s[index];
    return ch !== undefined && ch >= "0" && ch <= "9";
  }

  private skip() {
    while (this.i < this.s.length) {
      const ch = this.s.charAt(this.i);
      if (ch !== " " && ch !== "\n" && ch !== "\r" && ch !== "\t") break;
      this.i++;
    }
  }
}

function parseJson(text: string): Value {
  return new JsonParser(text).parseDocument();
}

function indexBy(items: Value[], key: string): Array<{ name: Value; item: Value }> {
  const out: Array<{ name: Value; item: Value }> = [];
  for (const item of items) {
    const name = requireKey(item, key);
    if (name.k === "arr" || name.k === "obj") {
      throw new Fail(1, `TypeError: unhashable type: '${name.k === "arr" ? "list" : "dict"}'\n`);
    }
    if (out.some((entry) => pyEq(entry.name, name)))
      throw new Fail(1, `duplicate ${key} ${pyRepr(name)}\n`);
    out.push({ name, item });
  }
  return out;
}

function diffLists(
  label: string,
  oldItems: Value[],
  newItems: Value[],
  key: string,
  report: string[],
  nested?: Record<string, string>,
) {
  const oldMap = indexBy(oldItems, key);
  const newMap = indexBy(newItems, key);
  const missing = oldMap
    .filter((entry) => !newMap.some((other) => pyEq(other.name, entry.name)))
    .sort((a, b) => cmpOrThrow(a.name, b.name));
  const extra = newMap
    .filter((entry) => !oldMap.some((other) => pyEq(other.name, entry.name)))
    .sort((a, b) => cmpOrThrow(a.name, b.name));
  for (const entry of missing) report.push(`MISSING ${label} ${pyStr(entry.name)}`);
  for (const entry of extra) report.push(`EXTRA ${label} ${pyStr(entry.name)}`);
  const shared = oldMap
    .filter((entry) => newMap.some((other) => pyEq(other.name, entry.name)))
    .sort((a, b) => cmpOrThrow(a.name, b.name));
  for (const entry of shared) {
    const other = newMap.find((candidate) => pyEq(candidate.name, entry.name));
    if (!other) continue;
    const fields = [...new Set([...objectKeys(entry.item), ...objectKeys(other.item)])].sort(
      cmpStr,
    );
    for (const field of fields) {
      if (nested && field in nested) {
        diffLists(
          `${label} ${pyStr(entry.name)}.${field}`,
          orArray(getOrNull(entry.item, field)),
          orArray(getOrNull(other.item, field)),
          present(nested[field]),
          report,
        );
        continue;
      }
      const oldValue = getOrNull(entry.item, field);
      const newValue = getOrNull(other.item, field);
      if (!pyEq(oldValue, newValue))
        report.push(
          `DIFF ${label} ${pyStr(entry.name)}.${field}: old=${pyRepr(oldValue)} new=${pyRepr(newValue)}`,
        );
    }
  }
}

function objectKeys(value: Value): string[] {
  if (value.k !== "obj")
    throw new Fail(1, `TypeError: '${typeName(value)}' object is not iterable\n`);
  return value.v.map(([key]) => key);
}

function columnNames(table: Value): string[] {
  return asArray(requireKey(table, "columns")).map((column) => {
    const name = requireKey(column, "name");
    if (name.k !== "str") throw new Fail(1, `TypeError: column name is ${typeName(name)}\n`);
    return name.v;
  });
}

function validateLedgerTransition(
  oldTables: Value[],
  oldRows: Value | undefined,
  newTables: Value[],
  newRows: Value | undefined,
): string[] {
  const problems: string[] = [];
  const [oldTable] = oldTables;
  const [newTable] = newTables;
  if (
    oldTables.length === 1 &&
    oldTable !== undefined &&
    !sameOrdered(columnNames(oldTable), OLD_LEDGER_COLUMNS)
  ) {
    problems.push(
      `LEDGER old table columns ${stringList(columnNames(oldTable))} are not the retired shape ${stringList(OLD_LEDGER_COLUMNS)}`,
    );
  }
  if (newTables.length === 1 && newTable !== undefined) {
    const names = columnNames(newTable);
    if (!sameOrdered(names, NEW_LEDGER_COLUMNS)) {
      problems.push(
        `LEDGER new table columns ${stringList(names)} are not ${stringList(NEW_LEDGER_COLUMNS)}`,
      );
    }
    const notnull = new Map<string, Value>();
    for (const column of asArray(requireKey(newTable, "columns"))) {
      const name = requireKey(column, "name");
      if (name.k !== "str") throw new Fail(1, `TypeError: column name is ${typeName(name)}\n`);
      notnull.set(name.v, requireKey(column, "notnull"));
    }
    for (const name of NEW_LEDGER_COLUMNS) {
      if (!truthy(notnull.get(name) ?? { k: "bool", v: false }))
        problems.push(`LEDGER new table column ${name} must be NOT NULL`);
    }
  }
  if (newRows === undefined || newRows.k !== "arr" || newRows.v.length === 0) {
    problems.push("LEDGER new rows missing");
    return problems;
  }
  const versions = newRows.v.map((row) => getOrNull(row, "version"));
  const expected = Array.from({ length: newRows.v.length }, (_, index) => int(index + 1));
  if (!versions.every((version, index) => pyEq(version, present(expected[index])))) {
    problems.push(`LEDGER new receipts are not contiguous from 1: ${pyRepr(arr(versions))}`);
  }
  const lineages: Value[] = [];
  for (const row of newRows.v) {
    const lineage = getOrNull(row, "lineage");
    if (!lineages.some((item) => pyEq(item, lineage))) lineages.push(lineage);
  }
  const sorted = [...lineages].sort((left, right) => cmpOrThrow(left, right));
  if (!(sorted.length === 1 && pyEq(present(sorted[0]), str(NEW_LINEAGE)))) {
    problems.push(
      `LEDGER new receipts carry lineage(s) ${pyRepr(arr(sorted))}, expected [${pyRepr(str(NEW_LINEAGE))}]`,
    );
  }
  for (const row of newRows.v) {
    const digest = getOrNull(row, "sql_sha256");
    if (digest.k !== "str" || !HEX64.test(digest.v)) {
      problems.push(
        `LEDGER new receipt ${pyStr(getOrNull(row, "version"))} has no 64-hex sql_sha256: ${pyRepr(digest)}`,
      );
    }
  }
  const digests = newRows.v.map((row) => getOrNull(row, "sql_sha256"));
  const distinct: Value[] = [];
  for (const digest of digests)
    if (!distinct.some((item) => pyEq(item, digest))) distinct.push(digest);
  if (distinct.length !== newRows.v.length) problems.push("LEDGER new receipts repeat a digest");
  if (oldRows !== undefined && oldRows.k === "arr") {
    for (const row of oldRows.v) {
      if (row.k === "obj" && hasKey(row, "lineage")) {
        problems.push(
          `LEDGER old receipt ${pyStr(getOrNull(row, "version"))} carries a lineage; the old side must be the retired shape`,
        );
        break;
      }
      if (row.k !== "obj")
        throw new Fail(1, `TypeError: argument of type '${typeName(row)}' is not iterable\n`);
    }
  }
  return problems;
}

function appRoleName(catalog: Value): Value | undefined {
  if (!hasKey(catalog, "app_role")) return undefined;
  const raw = objGet(catalog, "app_role") ?? NULL;
  const role = truthy(raw) ? raw : { k: "obj" as const, v: [] };
  if (role.k !== "obj")
    throw new Fail(1, `AttributeError: '${typeName(role)}' object has no attribute 'get'\n`);
  const name = objGet(role, "role");
  if (name === undefined || name.k === "null") return undefined;
  return name;
}

// Python str.strip() uses Unicode whitespace from str.isspace(). U+FEFF is not in that set.
const PYTHON_SPACE = new Set<number>([
  0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x001c, 0x001d, 0x001e, 0x001f, 0x0020, 0x0085, 0x00a0,
  0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a,
  0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
]);

function pythonStrip(value: string): string {
  const chars = Array.from(value);
  let start = 0;
  let end = chars.length;
  while (start < end && PYTHON_SPACE.has(codePointOf(chars[start]))) start++;
  while (end > start && PYTHON_SPACE.has(codePointOf(chars[end - 1]))) end--;
  return chars.slice(start, end).join("");
}

function parseAcl(
  acl: Value,
): { entries: Array<{ grantee: string; privs: string; grantor: string }> } | { error: string } {
  if (acl.k === "null") return { entries: [] };
  if (acl.k !== "str")
    throw new Fail(1, `AttributeError: '${typeName(acl)}' object has no attribute 'strip'\n`);
  const text = pythonStrip(acl.v);
  if (text.includes('"') || !text.startsWith("{") || !text.endsWith("}"))
    return { error: `unparsable acl ${pyRepr(acl)}` };
  const entries: Array<{ grantee: string; privs: string; grantor: string }> = [];
  for (const item of text
    .slice(1, -1)
    .split(",")
    .filter((part) => part.length > 0)) {
    const match = /^([^=]*)=([arwdDxtmXUCTc*]*)\/(.+)$/.exec(item);
    if (!match) return { error: `unparsable acl entry ${pyStrRepr(item)} in ${pyRepr(acl)}` };
    entries.push({
      grantee: present(match[1]),
      privs: present(match[2]),
      grantor: present(match[3]),
    });
  }
  return { entries };
}

function coversOwnerPrivileges(privs: string): boolean {
  return Array.from("arwdD").every((ch) => privs.includes(ch));
}

function columnMap(table: Value): Map<string, Value> {
  const columns = new Map<string, Value>();
  for (const column of asArray(requireKey(table, "columns"))) {
    const name = requireKey(column, "name");
    if (name.k !== "str") throw new Fail(1, `TypeError: column name is ${typeName(name)}\n`);
    columns.set(name.v, column);
  }
  return columns;
}

function validateLedgerTableIdentity(
  oldLedger: Value[],
  newLedger: Value[],
  appRole: Value | undefined,
): string[] {
  if (oldLedger.length !== 1 || newLedger.length !== 1) return [];
  const problems: string[] = [];
  const oldTable = present(oldLedger[0]);
  const newTable = present(newLedger[0]);
  for (const field of ["kind", "rls", "force_rls", "acl", "triggers", "policies"]) {
    const oldValue = getOrNull(oldTable, field);
    const newValue = getOrNull(newTable, field);
    if (!pyEq(oldValue, newValue))
      problems.push(
        `LEDGER TABLE ${field} differs: old=${pyRepr(oldValue)} new=${pyRepr(newValue)}`,
      );
  }
  const oldColumns = columnMap(oldTable);
  const newColumns = columnMap(newTable);
  for (const name of [...oldColumns.keys()].filter((key) => newColumns.has(key)).sort(cmpStr)) {
    const oldColumn = oldColumns.get(name);
    const newColumn = newColumns.get(name);
    if (oldColumn === undefined || newColumn === undefined) continue;
    const oldAcl = getOrNull(oldColumn, "acl");
    const newAcl = getOrNull(newColumn, "acl");
    if (!pyEq(oldAcl, newAcl))
      problems.push(
        `LEDGER TABLE column ${name} acl differs: old=${pyRepr(oldAcl)} new=${pyRepr(newAcl)}`,
      );
  }
  for (const [side, columns] of [
    ["old", oldColumns],
    ["new", newColumns],
  ] as const) {
    for (const [name, column] of [...columns.entries()].sort((left, right) =>
      cmpStr(left[0], right[0]),
    )) {
      const acl = getOrNull(column, "acl");
      if (acl.k !== "null")
        problems.push(
          `LEDGER TABLE ${side} column ${name} carries a column acl ${pyRepr(acl)}; the normal ledger has none`,
        );
    }
  }
  for (const [side, table] of [
    ["old", oldTable],
    ["new", newTable],
  ] as const) {
    const parsed = parseAcl(getOrNull(table, "acl"));
    if ("error" in parsed) {
      problems.push(`LEDGER TABLE ${side} ${parsed.error}`);
      continue;
    }
    if (parsed.entries.length === 0) {
      problems.push(`LEDGER TABLE ${side} has no acl; the app role must hold SELECT`);
      continue;
    }
    const owners = parsed.entries
      .filter((entry) => entry.grantee === entry.grantor && coversOwnerPrivileges(entry.privs))
      .map((entry) => entry.grantee);
    if (owners.length !== 1) {
      problems.push(
        `LEDGER TABLE ${side} acl owner entries ${stringList(owners)} (expected exactly one self-granted full entry)`,
      );
    }
    const onlyOwners = parsed.entries.every((item) => owners.includes(item.grantee));
    for (const entry of parsed.entries) {
      const isOwner = owners.includes(entry.grantee);
      const isApp = appRole !== undefined && pyEq(str(entry.grantee), appRole);
      if (isOwner && (isApp || (appRole === undefined && onlyOwners))) {
        problems.push(
          `LEDGER TABLE ${side} acl grants ${pyStrRepr(entry.grantee)} ${pyStrRepr(entry.privs)}; app role must not hold owner write`,
        );
        continue;
      }
      if (isOwner) continue;
      if (entry.grantee === "")
        problems.push(`LEDGER TABLE ${side} acl grants PUBLIC ${pyStrRepr(entry.privs)}`);
      else if (appRole === undefined || !isApp) {
        problems.push(
          `LEDGER TABLE ${side} acl grants foreign grantee ${pyStrRepr(entry.grantee)} ${pyStrRepr(entry.privs)}`,
        );
      } else if (entry.privs !== "r") {
        problems.push(
          `LEDGER TABLE ${side} acl grants ${pyStrRepr(entry.grantee)} ${pyStrRepr(entry.privs)}; expected exactly 'r' (read, no grant option)`,
        );
      }
      if (owners.length > 0 && entry.grantor !== owners[0]) {
        problems.push(
          `LEDGER TABLE ${side} acl entry for ${pyStrRepr(entry.grantee)} granted by ${pyStrRepr(entry.grantor)}, not the owner`,
        );
      }
    }
    if (
      appRole !== undefined &&
      !parsed.entries.some((entry) => pyEq(str(entry.grantee), appRole))
    ) {
      problems.push(`LEDGER TABLE ${side} acl has no entry for the app role ${pyRepr(appRole)}`);
    }
  }
  return problems;
}

function validateRoleIdentity(oldRole: Value, newRole: Value): string[] {
  const problems: string[] = [];
  for (const [side, role] of [
    ["old", oldRole],
    ["new", newRole],
  ] as const) {
    const keys = objectKeys(role);
    if (!sameStrings(keys, ROLE_KEYS)) {
      const differ = [
        ...keys.filter((key) => !ROLE_KEYS.includes(key)),
        ...ROLE_KEYS.filter((key) => !keys.includes(key)),
      ].sort(cmpStr);
      problems.push(`ROLE ${side} metadata keys ${stringList(differ)} missing/unexpected`);
    }
    const attributes = getOrNull(role, "attributes");
    const attributeKeys = attributes.k === "obj" ? objectKeys(attributes) : [];
    if (
      attributes.k !== "obj" ||
      !sameStrings(attributeKeys, ATTRIBUTE_KEYS) ||
      !isTrue(getOrNull(attributes, "exists"))
    ) {
      problems.push(`ROLE ${side} attributes incomplete or role absent: ${pyRepr(attributes)}`);
    } else {
      for (const flag of ELEVATED_FLAGS) {
        const value = getOrNull(attributes, flag);
        if (!isFalse(value))
          problems.push(`ROLE ${side} app role has ${flag}=${pyRepr(value)}; an app role must not`);
      }
      const members = getOrNull(attributes, "member_of");
      if (truthy(members))
        problems.push(`ROLE ${side} app role is a member of ${pyStr(members)}; expected none`);
    }
    for (const [field, expectedKeys] of [
      ["table_privileges", TABLE_PRIV_KEYS],
      ["column_privileges", COLUMN_PRIV_KEYS],
    ] as const) {
      const rows = orArray(getOrNull(role, field));
      for (const row of rows) {
        if (!sameStrings(objectKeys(row), expectedKeys)) {
          problems.push(
            `ROLE ${side} ${field} row ${pyRepr(row)} metadata keys differ from ${stringList([...expectedKeys].sort(cmpStr))}`,
          );
          break;
        }
      }
      const grantables: Value[] = [];
      for (const row of rows) {
        const grantable = getOrNull(row, "grantable");
        if (!grantables.some((item) => pyEq(item, grantable))) grantables.push(grantable);
      }
      if (grantables.some((item) => !(item.k === "str" && (item.v === "NO" || item.v === "YES")))) {
        problems.push(
          `ROLE ${side} ${field} grantability values ${stringList(grantables.map(pyStr).sort(cmpStr))} are not NO/YES`,
        );
      }
    }
  }
  const oldName = getOrNull(oldRole, "role");
  const newName = getOrNull(newRole, "role");
  if (!pyEq(oldName, newName))
    problems.push(`ROLE identity differs: old=${pyRepr(oldName)} new=${pyRepr(newName)}`);
  const oldAttributes = getOrNull(oldRole, "attributes");
  const newAttributes = getOrNull(newRole, "attributes");
  if (!pyEq(oldAttributes, newAttributes))
    problems.push(
      `ROLE attributes differ: old=${pyRepr(oldAttributes)} new=${pyRepr(newAttributes)}`,
    );
  const oldSchema = getOrNull(oldRole, "schema_privileges");
  const newSchema = getOrNull(newRole, "schema_privileges");
  if (!pyEq(oldSchema, newSchema))
    problems.push(
      `DIFF app_role.schema_privileges old=${pyRepr(oldSchema)} new=${pyRepr(newSchema)}`,
    );
  return problems;
}

function isLedgerRow(row: Value): boolean {
  return (
    pyEq(getOrNull(row, "schema"), str("fvoci")) && pyEq(getOrNull(row, "table"), str(LEDGER_TABLE))
  );
}

function sortedPairs(rows: string[][]): string[][] {
  return [...rows].sort((left, right) => {
    const count = Math.min(left.length, right.length);
    for (let i = 0; i < count; i++) {
      const diff = cmpStr(present(left[i]), present(right[i]));
      if (diff) return diff;
    }
    return left.length - right.length;
  });
}

function samePairs(left: string[][], right: string[][]): boolean {
  return (
    left.length === right.length &&
    left.every((row, index) => {
      const other = present(right[index]);
      return (
        row.length === other.length && row.every((cell, cellIndex) => cell === other[cellIndex])
      );
    })
  );
}

function validateLedgerPrivileges(oldRole: Value, newRole: Value): string[] {
  const problems: string[] = [];
  const grantors: { old?: string[]; new?: string[] } = {};
  for (const [side, role, columns] of [
    ["old", oldRole, OLD_LEDGER_COLUMNS],
    ["new", newRole, NEW_LEDGER_COLUMNS],
  ] as const) {
    const tableRows = orArray(getOrNull(role, "table_privileges"));
    const columnRows = orArray(getOrNull(role, "column_privileges"));
    const names = [
      ...new Set([...tableRows, ...columnRows].map((row) => pyStr(getOrNull(row, "grantor")))),
    ].sort(cmpStr);
    grantors[side] = names;
    if (names.length !== 1)
      problems.push(
        `LEDGER app_role ${side} privileges come from ${stringList(names)}, expected exactly one grantor`,
      );
    const tablePrivs = sortedPairs(
      tableRows
        .filter(isLedgerRow)
        .map((row) => [pyStr(getOrNull(row, "privilege")), pyStr(getOrNull(row, "grantable"))]),
    );
    if (!samePairs(tablePrivs, [["SELECT", "NO"]])) {
      problems.push(
        `LEDGER app_role ${side} table privileges on ${LEDGER_TABLE} are ${tupleList(tablePrivs)}, expected [('SELECT', 'NO')] only`,
      );
    }
    const columnPrivs = sortedPairs(
      columnRows
        .filter(isLedgerRow)
        .map((row) => [
          pyStr(getOrNull(row, "column")),
          pyStr(getOrNull(row, "privilege")),
          pyStr(getOrNull(row, "grantable")),
        ]),
    );
    const expected = sortedPairs(columns.map((column) => [column, "SELECT", "NO"]));
    if (!samePairs(columnPrivs, expected)) {
      problems.push(
        `LEDGER app_role ${side} column privileges on ${LEDGER_TABLE} are ${tupleList(columnPrivs)}, expected ${tupleList(expected)}`,
      );
    }
  }
  const oldGrantors = present(grantors.old);
  const newGrantors = present(grantors.new);
  if (oldGrantors.join("\0") !== newGrantors.join("\0")) {
    problems.push(
      `LEDGER grantor differs across sides: old=${stringList(oldGrantors)} new=${stringList(newGrantors)}`,
    );
  }
  return problems;
}

function grantSet(role: Value, field: string): string[] {
  const rendered = new Set<string>();
  for (const row of orArray(getOrNull(role, field))) {
    if (!isLedgerRow(row)) rendered.add(pyJson(row));
  }
  return [...rendered].sort(cmpStr);
}

function onlyLedger(tables: Value[]): Value[] {
  return tables.filter((table) => pyEq(requireKey(table, "name"), str(LEDGER_TABLE)));
}

function exceptLedger(tables: Value[]): Value[] {
  return tables.filter((table) => !pyEq(requireKey(table, "name"), str(LEDGER_TABLE)));
}

function columnNameValues(table: Value): Value[] {
  return asArray(requireKey(table, "columns")).map((column) => requireKey(column, "name"));
}

function compare(
  oldPath: string,
  newPath: string,
  oldCatalog: Value,
  newCatalog: Value,
): { text: string; code: number } {
  if (oldCatalog.k !== "obj")
    throw new Fail(1, `AttributeError: '${typeName(oldCatalog)}' object has no attribute 'get'\n`);
  if (newCatalog.k !== "obj")
    throw new Fail(1, `AttributeError: '${typeName(newCatalog)}' object has no attribute 'get'\n`);
  const report: string[] = [];
  const oldVersion = getOrNull(oldCatalog, "server_version");
  const newVersion = getOrNull(newCatalog, "server_version");
  if (!pyEq(oldVersion, newVersion)) {
    report.push(
      `NOTE server_version old=${pyStr(oldVersion)} new=${pyStr(newVersion)} (same-version comparison expected)`,
    );
  }
  diffLists(
    "schema",
    asArray(requireKey(oldCatalog, "schemas")),
    asArray(requireKey(newCatalog, "schemas")),
    "name",
    report,
  );
  const oldTables = asArray(requireKey(oldCatalog, "tables"));
  const newTables = asArray(requireKey(newCatalog, "tables"));
  const oldLedger = onlyLedger(oldTables);
  const newLedger = onlyLedger(newTables);
  if (oldLedger.length !== 1 || newLedger.length !== 1) {
    report.push(
      `MISSING ledger table ${LEDGER_TABLE} on one side (old=${String(oldLedger.length)} new=${String(newLedger.length)})`,
    );
  }
  report.push(
    ...validateLedgerTransition(
      oldLedger,
      objGet(oldCatalog, "ledger"),
      newLedger,
      objGet(newCatalog, "ledger"),
    ),
  );
  report.push(...validateLedgerTableIdentity(oldLedger, newLedger, appRoleName(newCatalog)));
  const productOld = exceptLedger(oldTables);
  const productNew = exceptLedger(newTables);
  diffLists("table", productOld, productNew, "name", report, TABLE_NESTED);
  const indexedOld = indexBy(productOld, "name");
  const indexedNew = indexBy(productNew, "name");
  for (const entry of indexedOld
    .filter((item) => indexedNew.some((other) => pyEq(other.name, item.name)))
    .sort((a, b) => cmpOrThrow(a.name, b.name))) {
    const other = indexedNew.find((candidate) => pyEq(candidate.name, entry.name));
    if (!other) continue;
    const oldOrder = columnNameValues(entry.item);
    const newOrder = columnNameValues(other.item);
    if (
      oldOrder.length !== newOrder.length ||
      oldOrder.some((name, index) => !pyEq(name, present(newOrder[index])))
    ) {
      report.push(
        `COLUMN-ORDER table ${pyStr(entry.name)}: old=${pyRepr(arr(oldOrder))} new=${pyRepr(arr(newOrder))}`,
      );
    }
  }
  diffLists(
    "sequence",
    asArray(requireKey(oldCatalog, "sequences")),
    asArray(requireKey(newCatalog, "sequences")),
    "name",
    report,
  );
  diffLists(
    "function",
    asArray(requireKey(oldCatalog, "functions")),
    asArray(requireKey(newCatalog, "functions")),
    "signature",
    report,
  );
  diffLists(
    "view",
    asArray(requireKey(oldCatalog, "views")),
    asArray(requireKey(newCatalog, "views")),
    "name",
    report,
  );
  const oldExtensions = requireKey(oldCatalog, "extensions");
  const newExtensions = requireKey(newCatalog, "extensions");
  if (!pyEq(oldExtensions, newExtensions))
    report.push(`DIFF extensions old=${pyStr(oldExtensions)} new=${pyStr(newExtensions)}`);
  const oldSeeds = requireKey(oldCatalog, "seeds");
  const newSeeds = requireKey(newCatalog, "seeds");
  for (const seed of [...new Set([...objectKeys(oldSeeds), ...objectKeys(newSeeds)])].sort(
    cmpStr,
  )) {
    const oldSeed = getOrNull(oldSeeds, seed);
    const newSeed = getOrNull(newSeeds, seed);
    if (!pyEq(oldSeed, newSeed))
      report.push(`DIFF seed ${seed}: old=${pyRepr(oldSeed)} new=${pyRepr(newSeed)}`);
  }
  const oldHasRole = hasKey(oldCatalog, "app_role");
  const newHasRole = hasKey(newCatalog, "app_role");
  if (!oldHasRole) report.push("app_role missing on old");
  if (!newHasRole) report.push("app_role missing on new");
  if (oldHasRole && newHasRole) {
    const oldRole = requireKey(oldCatalog, "app_role");
    const newRole = requireKey(newCatalog, "app_role");
    report.push(...validateRoleIdentity(oldRole, newRole));
    report.push(...validateLedgerPrivileges(oldRole, newRole));
    for (const field of ["table_privileges", "column_privileges", "routine_privileges"]) {
      const oldGrants = new Set(grantSet(oldRole, field));
      const newGrants = new Set(grantSet(newRole, field));
      for (const item of [...oldGrants].filter((grant) => !newGrants.has(grant)).sort(cmpStr))
        report.push(`MISSING app_role.${field} ${item}`);
      for (const item of [...newGrants].filter((grant) => !oldGrants.has(grant)).sort(cmpStr))
        report.push(`EXTRA app_role.${field} ${item}`);
    }
    for (const field of ["sequence_usage", "schema_usage"]) {
      const oldValue = getOrNull(oldRole, field);
      const newValue = getOrNull(newRole, field);
      if (!pyEq(oldValue, newValue))
        report.push(`DIFF app_role.${field}: old=${pyRepr(oldValue)} new=${pyRepr(newValue)}`);
    }
  }
  const semantic = report.filter((line) => !line.startsWith("NOTE"));
  const ledgerNote = [
    `LEDGER TABLE (declared exception, printed not compared) old=${pyJson(arr(oldLedger))}`,
    `LEDGER TABLE (declared exception, printed not compared) new=${pyJson(arr(newLedger))}`,
    `LEDGER ROWS (declared exception, printed not compared) old=${pyStr(getOrNull(oldCatalog, "ledger"))} new=${pyStr(getOrNull(newCatalog, "ledger"))}`,
  ].join("\n");
  const text =
    [
      "# Catalog comparison",
      `old: ${oldPath}`,
      `new: ${newPath}`,
      `semantic differences: ${String(semantic.length)}`,
      "",
      ...report,
      "",
      ledgerNote,
      "",
      `RESULT: ${semantic.length === 0 ? "PASS" : "FAIL"}`,
    ].join("\n") + "\n";
  return { text, code: semantic.length === 0 ? 0 : 1 };
}

function ioFail(error: unknown, path: string): Fail {
  const code = typeof error === "object" && error && "code" in error ? String(error.code) : "";
  const shown = pyStrRepr(path);
  if (code === "ENOENT")
    return new Fail(1, `FileNotFoundError: [Errno 2] No such file or directory: ${shown}\n`);
  if (code === "EISDIR")
    return new Fail(1, `IsADirectoryError: [Errno 21] Is a directory: ${shown}\n`);
  if (code === "EACCES" || code === "EPERM")
    return new Fail(1, `PermissionError: [Errno 13] Permission denied: ${shown}\n`);
  const message = error instanceof Error ? error.message : String(error);
  return new Fail(1, `OSError: ${message}\n`);
}

function loadCatalog(path: string): Value {
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(path);
  } catch (error) {
    throw ioFail(error, path);
  }
  let text: string;
  try {
    // ignoreBOM keeps a leading U+FEFF so parseDocument rejects it, as json.load does
    // on a file opened with encoding="utf-8".
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    throw new Fail(1, `UnicodeDecodeError: 'utf-8' codec can't decode ${pyStrRepr(path)}\n`);
  }
  try {
    return parseJson(text);
  } catch (error) {
    if (error instanceof ParseError)
      throw new Fail(1, `JSONDecodeError: ${pyStrRepr(path)}: ${error.message}\n`);
    throw error;
  }
}

type Args = { old: string; new: string; report?: string };

function programName(): string {
  return basename(process.argv[1] ?? "compare-catalogs.ts");
}

function usageLine(prog: string): string {
  return `usage: ${prog} [-h] [--report REPORT] old new\n`;
}

function helpText(prog: string): string {
  return `${usageLine(prog)}
positional arguments:
  old
  new

options:
  -h, --help       show this help message and exit
  --report REPORT
`;
}

function isNegativeNumber(token: string): boolean {
  return /^-\d+(\.\d*)?([eE][+-]?\d+)?$/.test(token) || /^-\.\d+([eE][+-]?\d+)?$/.test(token);
}

function isOption(token: string): boolean {
  return token.length > 1 && token.startsWith("-") && !isNegativeNumber(token);
}

function isReportValue(token: string | undefined): token is string {
  return token !== undefined && token !== "--" && !isOption(token);
}

function parseArgs(argv: string[]): Args | "help" {
  const prog = programName();
  const rest = argv.slice(2);
  const positionals: string[] = [];
  const unknown: string[] = [];
  let report: string | undefined;
  let sawReport = false;
  const fail = (message: string) => {
    throw new Fail(2, `${usageLine(prog)}${prog}: error: ${message}\n`);
  };
  for (let i = 0; i < rest.length; i++) {
    const token = present(rest[i]);
    if (token === "--") {
      positionals.push(...rest.slice(i + 1));
      break;
    }
    if (token === "-h" || token === "--help") return "help";
    if (token === "--report" || token.startsWith("--report=")) {
      if (token.startsWith("--report=")) report = token.slice("--report=".length);
      else if (isReportValue(rest[i + 1])) report = rest[++i];
      else fail("argument --report: expected one argument");
      sawReport = true;
      continue;
    }
    if (isOption(token)) {
      unknown.push(token);
      continue;
    }
    positionals.push(token);
  }
  if (positionals.length < 2)
    fail(`the following arguments are required: ${positionals.length === 0 ? "old, new" : "new"}`);
  if (positionals.length > 2) unknown.push(...positionals.slice(2));
  if (unknown.length > 0) fail(`unrecognized arguments: ${unknown.join(" ")}`);
  return {
    old: present(positionals[0]),
    new: present(positionals[1]),
    ...(sawReport ? { report } : {}),
  };
}

function main(): number {
  const parsed = parseArgs(process.argv);
  if (parsed === "help") {
    stdout(helpText(programName()));
    return 0;
  }
  const oldCatalog = loadCatalog(parsed.old);
  const newCatalog = loadCatalog(parsed.new);
  const result = compare(parsed.old, parsed.new, oldCatalog, newCatalog);
  if (parsed.report !== undefined) {
    try {
      writeFileSync(parsed.report, result.text);
    } catch (error) {
      throw ioFail(error, parsed.report);
    }
  }
  stdout(result.text);
  return result.code;
}

if (import.meta.main) {
  try {
    process.exit(main());
  } catch (error) {
    if (error instanceof Fail) {
      if (error.stdout) stdout(error.stdout);
      if (error.stderr) stderr(error.stderr);
      process.exit(error.code);
    }
    stderr(`${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    process.exit(1);
  }
}
