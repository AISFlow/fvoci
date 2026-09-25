import assert from "node:assert/strict";
import test from "node:test";

function mergeHolidayItems(
  current: string[] | undefined,
  date: string,
  remove: boolean,
): string[] {
  return remove
    ? (current ?? []).filter((day) => day !== date)
    : [...new Set([...(current ?? []), date])].sort();
}

test("holiday cache merge adds, sorts, and removes dates", () => {
  assert.deepEqual(mergeHolidayItems(["2026-09-02"], "2026-09-01", false), [
    "2026-09-01",
    "2026-09-02",
  ]);
  assert.deepEqual(mergeHolidayItems(["2026-09-01"], "2026-09-01", false), ["2026-09-01"]);
  assert.deepEqual(mergeHolidayItems(["2026-09-01", "2026-09-02"], "2026-09-01", true), [
    "2026-09-02",
  ]);
});
