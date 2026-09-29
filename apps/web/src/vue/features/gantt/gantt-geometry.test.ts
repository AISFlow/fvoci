import assert from "node:assert/strict";
import test from "node:test";
import {
  applyBarPointer,
  barRect,
  chartHeight,
  dateToX,
  dayColumns,
  linkPaths,
  monthBands,
  packFlow,
  scaleWidth,
  shiftWithin,
  stackRows,
  xToDate,
  type BarBox,
  type GanttScale,
  type ScheduledItem,
} from "./gantt-geometry.ts";

const SEPT: GanttScale = { start: "2026-08-30", end: "2026-10-03", pxPerDay: 32 };

function item(id: string, start: string, end: string, extra: Partial<ScheduledItem> = {}): ScheduledItem {
  return { id, start, end, milestone: false, inferred: "none", ...extra };
}

function bar(id: string, lane: number, x: number, width: number): BarBox {
  return { id, lane, x, width, milestone: false, inferred: "none" };
}

test("dates and x positions: a day is pxPerDay wide, x rounds down and is clamped", () => {
  assert.equal(dateToX("2026-08-30", SEPT), 0);
  assert.equal(dateToX("2026-09-01", SEPT), 64);
  assert.equal(dateToX("bad", SEPT), null);
  assert.equal(xToDate(0, SEPT), "2026-08-30");
  assert.equal(xToDate(31.9, SEPT), "2026-08-30");
  assert.equal(xToDate(32, SEPT), "2026-08-31");
  assert.equal(xToDate(-50, SEPT), "2026-08-30");
  assert.equal(xToDate(10_000, SEPT), "2026-10-03");
  // 35 days, both ends included.
  assert.equal(scaleWidth(SEPT), 35 * 32);
});

test("a bar covers both its start and end day; a milestone is a fixed diamond", () => {
  assert.deepEqual(barRect(item("a", "2026-09-10", "2026-09-14"), SEPT), { x: 11 * 32, width: 5 * 32 });
  assert.deepEqual(barRect(item("a", "2026-09-10", "2026-09-10"), SEPT), { x: 11 * 32, width: 32 });
  assert.deepEqual(barRect(item("m", "2026-09-10", "2026-09-10", { milestone: true }), SEPT), {
    x: 11 * 32 + 16 - 6,
    width: 12,
  });
  assert.equal(barRect(item("x", "not-a-date", "2026-09-10"), SEPT), null);
});

test("rows: one lane per item in (start, id) order; unplaceable items overflow", () => {
  const { bars, overflow } = stackRows(
    [
      item("c", "2026-09-05", "2026-09-06"),
      item("b", "2026-09-01", "2026-09-02"),
      item("a", "2026-09-05", "2026-09-05"),
      item("z", "bad", "bad"),
    ],
    SEPT,
  );
  assert.deepEqual(
    bars.map((b) => [b.id, b.lane]),
    [
      ["b", 0],
      ["a", 1],
      ["c", 2],
    ],
  );
  assert.deepEqual(overflow, ["z"]);
});

test("overlap packing shares lanes and keeps a blocked item below an overlapping blocker", () => {
  const items = [
    item("a", "2026-09-01", "2026-09-03"),
    item("b", "2026-09-05", "2026-09-06"),
    item("c", "2026-09-02", "2026-09-04"),
  ];
  const free = packFlow(items, SEPT, []);
  assert.deepEqual(
    Object.fromEntries(free.bars.map((b) => [b.id, b.lane])),
    { a: 0, c: 1, b: 0 },
  );
  // b is blocked by c (lane 1): it never sits above c, and goes below it when
  // their bars touch (b starts the day after c ends).
  const link = [{ blockerId: "c", blockedId: "b", type: "FS" as const, lagDays: 0 }];
  assert.deepEqual(
    Object.fromEntries(packFlow(items, SEPT, link).bars.map((b) => [b.id, b.lane])),
    { a: 0, c: 1, b: 2 },
  );
  const later = [...items.slice(0, 2).map((i) => (i.id === "b" ? item("b", "2026-09-08", "2026-09-09") : i)), items[2]!];
  assert.deepEqual(
    Object.fromEntries(packFlow(later, SEPT, link).bars.map((b) => [b.id, b.lane])),
    { a: 0, c: 1, b: 1 },
  );
});

test("day columns mark weekends and workspace holidays off duty", () => {
  const scale: GanttScale = { start: "2026-09-04", end: "2026-09-08", pxPerDay: 20 };
  const columns = dayColumns(scale, { weekend: [0, 6], holidays: new Set(["2026-09-07"]) });
  assert.deepEqual(
    columns.map((c) => [c.date, c.x, c.width, c.label, c.offDuty]),
    [
      ["2026-09-04", 0, 20, "4", false],
      ["2026-09-05", 20, 20, "5", true],
      ["2026-09-06", 40, 20, "6", true],
      ["2026-09-07", 60, 20, "7", true],
      ["2026-09-08", 80, 20, "8", false],
    ],
  );
});

