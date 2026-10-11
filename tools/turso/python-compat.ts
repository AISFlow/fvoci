// Value semantics the guard's file contracts inherited from the original
// implementation that tools/web-e2e/compat.ts does not provide: dict()
// conversion, JSON equality, and json.dumps bytes with ensure_ascii's
// escaping of every character outside U+0020..U+007E (DEL included).
import { compareCodePoints } from "../web-e2e/compat.ts";

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export function isRecord(value: unknown): value is Record<string, Json> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Unicode decimal digit, the meaning `\d` had in the original patterns. */
export const DIGIT = "\\p{Nd}";

/** Structural equality of decoded JSON values. */
export function jsonEqual(left: unknown, right: unknown): boolean {
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((item, index) => jsonEqual(item, right[index]))
    );
  }
  if (isRecord(left) || isRecord(right)) {
    if (!isRecord(left) || !isRecord(right)) return false;
    const keys = Object.keys(left);
    return (
      keys.length === Object.keys(right).length &&
      keys.every((key) => Object.hasOwn(right, key) && jsonEqual(left[key], right[key]))
    );
  }
  return left === right;
}

/** dict(value) over a decoded JSON value; anything dict() refuses throws. */
export function toDict(value: unknown): Record<string, Json> {
  if (isRecord(value)) return { ...value };
  // Own data properties only: a "__proto__" input stays an inert key.
  const result = Object.create(null) as Record<string, Json>;
  if (typeof value === "string") {
    if (value !== "") throw new TypeError("dictionary update sequence element has length 1");
    return result;
  }
  if (!Array.isArray(value)) throw new TypeError("not iterable");
  for (const item of value) {
    let pair: Json[];
    if (typeof item === "string") pair = Array.from(item);
    else if (Array.isArray(item)) pair = item as Json[];
    else if (isRecord(item)) pair = Object.keys(item);
    else throw new TypeError("not iterable");
    if (pair.length !== 2)
      throw new TypeError("dictionary update sequence element has wrong length");
    const [key, entry] = pair as [Json, Json];
    if (Array.isArray(key) || isRecord(key)) throw new TypeError("unhashable");
    // Only string keys can ever be looked up; other hashable keys are inert.
    if (typeof key === "string") result[key] = entry;
  }
  return result;
}

function quote(text: string): string {
  let out = '"';
  for (let index = 0; index < text.length; index += 1) {
    const unit = text.charCodeAt(index);
    const char = text[index] as string;
    if (char === '"') out += '\\"';
    else if (char === "\\") out += "\\\\";
    else if (char === "\n") out += "\\n";
    else if (char === "\r") out += "\\r";
    else if (char === "\t") out += "\\t";
    else if (char === "\b") out += "\\b";
    else if (char === "\f") out += "\\f";
    else if (unit < 0x20 || unit > 0x7e) out += "\\u" + unit.toString(16).padStart(4, "0");
    else out += char;
  }
  return out + '"';
}

/**
 * json.dumps(value[, sort_keys]) with default separators and ensure_ascii.
 * Numbers are integers in every guard file; a non-integer uses JS formatting.
 */
export function dumps(value: Json, sortKeys = false): string {
  if (value === null) return "null";
  if (value === true) return "true";
  if (value === false) return "false";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return quote(value);
  if (Array.isArray(value))
    return "[" + value.map((item) => dumps(item, sortKeys)).join(", ") + "]";
  const keys = Object.keys(value);
  if (sortKeys) keys.sort(compareCodePoints);
  return (
    "{" +
    keys.map((key) => quote(key) + ": " + dumps(value[key] as Json, sortKeys)).join(", ") +
    "}"
  );
}
