import assert from "node:assert/strict";
import test from "node:test";
import { mergeHolidayItems } from "./holidays.ts";

await test("holiday cache merge adds, sorts, and removes dates", () => {
  assert.deepEqual(mergeHolidayItems(["2026-09-02"], "2026-09-01", false), [
    "2026-09-01",
    "2026-09-02",
  ]);
  assert.deepEqual(mergeHolidayItems(["2026-09-01"], "2026-09-01", false), ["2026-09-01"]);
  assert.deepEqual(mergeHolidayItems(["2026-09-01", "2026-09-02"], "2026-09-01", true), [
    "2026-09-02",
  ]);
});
