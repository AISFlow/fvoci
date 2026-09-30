import { cellValueAsString } from "@office-kit/xlsx/cell";
import { fromArrayBuffer, loadWorkbook } from "@office-kit/xlsx/io";
import { OpenXmlContentLimitError, OpenXmlDecompressionBombError } from "@office-kit/xlsx/utils";
import { getSheet, sheetNames, type Workbook } from "@office-kit/xlsx/workbook";
import { getValueExtent, iterValues } from "@office-kit/xlsx/worksheet";
import { unzipSync } from "fflate";
import {
  XLSX_LIMITS,
  pageWindow,
  type CellBox,
  type PageWindow,
  type XlsxLimits,
} from "./xlsx-limits.ts";

/** A tab of the workbook: a worksheet, or a chartsheet/other slot shown as unavailable. */
export type XlsxSheet = { name: string; kind: "worksheet" } | { name: string; kind: "unsupported" };

export type XlsxPage = PageWindow & {
  /** Display text per cell of `box`, row-major. */
  rows: string[][];
};

export type XlsxBook = {
  sheets: XlsxSheet[];
  /** Page of worksheet `index`; `null` for an unsupported or empty sheet. */
  page(index: number, rowPage: number, colPage: number): XlsxPage | null;
};

export type XlsxOpenResult =
  | { status: "ok"; book: XlsxBook }
  /** Over a load cap: the file stays download-only. */
  | { status: "tooLarge" }
  /** Not a readable XLSX package (corrupt, encrypted, legacy .xls, …). */
  | { status: "invalid" };

const OVER_CAP = Symbol("xlsx package over cap");

/**
 * Declared-metadata check that runs before @office-kit/xlsx sees the bytes.
 *
 * The library's strict reader enforces the byte cap while it inflates, but a
 * malformed ZIP32 central directory makes it fall back to fflate's
 * `unzipSync(bytes)`, which allocates every part at its declared size and
 * inflates it before the library's post-hoc cap check; its entry loop also
 * trusts a ZIP64 entry count the strict reader ignored. This walks the same
 * directory with the same fflate call, so the fallback's view is exactly the
 * one checked here, and inflates nothing (`filter` always returns `false`).
 *
 * Every record the walk reaches is charged its larger size: the fallback
 * allocates the declared original size for a deflated part and copies the
 * stored bytes of a stored one, and records may share a body, so compressed
 * sizes are counted per record too. Over `maxEntries` records or
 * `maxExpandedBytes` bytes (including a non-finite sum) is `tooLarge`, which
 * also stops the entry loop; any other walk failure is `invalid`.
 *
 * Only declared sizes are bounded here, not how much work inflating a
 * record's stream takes: the fallback decodes a deflated stream to its end
 * even past the declared size (the extra output is dropped). The viewer
 * therefore parses in a worker with a wall-clock bound (`xlsx-client.ts`).
 */
export function checkXlsxPackage(
  bytes: Uint8Array,
  limits: XlsxLimits = XLSX_LIMITS,
): "ok" | "tooLarge" | "invalid" {
  let entries = 0;
  let declared = 0;
  try {
    unzipSync(bytes, {
      filter(file) {
        entries += 1;
        declared += Math.max(file.size, file.originalSize);
        if (entries > limits.maxEntries || !(declared <= limits.maxExpandedBytes)) throw OVER_CAP;
        return false;
      },
    });
  } catch (error) {
    return error === OVER_CAP ? "tooLarge" : "invalid";
  }
  return "ok";
}

/**
 * Parses the workbook (source `parseSheets`). `checkXlsxPackage` bounds the
 * declared part sizes and count first; the library then enforces the byte cap
 * while it inflates and the cell and row caps while it parses. Only cell
 * values are read: formulas show their stored cached value and are never
 * evaluated; VBA projects and external links stay opaque package parts that
 * are neither run nor fetched.
 */
export async function openXlsx(
  bytes: Uint8Array,
  limits: XlsxLimits = XLSX_LIMITS,
): Promise<XlsxOpenResult> {
  const checked = checkXlsxPackage(bytes, limits);
  if (checked !== "ok") return { status: checked };
  let workbook: Workbook;
  try {
    workbook = await loadWorkbook(fromArrayBuffer(bytes), {
      decompressionLimits: { maxTotalUncompressedBytes: limits.maxExpandedBytes },
      contentLimits: { maxCells: limits.maxCells, maxRows: limits.maxRows },
    });
  } catch (error) {
    if (
      error instanceof OpenXmlDecompressionBombError ||
      error instanceof OpenXmlContentLimitError
    ) {
      return { status: "tooLarge" };
    }
    return { status: "invalid" };
  }
  const sheets: XlsxSheet[] = sheetNames(workbook).map((name) =>
    getSheet(workbook, name) ? { name, kind: "worksheet" } : { name, kind: "unsupported" },
  );
  const extents = new Map<number, CellBox | null>();
  return {
    status: "ok",
    book: {
      sheets,
      page(index, rowPage, colPage) {
        const entry = sheets[index];
        if (!entry || entry.kind !== "worksheet") return null;
        const sheet = getSheet(workbook, entry.name);
        if (!sheet) return null;
        let extent = extents.get(index);
        if (extent === undefined) {
          extent = getValueExtent(sheet) ?? null;
          extents.set(index, extent);
        }
        if (extent === null) return null;
        const window = pageWindow(extent, rowPage, colPage);
        const rows = [...iterValues(sheet, window.box)].map((row) =>
          row.map((value) => cellValueAsString(value)),
        );
        return { ...window, rows };
      },
    },
  };
}
