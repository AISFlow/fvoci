import assert from "node:assert/strict";
import test from "node:test";
import { formatDuration } from "./time-entry-format.ts";

test("formatDuration shows hours and minutes like the source", () => {
  assert.equal(formatDuration(90 * 60), "1시간 30분");
  assert.equal(formatDuration(2 * 3600), "2시간");
  assert.equal(formatDuration(59), "0분");
  assert.equal(formatDuration(5 * 60 + 59), "5분");
});
