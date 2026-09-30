import assert from "node:assert/strict";

/** Validates a fixture slot or callback before the test exercises it. */
export function assertPresent<T>(value: T | null | undefined): T {
  assert.ok(value !== undefined && value !== null, "expected fixture value or installed callback");
  return value;
}
