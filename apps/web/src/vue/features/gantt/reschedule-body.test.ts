import assert from "node:assert/strict";
import test from "node:test";
import { rescheduleBody, type RescheduleItem } from "./reschedule-body.ts";

const TZ = "Asia/Seoul";

function item(
  fields: Partial<RescheduleItem> & Pick<RescheduleItem, "start" | "end">,
): RescheduleItem {
  const withDates = { startDate: null, dueDate: null, dueAt: null, ...fields };
  const hasStart = withDates.startDate !== null;
  const hasFinish = withDates.dueDate !== null || withDates.dueAt !== null;
  // As the server derives it (src/gantt/schedule.rs), unless the test says.
  const inferred = hasStart && hasFinish ? "none" : hasStart ? "from-start" : "from-due";
  return { inferred, ...withDates };
}

const RANGE = item({
  startDate: "2026-09-10",
  dueDate: "2026-09-14",
  start: "2026-09-10",
  end: "2026-09-14",
});

await test("moving a bar shifts start and due by the same days and sends the layout's dates as expectedDates", () => {
  assert.deepEqual(
    rescheduleBody(RANGE, { kind: "move", start: "2026-09-15", end: "2026-09-19" }, TZ),
    {
      startDate: "2026-09-15",
      dueDate: "2026-09-19",
      expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
    },
  );
});

await test("a drop on the same days writes nothing", () => {
  assert.equal(
    rescheduleBody(RANGE, { kind: "move", start: "2026-09-10", end: "2026-09-14" }, TZ),
    null,
  );
  assert.equal(
    rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-14" }, TZ),
    null,
  );
});

