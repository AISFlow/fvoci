import assert from "node:assert/strict";
import test from "node:test";
import { randomBytes } from "node:crypto";
import { deflateRawSync } from "node:zlib";
import { fromArrayBuffer, loadWorkbook } from "@office-kit/xlsx/io";
import { OpenXmlDecompressionBombError } from "@office-kit/xlsx/utils";
import { writeZip } from "./docx-test-fixture.ts";
import { buildFixtureXlsx, gridSheet } from "./xlsx-test-fixture.ts";
import { XLSX_LIMITS, type XlsxLimits } from "./xlsx-limits.ts";
import { checkXlsxPackage, openXlsx, type XlsxBook } from "./xlsx-workbook.ts";

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
    await buildFixtureXlsx([
      { name: "Big", cells: [{ ref: "A1", inline: "x".repeat(64 * 1024) }] },
    ]),
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
  assert.equal(
    (await openXlsx(new TextEncoder().encode("plain text, not a zip"))).status,
    "invalid",
  );
  assert.equal((await openXlsx(new Uint8Array(0))).status, "invalid");
  const truncated = (await buildFixtureXlsx([gridSheet("T", 2, 2)])).subarray(0, 200);
  assert.equal((await openXlsx(truncated)).status, "invalid");
});

// --- Malformed directories: the library's unzipSync fallback ----------------

/** The library alone, with the options `openXlsx` passes it. */
async function libraryLoad(
  bytes: Uint8Array,
  limits: XlsxLimits,
): Promise<"ok" | "tooLarge" | "invalid"> {
  try {
    await loadWorkbook(fromArrayBuffer(bytes), {
      decompressionLimits: { maxTotalUncompressedBytes: limits.maxExpandedBytes },
      contentLimits: { maxCells: limits.maxCells, maxRows: limits.maxRows },
    });
    return "ok";
  } catch (error) {
    return error instanceof OpenXmlDecompressionBombError ? "tooLarge" : "invalid";
  }
}

/**
 * Copy of a comment-less fixture ZIP whose first central-directory record has
 * a broken signature (the strict reader then falls back to fflate's
 * `unzipSync`), optionally declaring `originalSize` for that record.
 */
function brokenDirectory(zip: Uint8Array, originalSize?: number): Uint8Array {
  const bytes = zip.slice();
  const view = new DataView(bytes.buffer);
  const eocd = bytes.byteLength - 22;
  assert.equal(view.getUint32(eocd, true), 0x06054b50);
  const cd = view.getUint32(eocd + 16, true);
  assert.equal(view.getUint32(cd, true), 0x02014b50);
  bytes[cd] = 0;
  if (originalSize !== undefined) view.setUint32(cd + 24, originalSize, true);
  return bytes;
}

/**
 * 98 bytes: a ZIP64 end record claiming `count` entries, its locator, and a
 * regular end record with one entry and no ZIP64 sentinels. The strict reader
 * ignores the ZIP64 records and falls back on the bad directory; fflate's
 * `unzipSync` honours the ZIP64 count and runs its entry loop `count` times.
 */
function zip64Count(count: number): Uint8Array {
  const bytes = new Uint8Array(56 + 20 + 22);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, 0x06064b50, true);
  view.setUint32(32, count, true);
  view.setUint32(56, 0x07064b50, true);
  view.setUint32(76, 0x06054b50, true);
  view.setUint16(76 + 8, 1, true);
  view.setUint16(76 + 10, 1, true);
  return bytes;
}

const smallCap: XlsxLimits = { ...XLSX_LIMITS, maxExpandedBytes: 1024 * 1024 };

test("negative control: with a broken directory the library inflates through its fallback", async () => {
  const zip = await buildFixtureXlsx([gridSheet("S", 3, 3)], { deflate: true });
  // The fallback still reads the workbook, so a lenient reader stays usable.
  assert.equal(await libraryLoad(brokenDirectory(zip), smallCap), "ok");
  assert.equal((await openXlsx(brokenDirectory(zip), smallCap)).status, "ok");
  // A part declaring 2 MiB is allocated at that size and inflated; the
  // library's post-hoc check only sees the few bytes the stream produced.
  assert.equal(await libraryLoad(brokenDirectory(zip, 2 * 1024 * 1024), smallCap), "ok");
});

test("declared sizes over the cap are refused before the fallback allocates or inflates", async () => {
  const zip = await buildFixtureXlsx([gridSheet("S", 3, 3)], { deflate: true });
  const overCap = brokenDirectory(zip, 2 * 1024 * 1024);
  assert.equal(checkXlsxPackage(overCap, smallCap), "tooLarge");
  // The library alone answers "ok" (above), so "tooLarge" comes from the check.
  assert.equal((await openXlsx(overCap, smallCap)).status, "tooLarge");
  assert.equal(checkXlsxPackage(brokenDirectory(zip, 512 * 1024), smallCap), "ok");
});

test("compressed sizes are charged too, so a small declared size cannot hide a large stream", async () => {
  const data = randomBytes(12 * 1024);
  const zip = writeZip([
    { name: "xl/workbook.xml", deflated: deflateRawSync(data), crc: 0, size: 1 },
  ]);
  const limits = { ...XLSX_LIMITS, maxExpandedBytes: 8 * 1024 };
  assert.equal(await libraryLoad(brokenDirectory(zip), limits), "invalid");
  assert.equal(checkXlsxPackage(brokenDirectory(zip), limits), "tooLarge");
  assert.equal((await openXlsx(brokenDirectory(zip), limits)).status, "tooLarge");
});

test("negative control: the fallback's entry loop runs a ZIP64 count the strict reader ignored", async () => {
  // 20,001 loop iterations end in a missing-part error, not a cap.
  assert.equal(
    await libraryLoad(zip64Count(XLSX_LIMITS.maxEntries * 2 + 1), XLSX_LIMITS),
    "invalid",
  );
});

test("the part-count cap stops that loop before the library runs it", async () => {
  assert.equal(checkXlsxPackage(zip64Count(XLSX_LIMITS.maxEntries)), "ok");
  assert.equal((await openXlsx(zip64Count(XLSX_LIMITS.maxEntries))).status, "invalid");
  assert.equal(checkXlsxPackage(zip64Count(XLSX_LIMITS.maxEntries + 1)), "tooLarge");
  assert.equal((await openXlsx(zip64Count(XLSX_LIMITS.maxEntries * 2 + 1))).status, "tooLarge");
  // 0xFFFFFFFF iterations in the library alone; here the walk stops at 10,001.
  assert.equal((await openXlsx(zip64Count(0xffffffff))).status, "tooLarge");
});
