import { cellValueAsString } from "@office-kit/xlsx/cell";
import { fromArrayBuffer, loadWorkbook } from "@office-kit/xlsx/io";
import { OpenXmlContentLimitError, OpenXmlDecompressionBombError } from "@office-kit/xlsx/utils";
import { getSheet, sheetNames, type Workbook } from "@office-kit/xlsx/workbook";
import { getValueExtent, iterValues } from "@office-kit/xlsx/worksheet";
import { XLSX_LIMITS, pageWindow, type CellBox, type PageWindow, type XlsxLimits } from "./xlsx-limits.ts";

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

/**
 * Parses the workbook (source `parseSheets`). The caps are enforced by the
 * library while it reads — no separate pre-inflate pass. Only cell values are
 * read: formulas show their stored cached value and are never evaluated; VBA
 * projects and external links stay opaque package parts that are neither run
 * nor fetched.
 */
export async function openXlsx(bytes: Uint8Array, limits: XlsxLimits = XLSX_LIMITS): Promise<XlsxOpenResult> {
  let workbook: Workbook;
  try {
    workbook = await loadWorkbook(fromArrayBuffer(bytes), {
      decompressionLimits: { maxTotalUncompressedBytes: limits.maxExpandedBytes },
      contentLimits: { maxCells: limits.maxCells, maxRows: limits.maxRows },
    });
  } catch (error) {
    if (error instanceof OpenXmlDecompressionBombError || error instanceof OpenXmlContentLimitError) {
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
        const rows = [...iterValues(sheet, window.box)].map((row) => row.map((value) => cellValueAsString(value)));
        return { ...window, rows };
      },
    },
  };
}

