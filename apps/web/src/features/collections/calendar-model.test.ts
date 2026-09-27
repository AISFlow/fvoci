import assert from "node:assert/strict";
import test from "node:test";
import {
  dateMovable,
  dateMoveRequest,
  DEFAULT_LOCAL_TIME,
  type CalendarRow,
} from "./calendar-model.ts";

const fields = [
  { id: "f-date", type: "date", version: 3, deletedAt: null },
  { id: "f-dt", type: "datetime", version: 5, deletedAt: null },
  { id: "f-text", type: "text", version: 1, deletedAt: null },
  { id: "f-gone", type: "date", version: 2, deletedAt: "2026-09-01T00:00:00Z" },
];

function row(patch: Partial<CalendarRow> = {}): CalendarRow {
  return {
    id: "item-1",
    date: "2026-09-10",
    canEdit: true,
    taskId: "task-1",
    startDate: "2026-09-01",
    dueDate: "2026-09-10",
    dueAt: null,
    values: {},
    version: 7,
    ...patch,
  };
}

const expectedDates = { startDate: "2026-09-01", dueDate: "2026-09-10", dueAt: null };

test("due moves send the new day, clear dueAt and guard every task date", () => {
  assert.deepEqual(dateMoveRequest("due", row(), "2026-09-12", fields, "Asia/Seoul"), {
    kind: "task",
    taskId: "task-1",
    body: { dueDate: "2026-09-12", dueAt: null, expectedDates },
  });
  const timed = row({ dueDate: null, dueAt: "2026-09-10T03:00:00Z" });
  assert.deepEqual(dateMoveRequest("due", timed, null, fields, "Asia/Seoul"), {
    kind: "task",
    taskId: "task-1",
    body: {
      dueDate: null,
      dueAt: null,
      expectedDates: { startDate: "2026-09-01", dueDate: null, dueAt: "2026-09-10T03:00:00Z" },
    },
  });
});

test("start moves only change startDate", () => {
  const moved = row({ date: "2026-09-01" });
  assert.deepEqual(dateMoveRequest("start", moved, "2026-09-03", fields, "UTC"), {
    kind: "task",
    taskId: "task-1",
    body: { startDate: "2026-09-03", expectedDates },
  });
  assert.deepEqual(dateMoveRequest("start", moved, null, fields, "UTC"), {
    kind: "task",
    taskId: "task-1",
    body: { startDate: null, expectedDates },
  });
});

test("a date field stays a plain date with item and field versions", () => {
  assert.deepEqual(dateMoveRequest("f-date", row(), "2026-09-20", fields, "UTC"), {
    kind: "field",
    fieldId: "f-date",
    expectedVersion: 7,
    expectedFieldVersion: 3,
    value: { date: "2026-09-20" },
  });
  assert.deepEqual(dateMoveRequest("f-date", row(), null, fields, "UTC"), {
    kind: "field",
    fieldId: "f-date",
    expectedVersion: 7,
    expectedFieldVersion: 3,
    value: null,
  });
});

test("a datetime field keeps its wall time in the user's zone", () => {
  // 2026-09-10 23:30 in Seoul is 14:30Z the same day.
  const timed = row({ values: { "f-dt": { datetime: "2026-09-10T14:30:00Z" } } as never });
  assert.deepEqual(dateMoveRequest("f-dt", timed, "2026-09-11", fields, "Asia/Seoul"), {
    kind: "field",
    fieldId: "f-dt",
    expectedVersion: 7,
    expectedFieldVersion: 5,
    value: { datetime: "2026-09-11T14:30:00Z" },
  });
  // Same instant seen from New York is 10:30 on 2026-09-10.
  const ny = row({ ...timed, date: "2026-09-10" });
  assert.deepEqual(dateMoveRequest("f-dt", ny, "2026-09-12", fields, "America/New_York"), {
    kind: "field",
    fieldId: "f-dt",
    expectedVersion: 7,
    expectedFieldVersion: 5,
    value: { datetime: "2026-09-12T14:30:00Z" },
  });
});

test("an undated datetime gets the default wall time; a DST gap is unavailable", () => {
  const undated = row({ date: null });
  assert.equal(DEFAULT_LOCAL_TIME, "09:00");
  assert.deepEqual(dateMoveRequest("f-dt", undated, "2026-09-11", fields, "Asia/Seoul"), {
    kind: "field",
    fieldId: "f-dt",
    expectedVersion: 7,
    expectedFieldVersion: 5,
    value: { datetime: "2026-09-11T00:00:00Z" },
  });
  // 02:30 does not exist on 2026-03-08 in New York.
  const early = row({ values: { "f-dt": { datetime: "2026-03-01T07:30:00Z" } } as never });
  assert.deepEqual(
    dateMoveRequest("f-dt", early, "2026-03-08", fields, "America/New_York"),
    { kind: "unavailable" },
  );
});

test("moves that must not write are rejected", () => {
  assert.equal(dateMoveRequest(null, row(), "2026-09-12", fields, "UTC"), null);
  assert.equal(dateMoveRequest("due", row({ canEdit: false }), "2026-09-12", fields, "UTC"), null);
  assert.equal(dateMoveRequest("due", row(), "2026-09-10", fields, "UTC"), null);
  assert.equal(dateMoveRequest("due", row({ date: null }), null, fields, "UTC"), null);
  assert.equal(dateMoveRequest("due", row({ taskId: null }), "2026-09-12", fields, "UTC"), null);
  assert.equal(dateMoveRequest("due", row(), "2026-02-30", fields, "UTC"), null);
  assert.equal(dateMoveRequest("f-text", row(), "2026-09-12", fields, "UTC"), null);
  assert.equal(dateMoveRequest("f-gone", row(), "2026-09-12", fields, "UTC"), null);
  assert.equal(dateMoveRequest("f-missing", row(), "2026-09-12", fields, "UTC"), null);
});

test("only editable rows with a usable date basis are draggable", () => {
  assert.equal(dateMovable("due", row(), fields), true);
  assert.equal(dateMovable("start", row({ date: null }), fields), true);
  assert.equal(dateMovable("f-dt", row({ taskId: null }), fields), true);
  assert.equal(dateMovable(null, row(), fields), false);
  assert.equal(dateMovable("due", row({ canEdit: false }), fields), false);
  assert.equal(dateMovable("due", row({ taskId: null }), fields), false);
  assert.equal(dateMovable("f-text", row(), fields), false);
  assert.equal(dateMovable("f-gone", row(), fields), false);
});
