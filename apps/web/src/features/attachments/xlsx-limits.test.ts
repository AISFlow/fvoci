import assert from "node:assert/strict";
import test from "node:test";
import {
  XLSX_COLS_PER_PAGE,
  XLSX_LIMITS,
  XLSX_MAX_BYTES,
  XLSX_ROWS_PER_PAGE,
  pageCount,
  pageWindow,
} from "./xlsx-limits.ts";

test("source load caps and page sizes", () => {
  assert.equal(XLSX_MAX_BYTES, 32 * 1024 * 1024);
  assert.deepEqual(XLSX_LIMITS, { maxExpandedBytes: 32 * 1024 * 1024, maxCells: 100_000, maxRows: 20_000 });
  assert.equal(XLSX_ROWS_PER_PAGE, 200);
  assert.equal(XLSX_COLS_PER_PAGE, 64);
});

test("pageCount has at least one page", () => {
  assert.equal(pageCount(0, 200), 1);
  assert.equal(pageCount(200, 200), 1);
  assert.equal(pageCount(201, 200), 2);
});

test("pageWindow pages rows by 200 and columns by 64 from the extent origin", () => {
  const extent = { minRow: 3, maxRow: 452, minCol: 2, maxCol: 130 };
  const first = pageWindow(extent, 0, 0);
  assert.equal(first.rowPages, 3);
  assert.equal(first.colPages, 3);
  assert.deepEqual(first.box, { minRow: 3, maxRow: 202, minCol: 2, maxCol: 65 });
  const last = pageWindow(extent, 2, 2);
  assert.deepEqual(last.box, { minRow: 403, maxRow: 452, minCol: 130, maxCol: 130 });
});

test("pageWindow clamps stale or invalid page indexes into range", () => {
  const extent = { minRow: 1, maxRow: 10, minCol: 1, maxCol: 3 };
  for (const [row, col] of [
    [5, 9],
    [-1, -4],
    [Number.NaN, Number.POSITIVE_INFINITY],
  ] as const) {
    const window = pageWindow(extent, row, col);
    assert.equal(window.rowPage, 0);
    assert.equal(window.colPage, 0);
    assert.deepEqual(window.box, extent);
  }
  const wide = pageWindow({ minRow: 1, maxRow: 1000, minCol: 1, maxCol: 1 }, 99, 0);
  assert.equal(wide.rowPage, 4);
  assert.deepEqual(wide.box, { minRow: 801, maxRow: 1000, minCol: 1, maxCol: 1 });
});

test("one page never exceeds 200 × 64 cells, even on a sparse sheet at the grid limits", () => {
  const window = pageWindow({ minRow: 1, maxRow: 1_048_576, minCol: 1, maxCol: 16_384 }, 3, 7);
  assert.equal(window.rowPages, 5243);
  assert.equal(window.colPages, 256);
  assert.equal(window.box.maxRow - window.box.minRow + 1, 200);
  assert.equal(window.box.maxCol - window.box.minCol + 1, 64);
});
