import assert from "node:assert/strict";
import test from "node:test";
import { rescheduleBody, type RescheduleItem } from "./reschedule-body.ts";

const TZ = "Asia/Seoul";

function item(fields: Partial<RescheduleItem> & Pick<RescheduleItem, "start" | "end">): RescheduleItem {
  return { startDate: null, dueDate: null, dueAt: null, ...fields };
}

const RANGE = item({ startDate: "2026-09-10", dueDate: "2026-09-14", start: "2026-09-10", end: "2026-09-14" });

test("moving a bar shifts start and due by the same days and sends the layout's dates as expectedDates", () => {
  assert.deepEqual(rescheduleBody(RANGE, { kind: "move", start: "2026-09-15", end: "2026-09-19" }, TZ), {
    startDate: "2026-09-15",
    dueDate: "2026-09-19",
    expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
  });
});

test("a drop on the same days writes nothing", () => {
  assert.equal(rescheduleBody(RANGE, { kind: "move", start: "2026-09-10", end: "2026-09-14" }, TZ), null);
  assert.equal(rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-14" }, TZ), null);
});

test("moving a task with only a due date writes only the due date", () => {
  const dueOnly = item({ dueDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(rescheduleBody(dueOnly, { kind: "move", start: "2026-09-18", end: "2026-09-18" }, TZ), {
    dueDate: "2026-09-18",
    expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
  });
  const startOnly = item({ startDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(rescheduleBody(startOnly, { kind: "move", start: "2026-09-23", end: "2026-09-23" }, TZ), {
    startDate: "2026-09-23",
    expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
  });
});

test("moving a dueAt keeps its time of day in the user's time zone and never clears it", () => {
  // 00:30 on 2026-09-15 in Seoul; the bar's end is its UTC date, 2026-09-14.
  const timed = item({
    startDate: "2026-09-10",
    dueAt: "2026-09-14T15:30:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
  });
  assert.deepEqual(rescheduleBody(timed, { kind: "move", start: "2026-09-13", end: "2026-09-17" }, TZ), {
    startDate: "2026-09-13",
    dueAt: "2026-09-17T15:30:00.000Z",
    expectedDates: { startDate: "2026-09-10", dueDate: null, dueAt: "2026-09-14T15:30:00.000Z" },
  });
  const dueAtOnly = item({ dueAt: "2026-09-20T09:30:00.123Z", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(rescheduleBody(dueAtOnly, { kind: "move", start: "2026-09-15", end: "2026-09-15" }, TZ), {
    dueAt: "2026-09-15T09:30:00.123Z",
    expectedDates: { startDate: null, dueDate: null, dueAt: "2026-09-20T09:30:00.123Z" },
  });
});

test("a task with both dueDate and dueAt moves both", () => {
  const both = item({
    startDate: "2026-09-10",
    dueDate: "2026-09-14",
    dueAt: "2026-09-14T09:00:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
  });
  assert.deepEqual(rescheduleBody(both, { kind: "move", start: "2026-09-12", end: "2026-09-16" }, TZ), {
    startDate: "2026-09-12",
    dueDate: "2026-09-16",
    dueAt: "2026-09-16T09:00:00.000Z",
    expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: "2026-09-14T09:00:00.000Z" },
  });
});

test("the start handle writes only the start; the end handle only the due", () => {
  assert.deepEqual(rescheduleBody(RANGE, { kind: "start", start: "2026-09-08", end: "2026-09-14" }, TZ), {
    startDate: "2026-09-08",
    expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
  });
  assert.deepEqual(rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-12" }, TZ), {
    dueDate: "2026-09-12",
    expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
  });
});

test("the end handle on a dueAt moves it by whole days and keeps its time", () => {
  const timed = item({ startDate: "2026-09-10", dueAt: "2026-09-14T09:30:00.000Z", start: "2026-09-10", end: "2026-09-14" });
  assert.deepEqual(rescheduleBody(timed, { kind: "end", start: "2026-09-10", end: "2026-09-16" }, TZ), {
    dueAt: "2026-09-16T09:30:00.000Z",
    expectedDates: { startDate: "2026-09-10", dueDate: null, dueAt: "2026-09-14T09:30:00.000Z" },
  });
});

test("a handle on a one-date bar writes the missing date at the edge that stayed", () => {
  const dueOnly = item({ dueDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(rescheduleBody(dueOnly, { kind: "start", start: "2026-09-17", end: "2026-09-20" }, TZ), {
    startDate: "2026-09-17",
    expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
  });
  assert.deepEqual(rescheduleBody(dueOnly, { kind: "end", start: "2026-09-20", end: "2026-09-22" }, TZ), {
    startDate: "2026-09-20",
    dueDate: "2026-09-22",
    expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
  });
  const startOnly = item({ startDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(rescheduleBody(startOnly, { kind: "end", start: "2026-09-20", end: "2026-09-24" }, TZ), {
    dueDate: "2026-09-24",
    expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
  });
  assert.deepEqual(rescheduleBody(startOnly, { kind: "start", start: "2026-09-18", end: "2026-09-20" }, TZ), {
    startDate: "2026-09-18",
    dueDate: "2026-09-20",
    expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
  });
});

test("a handle on a swapped bar (due before start) saves the drawn range in order", () => {
  const swapped = item({ startDate: "2026-09-14", dueDate: "2026-09-10", start: "2026-09-10", end: "2026-09-14" });
  assert.deepEqual(rescheduleBody(swapped, { kind: "end", start: "2026-09-10", end: "2026-09-16" }, TZ), {
    startDate: "2026-09-10",
    dueDate: "2026-09-16",
    expectedDates: { startDate: "2026-09-14", dueDate: "2026-09-10", dueAt: null },
  });
  // A move keeps both dates (still swapped) and shifts them together.
  assert.deepEqual(rescheduleBody(swapped, { kind: "move", start: "2026-09-11", end: "2026-09-15" }, TZ), {
    startDate: "2026-09-15",
    dueDate: "2026-09-11",
    expectedDates: { startDate: "2026-09-14", dueDate: "2026-09-10", dueAt: null },
  });
});

test("collapsing a range onto one day keeps both dates", () => {
  assert.deepEqual(rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-10" }, TZ), {
    dueDate: "2026-09-10",
    expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
  });
});
