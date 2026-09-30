import assert from "node:assert/strict";
import { test, expect } from "bun:test";
import {
  addDays,
  dayMove,
  editorWrite,
  eventFor,
  optimisticRow,
  resizable,
  resizeWrite,
  weekDays,
} from "./calendar-adapter";
import type { CollectionField, CollectionQueryPreview } from "@/lib/queries/collections";
const fields = [
  { id: "date", type: "date", version: 3, deletedAt: null },
  { id: "time", type: "datetime", version: 5, deletedAt: null },
] as CollectionField[];
const base: CollectionQueryPreview = {
  id: "item",
  taskId: "task",
  documentId: null,
  statusId: null,
  displayId: "CAL-1",
  title: "Task",
  canEdit: true,
  date: "2026-09-10",
  startDate: "2026-09-01",
  dueDate: "2026-09-10",
  dueAt: null,
  values: {},
  version: 7,
};
const expectedDates = { startDate: base.startDate, dueDate: base.dueDate, dueAt: base.dueAt };
test("date buckets are not UTC midnight instants and have no end/duration", () => {
  const event = eventFor(base, "due", fields, "America/New_York");
  expect(event.local).toBe("2026-09-10");
  expect(event.timed).toBe(false);
  expect("end" in event).toBe(false);
  expect("duration" in event).toBe(false);
  expect(
    eventFor(
      { ...base, dueDate: null, dueAt: "2026-09-11T01:30:00Z", date: "2026-09-10" },
      "due",
      fields,
      "America/New_York",
    ).local,
  ).toBe("2026-09-10T21:30");
});
test("month drop keeps existing dueDate policy; time-axis edit preserves dueAt", () => {
  const row = { ...base, dueDate: null, dueAt: "2026-09-10T14:30:00Z" };
  expect(dayMove("due", row, "2026-09-12", fields, "Asia/Seoul")).toEqual({
    kind: "task",
    taskId: "task",
    body: {
      dueDate: "2026-09-12",
      dueAt: null,
      expectedDates: { ...expectedDates, dueDate: null, dueAt: row.dueAt },
    },
  });
  expect(editorWrite("due", row, "2026-09-12T23:30", true, fields, "Asia/Seoul")).toEqual({
    kind: "task",
    taskId: "task",
    body: {
      dueDate: null,
      dueAt: "2026-09-12T14:30:00Z",
      expectedDates: { ...expectedDates, dueDate: null, dueAt: row.dueAt },
    },
  });
});
test("date/datetime values retain item and field conflict versions", () => {
  expect(editorWrite("date", base, "2026-09-12", false, fields, "Pacific/Honolulu")).toEqual({
    kind: "field",
    fieldId: "date",
    expectedVersion: 7,
    expectedFieldVersion: 3,
    value: { date: "2026-09-12" },
  });
  expect(editorWrite("time", base, "2026-09-12T23:30", true, fields, "Asia/Seoul")).toEqual({
    kind: "field",
    fieldId: "time",
    expectedVersion: 7,
    expectedFieldVersion: 5,
    value: { datetime: "2026-09-12T14:30:00Z" },
  });
});
test("DST gap is refused; fold uses existing converter for new times and preserves existing second occurrence", () => {
  expect(editorWrite("due", base, "2026-03-08T02:30", true, fields, "America/New_York")).toBeNull();
  expect(
    editorWrite("time", base, "2026-03-08T02:30", true, fields, "America/New_York"),
  ).toBeNull();
  expect(
    editorWrite("due", base, "2026-11-01T01:30", true, fields, "America/New_York"),
  ).toMatchObject({ body: { dueAt: "2026-11-01T05:30:00Z" } });
  const folded = { ...base, dueDate: null, dueAt: "2026-11-01T06:30:00Z" };
  expect(
    editorWrite("due", folded, "2026-11-01T01:30", true, fields, "America/New_York"),
  ).toMatchObject({ body: { dueAt: folded.dueAt } });
  expect(
    editorWrite(
      "time",
      { ...base, values: { time: { datetime: folded.dueAt } } as never },
      "2026-11-01T01:30",
      true,
      fields,
      "America/New_York",
    ),
  ).toMatchObject({ value: { datetime: folded.dueAt } });
});
test("resize uses only existing ordered plain endpoints and guards all dates", () => {
  expect(resizeWrite(base, "due", "end", "2026-09-12")).toEqual({
    kind: "task",
    taskId: "task",
    body: { dueDate: "2026-09-12", expectedDates },
  });
  expect(resizeWrite(base, "start", "start", "2026-09-02")).toMatchObject({
    body: { startDate: "2026-09-02", expectedDates },
  });
  for (const row of [
    { ...base, startDate: null },
    { ...base, dueDate: null },
    { ...base, dueAt: "2026-09-10T12:00Z" },
    { ...base, canEdit: false },
    { ...base, startDate: "2026-09-20" },
  ])
    expect(resizable(row, "due")).toBe(false);
  expect(resizable(base, "date")).toBe(false);
  expect(resizeWrite(base, "due", "end", "2026-08-01")).toBeNull();
  expect(resizeWrite(base, "due", "start", "2026-10-01")).toBeNull();
});
test("invalid/readonly edits never create a request; null clears explicitly", () => {
  expect(
    editorWrite("due", { ...base, canEdit: false }, "2026-09-12", false, fields, "UTC"),
  ).toBeNull();
  expect(editorWrite("date", base, "2026-02-30", false, fields, "UTC")).toBeNull();
  expect(dayMove("date", base, "2026-02-30", fields, "UTC")).toBeNull();
  expect(editorWrite("time", base, "", true, fields, "UTC")).toMatchObject({ value: null });
});
test("optimistic preview projects intent without changing concurrency snapshot", () => {
  const write = editorWrite("due", base, "2026-09-12", false, fields, "UTC");
  assert.ok(write);
  const next = optimisticRow(base, write, "UTC");
  const resized = resizeWrite(base, "due", "start", "2026-09-02");
  assert.ok(resized);
  expect(optimisticRow(base, resized, "UTC", "due").date).toBe("2026-09-10");
  expect(next.date).toBe("2026-09-12");
  expect(next.version).toBe(7);
  expect(base.date).toBe("2026-09-10");
  expect(weekDays("2026-03-08", 1)).toEqual([
    "2026-03-02",
    "2026-03-03",
    "2026-03-04",
    "2026-03-05",
    "2026-03-06",
    "2026-03-07",
    "2026-03-08",
  ]);
  expect(addDays("2026-03-08", 1)).toBe("2026-03-09");
});
test("dual due fields follow Rust dueDate-first buckets and unchanged date save preserves dueAt", () => {
  const dual = { ...base, dueDate: "2026-09-10", dueAt: "2026-09-20T14:30:00Z" };
  const event = eventFor(dual, "due", fields, "America/New_York");
  expect(event.timed).toBe(false);
  expect(event.local).toBe("2026-09-10");
  const unchanged = editorWrite("due", dual, event.local, event.timed, fields, "America/New_York");
  assert.ok(unchanged);
  expect(unchanged).toEqual({
    kind: "task",
    taskId: "task",
    body: { dueDate: dual.dueDate, expectedDates: { ...expectedDates, dueAt: dual.dueAt } },
  });
  expect(optimisticRow(dual, unchanged, "America/New_York", "due").date).toBe(dual.dueDate);
  expect(optimisticRow(dual, unchanged, "America/New_York", "due").dueAt).toBe(dual.dueAt);
  expect(editorWrite("due", dual, "2026-09-11", false, fields, "America/New_York")).toMatchObject({
    body: { dueDate: "2026-09-11", dueAt: null },
  });
  expect(
    editorWrite("due", dual, "2026-09-20T10:30", true, fields, "America/New_York"),
  ).toMatchObject({ body: { dueDate: null, dueAt: dual.dueAt } });
  expect(resizable(dual, "due")).toBe(false);
});
