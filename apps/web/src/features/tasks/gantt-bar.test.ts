import assert from "node:assert/strict";
import test from "node:test";
import { ganttBarPatch } from "./gantt-bar.ts";

test("start+dueDate → 둘 다 교체", () => {
  assert.deepEqual(
    ganttBarPatch(
      { startDate: "2026-08-03", dueDate: "2026-08-07", dueAt: null },
      { start: "2026-08-10", end: "2026-08-14" },
    ),
    { startDate: "2026-08-10", dueDate: "2026-08-14" },
  );
});

test("dueAt만 → dueDate 채우고 dueAt null", () => {
  assert.deepEqual(
    ganttBarPatch(
      {
        startDate: null,
        dueDate: null,
        dueAt: "2026-08-10T15:00:00.000Z",
      },
      { start: "2026-08-20", end: "2026-08-20" },
    ),
    {
      startDate: "2026-08-20",
      dueDate: "2026-08-20",
      dueAt: null,
    },
  );
});

test("dueDate 있으면 dueAt을 건드리지 않는다", () => {
  assert.deepEqual(
    ganttBarPatch(
      {
        startDate: "2026-08-03",
        dueDate: "2026-08-07",
        dueAt: "2026-08-07T09:00:00.000Z",
      },
      { start: "2026-08-10", end: "2026-08-12" },
    ),
    { startDate: "2026-08-10", dueDate: "2026-08-12" },
  );
});
