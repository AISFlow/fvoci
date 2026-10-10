// Text helpers shared by the gate checks.

import { PY_SPACE } from "../../web-e2e/compat.ts";

// The planner's blank test strips the characters Python's str.isspace accepts.
// That set differs from String.prototype.trim (it adds U+001C-U+001F and
// U+0085, and leaves U+FEFF), so a blank output is classified the same way.
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
