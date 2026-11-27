/**
 * The messages between the viewer's frame and its spreadsheet worker
 * (`xlsx-worker.ts`): the frame hands over the workbook's bytes (the archive
 * it wrote of the parts it counted, `zip-guard.ts`), the worker reads all of
 * it and answers once, with what the frame needs to draw it or why it could
 * not be read. The frame takes an answer only in these shapes
 * (`readFromXlsxWorker`). A module without imports, so the frame's chunk
 * takes it without the worker's code.
 *
 * Cells go as columns of typed arrays, one entry per cell, and every text as
 * an index into one table of strings: the answer is moved, not copied, and
 * a string shown in a thousand cells crosses once.
 */

const HEX = /^[0-9A-Fa-f]{6}$/;

/**
 * A colour as the answer carries it (`#rrggbb`), from one as a workbook
 * writes it (`RRGGBB` or `AARRGGBB`, alpha left aside); '' when it is none.
 */
export function rgbColor(value: string | undefined): string {
  if (value === undefined) return '';
  const hex = value.length === 8 ? value.slice(2) : value;
  return HEX.test(hex) ? `#${hex.toLowerCase()}` : '';
}

/** What a cell holds. */
export const NUMBER = 0;
export const STRING = 1;
export const BOOLEAN = 2;
export const ERROR = 3;
/** Nothing but a style: a fill or a border. */
export const BLANK = 4;

/** The most rows and columns a sheet has, as in Excel. */
export const MAX_ROWS = 1_048_576;
export const MAX_COLUMNS = 16_384;
/**
 * The widest a column is drawn, in pixels at 100%: Excel's widest column
 * (255 characters) of a large default font is far wider than any screen.
 */
export const MAX_COLUMN_PIXELS = 8_192;

/** How a cell's text lies across it, as `alignment@horizontal` names it; 0 is "general". */
export const HORIZONTAL = [
  'general',
  'left',
  'center',
  'right',
  'fill',
  'justify',
  'centerContinuous',
  'distributed',
] as const;
/** And up and down; 0 is the default, the bottom. */
export const VERTICAL = ['bottom', 'top', 'center', 'justify', 'distributed'] as const;
/** The styles of a border's line, as `border/*@style` names them; 0 is none. */
export const LINES = [
  'none',
  'thin',
  'medium',
  'dashed',
  'dotted',
  'thick',
  'double',
  'hair',
  'mediumDashed',
  'dashDot',
  'mediumDashDot',
  'dashDotDot',
  'mediumDashDotDot',
  'slantDashDot',
] as const;

/** A colour: `#rrggbb`, or '' for none (the default). */
export type Color = string;

export interface Font {
  bold: boolean;
  italic: boolean;
  /** 0 none, 1 single, 2 double. */
  underline: number;
  strike: boolean;
  /** In points. */
  size: number;
  color: Color;
  /** A family name, of letters, digits, spaces and `-_.` only; '' for the frame's own. */
  family: string;
}

/** A border's four sides: the line's style (an index of `LINES`) and colour. */
export interface Border {
  left: [number, Color];
  right: [number, Color];
  top: [number, Color];
  bottom: [number, Color];
}

/** A cell's style (`cellXfs/xf`), by indices into the tables of the answer. */
export interface Style {
  format: number;
  font: number;
  /** Its fill's colour, '' for none. */
  fill: Color;
  border: number;
  horizontal: number;
  vertical: number;
  wrap: boolean;
  /** Indented by this many steps (of three spaces' width). */
  indent: number;
}

export interface Sheet {
  name: string;
  /** It has charts, pictures or shapes, which are not drawn. */
  drawings: boolean;
  gridLines: boolean;
  /** Frozen rows at the top and columns at the left. */
  frozenRows: number;
  frozenColumns: number;
  /** In points. */
  defaultRowHeight: number;
  /** In pixels at 100%. */
  defaultColumnWidth: number;
  /** Runs of columns: first, last (from 0), width in pixels (-1 the default), hidden (0 or 1). */
  columns: Float64Array;
  /** Rows of their own height or hidden: row (from 0), height in points (-1 the default), hidden. */
  rows: Float64Array;
  /**
   * The style of every cell of a row not written in it (`row@s` with
   * `customFormat`): row (from 0) and style, in order of rows, each once.
   */
  rowStyles: Uint32Array;
  /**
   * The style of every cell of a run of columns not written in it, unless
   * its row has one (`col@style`): first, last (from 0) and style; where
   * runs overlap, as their widths do.
   */
  columnStyles: Uint32Array;
  /** The cells, by row and then column. */
  cellRows: Uint32Array;
  cellColumns: Uint16Array;
  kinds: Uint8Array;
  styles: Uint32Array;
  /** A number; for text and errors the index of the string; for a boolean 0 or 1. */
  values: Float64Array;
  /** Merged ranges: first row, first column, last row, last column (from 0). */
  merges: Uint32Array;
}

export interface Workbook {
  date1904: boolean;
  strings: string[];
  /** The number formats by index (a style's `format`): a workbook's own code, or a built-in one's number. */
  formats: (string | number)[];
  fonts: Font[];
  borders: Border[];
  styles: Style[];
  /** The colours of `[ColorN]` in a format code: the workbook's palette. */
  palette: Color[];
  sheets: Sheet[];
}

export interface ToXlsxWorker {
  bytes: ArrayBuffer;
}

export type XlsxFailure = 'unreadable' | 'too-large' | 'renderer';

export type FromXlsxWorker =
  { kind: 'read'; workbook: Workbook } | { kind: 'failed'; reason: XlsxFailure };

/** The buffers of a workbook's typed arrays, to move rather than copy. */
export function transfers(workbook: Workbook): ArrayBuffer[] {
  return workbook.sheets.flatMap((sheet) =>
    [
      sheet.columns,
      sheet.rows,
      sheet.rowStyles,
      sheet.columnStyles,
      sheet.cellRows,
      sheet.cellColumns,
      sheet.kinds,
      sheet.styles,
      sheet.values,
      sheet.merges,
    ].map((array) => array.buffer as ArrayBuffer),
  );
}