await test("moving a task with only a due date writes only the due date", () => {
  const dueOnly = item({ dueDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(dueOnly, { kind: "move", start: "2026-09-18", end: "2026-09-18" }, TZ),
    {
      dueDate: "2026-09-18",
      expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
    },
  );
  const startOnly = item({ startDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(startOnly, { kind: "move", start: "2026-09-23", end: "2026-09-23" }, TZ),
    {
      startDate: "2026-09-23",
      expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
    },
  );
});

await test("moving a dueAt keeps its time of day in the user's time zone and never clears it", () => {
  // 00:30 on 2026-09-15 in Seoul; the bar's end is its UTC date, 2026-09-14.
  const timed = item({
    startDate: "2026-09-10",
    dueAt: "2026-09-14T15:30:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
  });
  assert.deepEqual(
    rescheduleBody(timed, { kind: "move", start: "2026-09-13", end: "2026-09-17" }, TZ),
    {
      startDate: "2026-09-13",
      dueAt: "2026-09-17T15:30:00.000Z",
      expectedDates: { startDate: "2026-09-10", dueDate: null, dueAt: "2026-09-14T15:30:00.000Z" },
    },
  );
  const dueAtOnly = item({
    dueAt: "2026-09-20T09:30:00.123Z",
    start: "2026-09-20",
    end: "2026-09-20",
  });
  assert.deepEqual(
    rescheduleBody(dueAtOnly, { kind: "move", start: "2026-09-15", end: "2026-09-15" }, TZ),
    {
      dueAt: "2026-09-15T09:30:00.123Z",
      expectedDates: { startDate: null, dueDate: null, dueAt: "2026-09-20T09:30:00.123Z" },
    },
  );
});

await test("a task with both dueDate and dueAt moves both", () => {
  const both = item({
    startDate: "2026-09-10",
    dueDate: "2026-09-14",
    dueAt: "2026-09-14T09:00:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
  });
  assert.deepEqual(
    rescheduleBody(both, { kind: "move", start: "2026-09-12", end: "2026-09-16" }, TZ),
    {
      startDate: "2026-09-12",
      dueDate: "2026-09-16",
      dueAt: "2026-09-16T09:00:00.000Z",
      expectedDates: {
        startDate: "2026-09-10",
        dueDate: "2026-09-14",
        dueAt: "2026-09-14T09:00:00.000Z",
      },
    },
  );
});

await test("the start handle writes only the start; the end handle only the due", () => {
  assert.deepEqual(
    rescheduleBody(RANGE, { kind: "start", start: "2026-09-08", end: "2026-09-14" }, TZ),
    {
      startDate: "2026-09-08",
      expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
    },
  );
  assert.deepEqual(
    rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-12" }, TZ),
    {
      dueDate: "2026-09-12",
      expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
    },
  );
});

await test("the end handle on a dueAt moves it by whole days and keeps its time", () => {
  const timed = item({
    startDate: "2026-09-10",
    dueAt: "2026-09-14T09:30:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
  });
  assert.deepEqual(
    rescheduleBody(timed, { kind: "end", start: "2026-09-10", end: "2026-09-16" }, TZ),
    {
      dueAt: "2026-09-16T09:30:00.000Z",
      expectedDates: { startDate: "2026-09-10", dueDate: null, dueAt: "2026-09-14T09:30:00.000Z" },
    },
  );
});

await test("a handle on a one-date bar writes only the date of its own edge", () => {
  // The chart's handles: the start handle of a due-only task adds its start,
  // the end handle of a start-only task adds its due date.
  const dueOnly = item({ dueDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(dueOnly, { kind: "start", start: "2026-09-17", end: "2026-09-20" }, TZ),
    {
      startDate: "2026-09-17",
      expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
    },
  );
  const startOnly = item({ startDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(startOnly, { kind: "end", start: "2026-09-20", end: "2026-09-24" }, TZ),
    {
      dueDate: "2026-09-24",
      expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
    },
  );
  const dueAtOnly = item({
    dueAt: "2026-09-20T09:30:00.000Z",
    start: "2026-09-20",
    end: "2026-09-20",
  });
  assert.deepEqual(
    rescheduleBody(dueAtOnly, { kind: "start", start: "2026-09-18", end: "2026-09-20" }, TZ),
    {
      startDate: "2026-09-18",
      expectedDates: { startDate: null, dueDate: null, dueAt: "2026-09-20T09:30:00.000Z" },
    },
  );
});

await test("the handle on a one-date bar's own date only moves that date; no other date is invented", () => {
  // The chart offers no such handle (a move does the same); a change of that
  // kind still writes nothing but the one date.
  const dueOnly = item({ dueDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(dueOnly, { kind: "end", start: "2026-09-20", end: "2026-09-22" }, TZ),
    {
      dueDate: "2026-09-22",
      expectedDates: { startDate: null, dueDate: "2026-09-20", dueAt: null },
    },
  );
  const startOnly = item({ startDate: "2026-09-20", start: "2026-09-20", end: "2026-09-20" });
  assert.deepEqual(
    rescheduleBody(startOnly, { kind: "start", start: "2026-09-18", end: "2026-09-20" }, TZ),
    {
      startDate: "2026-09-18",
      expectedDates: { startDate: "2026-09-20", dueDate: null, dueAt: null },
    },
  );
  const dueAtOnly = item({
    dueAt: "2026-09-20T09:30:00.000Z",
    start: "2026-09-20",
    end: "2026-09-20",
  });
  assert.deepEqual(
    rescheduleBody(dueAtOnly, { kind: "end", start: "2026-09-20", end: "2026-09-23" }, TZ),
    {
      dueAt: "2026-09-23T09:30:00.000Z",
      expectedDates: { startDate: null, dueDate: null, dueAt: "2026-09-20T09:30:00.000Z" },
    },
  );
});

await test("a handle on a swapped bar (due before start) saves the drawn range in order", () => {
  const swapped = item({
    startDate: "2026-09-14",
    dueDate: "2026-09-10",
    start: "2026-09-10",
    end: "2026-09-14",
    inferred: "swapped",
  });
  assert.deepEqual(
    rescheduleBody(swapped, { kind: "end", start: "2026-09-10", end: "2026-09-16" }, TZ),
    {
      startDate: "2026-09-10",
      dueDate: "2026-09-16",
      expectedDates: { startDate: "2026-09-14", dueDate: "2026-09-10", dueAt: null },
    },
  );
  // A move keeps both dates (still swapped) and shifts them together.
  assert.deepEqual(
    rescheduleBody(swapped, { kind: "move", start: "2026-09-11", end: "2026-09-15" }, TZ),
    {
      startDate: "2026-09-15",
      dueDate: "2026-09-11",
      expectedDates: { startDate: "2026-09-14", dueDate: "2026-09-10", dueAt: null },
    },
  );
});

await test("collapsing a range onto one day keeps both dates", () => {
  assert.deepEqual(
    rescheduleBody(RANGE, { kind: "end", start: "2026-09-10", end: "2026-09-10" }, TZ),
    {
      dueDate: "2026-09-10",
      expectedDates: { startDate: "2026-09-10", dueDate: "2026-09-14", dueAt: null },
    },
  );
});

await test("a swapped dueAt: the start handle moves the dueAt, whose day is the range's start", () => {
  // 18:00 on 2026-09-10 in Seoul is 09:00Z the same day: the finish day is the 10th.
  const swapped = item({
    startDate: "2026-09-14",
    dueAt: "2026-09-10T09:00:00.000Z",
    start: "2026-09-10",
    end: "2026-09-14",
    inferred: "swapped",
  });
  assert.deepEqual(
    rescheduleBody(swapped, { kind: "start", start: "2026-09-08", end: "2026-09-14" }, TZ),
    {
      startDate: "2026-09-08",
      dueAt: "2026-09-14T09:00:00.000Z",
      expectedDates: { startDate: "2026-09-14", dueDate: null, dueAt: "2026-09-10T09:00:00.000Z" },
    },
  );
});

await test("across a daylight-saving change the moved dueAt keeps its local time, and its UTC day can move a day more", () => {
  // America/New_York leaves daylight saving on 2031-11-02. 19:30 EDT on
  // 2031-11-01 is 23:30Z (bar end 2031-11-01). Three days later at 19:30 EST
  // is 00:30Z on 2031-11-05: the bar is dropped ending on the 4th, and the
  // refetched layout ends it on the 5th. Asia/Seoul keeps one offset all year.
  const timed = item({
    startDate: "2031-10-28",
    dueAt: "2031-11-01T23:30:00.000Z",
    start: "2031-10-28",
    end: "2031-11-01",
  });
  const body = rescheduleBody(
    timed,
    { kind: "move", start: "2031-10-31", end: "2031-11-04" },
    "America/New_York",
  );
  assert.deepEqual(body, {
    startDate: "2031-10-31",
    dueAt: "2031-11-05T00:30:00.000Z",
    expectedDates: { startDate: "2031-10-28", dueDate: null, dueAt: "2031-11-01T23:30:00.000Z" },
  });
  assert.equal(new Date(body.dueAt).toISOString().slice(0, 10), "2031-11-05");
  const seoul = rescheduleBody(timed, { kind: "move", start: "2031-10-31", end: "2031-11-04" }, TZ);
  assert.ok(seoul?.dueAt);
  assert.equal(new Date(seoul.dueAt).toISOString().slice(0, 10), "2031-11-04");
});
