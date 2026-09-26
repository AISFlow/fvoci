import assert from "node:assert/strict";
import test from "node:test";
import {
  datetimeLocalInTimeZoneToIso,
  durationSecondsBetween,
  isoToDatetimeLocalInTimeZone,
} from "./datetime.ts";

test("datetime-local values convert in the user's time zone", () => {
  assert.equal(
    datetimeLocalInTimeZoneToIso("2026-09-01T09:30", "Asia/Seoul"),
    "2026-09-01T00:30:00.000Z",
  );
  assert.equal(isoToDatetimeLocalInTimeZone("2026-09-01T00:30:00.000Z", "Asia/Seoul"), "2026-09-01T09:30");
  assert.equal(datetimeLocalInTimeZoneToIso("2026-09-01", "Asia/Seoul"), "");
  // Skipped by the US spring-forward gap.
  assert.equal(datetimeLocalInTimeZoneToIso("2026-03-08T02:30", "America/New_York"), "");
});

test("durationSecondsBetween is end minus start in seconds", () => {
  assert.equal(durationSecondsBetween("2026-09-01T00:00:00Z", "2026-09-01T01:30:00Z"), 5400);
  assert.ok(durationSecondsBetween("2026-09-01T01:00:00Z", "2026-09-01T00:00:00Z") < 0);
});
