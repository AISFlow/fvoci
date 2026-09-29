import assert from "node:assert/strict";
import test from "node:test";
import { dateInZone, shiftInstantDays } from "./zoned-date.ts";

test("dateInZone is the calendar date in the zone, not in UTC", () => {
  const at = Date.parse("2026-09-30T15:30:00.000Z");
  assert.equal(dateInZone(at, "UTC"), "2026-09-30");
  assert.equal(dateInZone(at, "Asia/Seoul"), "2026-10-01");
  assert.equal(dateInZone(at, "America/Los_Angeles"), "2026-09-30");
});

test("an unknown time zone falls back to Asia/Seoul", () => {
  assert.equal(dateInZone(Date.parse("2026-09-30T15:30:00.000Z"), "Not/AZone"), "2026-10-01");
});

test("shiftInstantDays keeps the wall-clock time, seconds and milliseconds", () => {
  assert.equal(shiftInstantDays("2026-09-15T09:30:00.123Z", 3, "Asia/Seoul"), "2026-09-18T09:30:00.123Z");
  assert.equal(shiftInstantDays("2026-09-15T23:45:07.000Z", -20, "UTC"), "2026-08-26T23:45:07.000Z");
  // 00:30 in Seoul is the previous UTC day; the Seoul date moves by the delta.
  assert.equal(shiftInstantDays("2026-09-14T15:30:00.000Z", 1, "Asia/Seoul"), "2026-09-15T15:30:00.000Z");
  assert.equal(shiftInstantDays("2026-09-14T15:30:00.000Z", 0, "Asia/Seoul"), "2026-09-14T15:30:00.000Z");
});

test("across a daylight-saving change the local time stays, the UTC time moves", () => {
  // New York: EDT (UTC-4) until 2026-11-01, then EST (UTC-5). 09:00 local.
  assert.equal(shiftInstantDays("2026-10-30T13:00:00.000Z", 3, "America/New_York"), "2026-11-02T14:00:00.000Z");
  assert.equal(shiftInstantDays("2026-11-02T14:00:00.000Z", -3, "America/New_York"), "2026-10-30T13:00:00.000Z");
});

test("a wall time inside a spring-forward gap moves forward by the gap", () => {
  // 2027-03-14 02:30 does not exist in New York (02:00 -> 03:00).
  assert.equal(shiftInstantDays("2027-03-13T07:30:00.000Z", 1, "America/New_York"), "2027-03-14T07:30:00.000Z");
});

test("an ambiguous fall-back wall time takes the earlier instant", () => {
  // 2026-11-01 01:30 happens twice in New York; 01:30 EDT is 05:30Z.
  assert.equal(shiftInstantDays("2026-10-31T05:30:00.000Z", 1, "America/New_York"), "2026-11-01T05:30:00.000Z");
});

test("an invalid instant is refused", () => {
  assert.throws(() => shiftInstantDays("not a date", 1, "UTC"), RangeError);
});
