// Text helpers shared by the gate checks.

// The planner's blank test strips the characters Python's str.isspace accepts.
// That set differs from String.prototype.trim (it adds U+001C-U+001F and
// U+0085, and leaves U+FEFF), so a blank output is classified the same way.
// Same class as PY_SPACE in tools/web-e2e/compat.ts. That file belongs to the
// browser harness, so the gate keeps its own copy until one shared module exists.
const PY_SPACE =
  "\\t\\n\\v\\f\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const EDGES = new RegExp(`^[${PY_SPACE}]+|[${PY_SPACE}]+$`, "gu");

export function strip(value: string): string {
  return value.replace(EDGES, "");
}

export function isBlank(value: string): boolean {
  return strip(value) === "";
}

export type Parsed = { ok: true; value: unknown } | { ok: false };

export function parseJson(text: string): Parsed {
  try {
    return { ok: true, value: JSON.parse(text) as unknown };
  } catch {
    return { ok: false };
  }
}
