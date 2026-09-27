import assert from "node:assert/strict";
import test from "node:test";
import { buildFixtureXlsx, gridSheet } from "./xlsx-test-fixture.ts";
import { XLSX_LIMITS } from "./xlsx-limits.ts";
import { openXlsx, type XlsxBook } from "./xlsx-workbook.ts";

async function open(bytes: Uint8Array, limits = XLSX_LIMITS): Promise<XlsxBook> {
  const result = await openXlsx(bytes, limits);
  assert.equal(result.status, "ok");
  return (result as { book: XlsxBook }).book;
}

test("text cells: shared/inline strings, numbers, Korean and emoji, formula cached values", async () => {
  const book = await open(
    await buildFixtureXlsx(
      [
        {
          name: "요약 📊",
          cells: [
            { ref: "A1", shared: "한글 셀 😀" },
            { ref: "B1", inline: "inline 줄바꿈\n둘째 줄" },
            { ref: "C1", number: 3.5 },
            { ref: "A2", formula: "SUM(C1,1)", cached: 4.5 },
            { ref: "B2", formula: "[1]Remote!A1", cached: "외부 캐시값" },
            { ref: "C2", formula: "NOW()", cached: "stale" },
          ],
        },
      ],
      { vba: true, externalLink: true, deflate: true },
    ),
  );
  assert.deepEqual(book.sheets, [{ name: "요약 📊", kind: "worksheet" }]);
  const page = book.page(0, 0, 0);
  assert.ok(page);
  // Formulas show the stored cached value; nothing is recalculated or fetched.
  assert.deepEqual(page.rows, [
    ["한글 셀 😀", "inline 줄바꿈\n둘째 줄", "3.5"],
    ["4.5", "외부 캐시값", "stale"],
  ]);
});

test("merged ranges show the anchor value once, like the source", async () => {
  const book = await open(
    await buildFixtureXlsx([
      {
        name: "Merged",
        cells: [
          { ref: "A1", inline: "병합" },
          { ref: "C3", inline: "end" },
        ],
        merges: ["A1:B2"],
      },
    ]),
  );
  assert.deepEqual(book.page(0, 0, 0)?.rows, [
    ["병합", "", ""],
    ["", "", ""],
    ["", "", "end"],
  ]);
});

test("chartsheet is an unsupported tab; the worksheets around it still read", async () => {
  const book = await open(
    await buildFixtureXlsx([
      { name: "Chart", chart: true },
      { name: "SECOND SHEET", cells: [{ ref: "A1", inline: "SECOND SHEET" }] },
    ]),
  );
  assert.deepEqual(book.sheets, [
    { name: "Chart", kind: "unsupported" },
    { name: "SECOND SHEET", kind: "worksheet" },
  ]);
  assert.equal(book.page(0, 0, 0), null);
  assert.deepEqual(book.page(1, 0, 0)?.rows, [["SECOND SHEET"]]);
  assert.equal(book.page(2, 0, 0), null);
});

test("empty worksheet has no page", async () => {
  const book = await open(await buildFixtureXlsx([{ name: "Empty", cells: [] }]));
  assert.equal(book.page(0, 0, 0), null);
});

test("row pages of 200 and column pages of 64 map to the right cells", async () => {
  const book = await open(await buildFixtureXlsx([gridSheet("Grid", 401, 65)], { deflate: true }));
  const first = book.page(0, 0, 0)!;
  assert.equal(first.rowPages, 3);
  assert.equal(first.colPages, 2);
  assert.equal(first.rows.length, 200);
  assert.equal(first.rows[0]!.length, 64);
  assert.equal(first.rows[0]![0], "R1C1");
  assert.equal(first.rows[199]![63], "R200C64");
  const last = book.page(0, 2, 1)!;
  assert.deepEqual(last.rows, [["R401C65"]]);
  // Out-of-range page indexes clamp to the last page.
  assert.deepEqual(book.page(0, 9, 9)!.rows, [["R401C65"]]);
});

test("row, cell and inflated-byte caps reject the workbook as too large", async () => {
  const rows = await openXlsx(await buildFixtureXlsx([gridSheet("Rows", 20_001, 1)]));
  assert.equal(rows.status, "tooLarge");
  const cells = await openXlsx(await buildFixtureXlsx([gridSheet("Cells", 1001, 100)]));
  assert.equal(cells.status, "tooLarge");
  const atCaps = await openXlsx(await buildFixtureXlsx([gridSheet("Fits", 1000, 100)]));
  assert.equal(atCaps.status, "ok");
  // Stored (1:1) parts: only the total inflated-byte cap can trip.
  const bytes = await openXlsx(
    await buildFixtureXlsx([{ name: "Big", cells: [{ ref: "A1", inline: "x".repeat(64 * 1024) }] }]),
    { ...XLSX_LIMITS, maxExpandedBytes: 32 * 1024 },
  );
  assert.equal(bytes.status, "tooLarge");
});

test("a highly compressed part is refused before it inflates in full", async () => {
  const bomb = await buildFixtureXlsx(
    [{ name: "Bomb", cells: [{ ref: "A1", inline: "x".repeat(40 * 1024 * 1024) }] }],
    { deflate: true },
  );
  assert.ok(bomb.byteLength < 1024 * 1024);
  assert.equal((await openXlsx(bomb)).status, "tooLarge");
});

test("non-XLSX bytes are invalid, not a crash", async () => {
  assert.equal((await openXlsx(new TextEncoder().encode("plain text, not a zip"))).status, "invalid");
  assert.equal((await openXlsx(new Uint8Array(0))).status, "invalid");
  const truncated = (await buildFixtureXlsx([gridSheet("T", 2, 2)])).subarray(0, 200);
  assert.equal((await openXlsx(truncated)).status, "invalid");
});
