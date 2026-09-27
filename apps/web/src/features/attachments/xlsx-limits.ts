/** Largest XLSX the browser viewer downloads; bigger files stay download-only. */
export const XLSX_MAX_BYTES = 32 * 1024 * 1024;

/**
 * Source `LOAD_OPTS`: inflated bytes across the whole package, and the cells
 * and rows the parser may model. @office-kit/xlsx enforces all three while it
 * reads (declared sizes, then every inflated chunk; each cell and row before it
 * is built), so nothing is inflated or parsed ahead of these checks.
 */
export const XLSX_MAX_EXPANDED_BYTES = 32 * 1024 * 1024;
export const XLSX_MAX_CELLS = 100_000;
export const XLSX_MAX_ROWS = 20_000;

/** Source paging: one table shows at most 200 rows × 64 columns. */
export const XLSX_ROWS_PER_PAGE = 200;
export const XLSX_COLS_PER_PAGE = 64;

export type XlsxLimits = {
  maxExpandedBytes: number;
  maxCells: number;
  maxRows: number;
};

export const XLSX_LIMITS: XlsxLimits = {
  maxExpandedBytes: XLSX_MAX_EXPANDED_BYTES,
  maxCells: XLSX_MAX_CELLS,
  maxRows: XLSX_MAX_ROWS,
};

/** Inclusive 1-based cell box, as @office-kit/xlsx `getValueExtent` returns it. */
export type CellBox = {
  minRow: number;
  maxRow: number;
  minCol: number;
  maxCol: number;
};

export type PageWindow = {
  rowPage: number;
  colPage: number;
  rowPages: number;
  colPages: number;
  box: CellBox;
};

export function pageCount(span: number, perPage: number): number {
  return Math.max(1, Math.ceil(span / perPage));
}

function clampPage(page: number, pages: number): number {
  if (!Number.isFinite(page) || page < 0) return 0;
  return Math.min(Math.floor(page), pages - 1);
}

/**
 * The cells one page shows: row page `rowPage` and column page `colPage` of
 * `extent`, each clamped into range, so a stale page index after a sheet
 * switch still lands on a real page.
 */
export function pageWindow(
  extent: CellBox,
  rowPage: number,
  colPage: number,
  rowsPerPage: number = XLSX_ROWS_PER_PAGE,
  colsPerPage: number = XLSX_COLS_PER_PAGE,
): PageWindow {
  const rowPages = pageCount(extent.maxRow - extent.minRow + 1, rowsPerPage);
  const colPages = pageCount(extent.maxCol - extent.minCol + 1, colsPerPage);
  const row = clampPage(rowPage, rowPages);
  const col = clampPage(colPage, colPages);
  const minRow = extent.minRow + row * rowsPerPage;
  const minCol = extent.minCol + col * colsPerPage;
  return {
    rowPage: row,
    colPage: col,
    rowPages,
    colPages,
    box: {
      minRow,
      maxRow: Math.min(extent.maxRow, minRow + rowsPerPage - 1),
      minCol,
      maxCol: Math.min(extent.maxCol, minCol + colsPerPage - 1),
    },
  };
}