test("month bands span their days with the localized month label", () => {
  const scale: GanttScale = { start: "2026-08-30", end: "2026-09-02", pxPerDay: 10 };
  const bands = monthBands(dayColumns(scale, { weekend: [], holidays: new Set() }));
  assert.deepEqual(bands, [
    { key: "2026-08", label: "8월", x: 0, width: 20 },
    { key: "2026-09", label: "9월", x: 20, width: 20 },
  ]);
});

test("link anchors: FS end to start, SS start to start, FF end to end, shifted by the lag", () => {
  const lane = 40;
  const bars = [bar("a", 0, 0, 64), bar("b", 1, 160, 64)];
  const [fs] = linkPaths([{ blockerId: "a", blockedId: "b", type: "FS", lagDays: 0 }], bars, lane, 32);
  assert.deepEqual(fs!.points, [64, 20, 112, 20, 112, 60, 160, 60]);
  const [ss] = linkPaths([{ blockerId: "a", blockedId: "b", type: "SS", lagDays: 1 }], bars, lane, 32);
  assert.deepEqual(ss!.points, [0, 20, 96, 20, 96, 60, 192, 60]);
  const [ff] = linkPaths([{ blockerId: "a", blockedId: "b", type: "FF", lagDays: 0 }], bars, lane, 32);
  assert.deepEqual(ff!.points, [64, 20, 144, 20, 144, 60, 224, 60]);
});

test("link shapes: straight on one lane, vertical when ranges overlap, a detour when the target is behind", () => {
  const lane = 40;
  const sameLane = linkPaths(
    [{ blockerId: "a", blockedId: "b", type: "FS", lagDays: 0 }],
    [bar("a", 0, 0, 64), bar("b", 0, 70, 32)],
    lane,
    32,
  );
  assert.deepEqual(sameLane[0]!.points, [64, 20, 70, 20]);
  const overlap = linkPaths(
    [{ blockerId: "a", blockedId: "b", type: "SS", lagDays: 0 }],
    [bar("a", 0, 0, 96), bar("b", 2, 64, 64)],
    lane,
    32,
  );
  // Through the middle of the shared x range, from the blocker's lower edge to the blocked's upper edge.
  assert.deepEqual(overlap[0]!.points, [80, 20 + lane * 0.26, 80, 100 - lane * 0.26]);
  const behind = linkPaths(
    [{ blockerId: "a", blockedId: "b", type: "FS", lagDays: 0 }],
    [bar("a", 0, 128, 64), bar("b", 1, 0, 32)],
    lane,
    32,
  );
  const points = behind[0]!.points;
  assert.equal(points.length, 12);
  assert.deepEqual(points.slice(0, 2), [192, 20]);
  assert.deepEqual(points.slice(-2), [0, 60]);
  // The detour runs between the two lanes (the gutter at y = 40).
  assert.equal(points[5], 40);
});

test("links to items without a bar are skipped, and the chart is tall enough for detours", () => {
  const paths = linkPaths(
    [{ blockerId: "a", blockedId: "gone", type: "FS", lagDays: 0 }],
    [bar("a", 0, 0, 32)],
    40,
    32,
  );
  assert.deepEqual(paths, []);
  assert.equal(chartHeight(0, 40, []), 40);
  assert.equal(chartHeight(3, 40, []), 120);
  assert.equal(chartHeight(1, 40, [{ blockerId: "a", blockedId: "b", points: [0, 20, 5, 90] }]), 102);
});

test("dragging a bar: a move keeps the span inside the range, a handle never passes the other end", () => {
  const origin = { originStart: "2026-09-10", originEnd: "2026-09-14", originX: 11 * 32 + 5 };
  assert.deepEqual(applyBarPointer({ kind: "move", ...origin }, 16 * 32 + 5, SEPT), {
    start: "2026-09-15",
    end: "2026-09-19",
  });
  assert.deepEqual(applyBarPointer({ kind: "move", ...origin }, 40 * 32, SEPT), {
    start: "2026-09-29",
    end: "2026-10-03",
  });
  assert.deepEqual(applyBarPointer({ kind: "move", ...origin }, -500, SEPT), {
    start: "2026-08-30",
    end: "2026-09-03",
  });
  assert.deepEqual(applyBarPointer({ kind: "start", ...origin }, 9 * 32, SEPT), {
    start: "2026-09-08",
    end: "2026-09-14",
  });
  assert.deepEqual(applyBarPointer({ kind: "start", ...origin }, 30 * 32, SEPT), {
    start: "2026-09-14",
    end: "2026-09-14",
  });
  assert.deepEqual(applyBarPointer({ kind: "end", ...origin }, 2 * 32, SEPT), {
    start: "2026-09-10",
    end: "2026-09-10",
  });
  assert.deepEqual(shiftWithin("2026-09-10", 4, -1, SEPT), { start: "2026-09-09", end: "2026-09-13" });
});
