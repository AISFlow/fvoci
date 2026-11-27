/**
 * The spreadsheet worker's answer as the frame takes it: in the shapes of
 * `xlsx-protocol.ts` and no other, every index within what it indexes and
 * every value within what the frame draws (the cells in order of row and
 * column, as the panes search them), or not at all. What the frame draws a
 * sheet from is then only what was checked here.
 */
import { SHEET_LIMIT, SHEET_NAME_LENGTH } from '$lib/file-viewer/protocol.ts';

import {
  BLANK,
  ERROR,
  HORIZONTAL,
  LINES,
  MAX_COLUMN_PIXELS,
  MAX_COLUMNS,
  MAX_ROWS,
  STRING,
  VERTICAL,
  type Border,
  type Font,
  type FromXlsxWorker,
  type Sheet,
  type Style,
  type Workbook,
} from './xlsx-protocol.ts';

const COLOR = /^(?:#[0-9a-f]{6})?$/;
const FAMILY = /^[\p{L}\p{N} ._-]{0,64}$/u;
const CODE_LENGTH = 255;

type Fields = Record<string, unknown>;

const isRecord = (value: unknown): value is Fields => typeof value === 'object' && value !== null;
const isWhole = (value: unknown, least: number, most: number): value is number =>
  Number.isInteger(value) && (value as number) >= least && (value as number) <= most;
const isNumber = (value: unknown, least: number, most: number): value is number =>
  typeof value === 'number' && value >= least && value <= most;
const isColor = (value: unknown): value is string => typeof value === 'string' && COLOR.test(value);

/** Whether every entry of an array passes `check`. */
function every(value: unknown, check: (entry: unknown) => boolean): value is unknown[] {
  return Array.isArray(value) && value.every((entry) => check(entry));
}

const isFont = (value: unknown): value is Font =>
  isRecord(value) &&
  typeof value.bold === 'boolean' &&
  typeof value.italic === 'boolean' &&
  isWhole(value.underline, 0, 2) &&
  typeof value.strike === 'boolean' &&
  isNumber(value.size, 1, 409) &&
  isColor(value.color) &&
  typeof value.family === 'string' &&
  FAMILY.test(value.family);

const isSide = (value: unknown): boolean =>
  Array.isArray(value) &&
  value.length === 2 &&
  isWhole(value[0], 0, LINES.length - 1) &&
  isColor(value[1]);

const isBorder = (value: unknown): value is Border =>
  isRecord(value) &&
  isSide(value.left) &&
  isSide(value.right) &&
  isSide(value.top) &&
  isSide(value.bottom);

function isStyle(value: unknown, formats: number, fonts: number, borders: number): value is Style {
  return (
    isRecord(value) &&
    isWhole(value.format, 0, formats - 1) &&
    isWhole(value.font, 0, fonts - 1) &&
    isColor(value.fill) &&
    isWhole(value.border, 0, borders - 1) &&
    isWhole(value.horizontal, 0, HORIZONTAL.length - 1) &&
    isWhole(value.vertical, 0, VERTICAL.length - 1) &&
    typeof value.wrap === 'boolean' &&
    isWhole(value.indent, 0, 250)
  );
}

/** Whether runs of a typed array, `size` numbers each, pass `check`. */
function runs(
  array: Float64Array | Uint32Array,
  size: number,
  check: (run: number[]) => boolean,
): boolean {
  if (array.length % size !== 0) return false;
  for (let at = 0; at < array.length; at += size) {
    if (!check(Array.from(array.subarray(at, at + size)))) return false;
  }
  return true;
}

/** Whether the cells are in their shapes: as many of each, in order of row and column, each value in range. */
function cellsInShape(sheet: Fields, strings: number, styles: number): boolean {
  const { cellRows, cellColumns, kinds, styles: styleOf, values } = sheet;
  if (!(cellRows instanceof Uint32Array) || !(cellColumns instanceof Uint16Array)) return false;
  if (!(kinds instanceof Uint8Array) || !(styleOf instanceof Uint32Array)) return false;
  if (!(values instanceof Float64Array)) return false;
  const count = cellRows.length;
  if ([cellColumns, kinds, styleOf, values].some((array) => array.length !== count)) return false;
  let before = -1;
  for (let at = 0; at < count; at += 1) {
    const row = cellRows[at] ?? 0;
    const column = cellColumns[at] ?? 0;
    const place = row * MAX_COLUMNS + column;
    if (row >= MAX_ROWS || column >= MAX_COLUMNS || place <= before) return false;
    before = place;
    const kind = kinds[at] ?? 0;
    const value = values[at] ?? 0;
    if (kind > BLANK || (styleOf[at] ?? 0) >= Math.max(1, styles) || !Number.isFinite(value))
      return false;
    if ((kind === STRING || kind === ERROR) && !isWhole(value, 0, strings - 1)) return false;
  }
  return true;
}

/** Whether the rows of pairs of row and style are in order, each once. */
function ascending(pairs: Uint32Array): boolean {
  for (let at = 2; at < pairs.length; at += 2) {
    if ((pairs[at] ?? 0) <= (pairs[at - 2] ?? 0)) return false;
  }
  return true;
}

function isSheet(value: unknown, strings: number, styles: number): value is Sheet {
  if (!isRecord(value)) return false;
  const { columns, rows, merges } = value;
  if (!(columns instanceof Float64Array) || !(rows instanceof Float64Array)) return false;
  if (!(merges instanceof Uint32Array)) return false;
  if (!(value.rowStyles instanceof Uint32Array)) return false;
  if (!(value.columnStyles instanceof Uint32Array)) return false;
  return (
    typeof value.name === 'string' &&
    value.name.length <= SHEET_NAME_LENGTH &&
    typeof value.drawings === 'boolean' &&
    typeof value.gridLines === 'boolean' &&
    isWhole(value.frozenRows, 0, MAX_ROWS) &&
    isWhole(value.frozenColumns, 0, MAX_COLUMNS) &&
    isNumber(value.defaultRowHeight, 0, 409) &&
    isNumber(value.defaultColumnWidth, 0, MAX_COLUMN_PIXELS) &&
    runs(
      columns,
      4,
      ([first = 0, last = 0, width = 0, hidden = 0]) =>
        isWhole(first, 0, MAX_COLUMNS - 1) &&
        isWhole(last, first, MAX_COLUMNS - 1) &&
        (width === -1 || isNumber(width, 0, MAX_COLUMN_PIXELS)) &&
        isWhole(hidden, 0, 1),
    ) &&
    runs(
      rows,
      3,
      ([row = 0, height = 0, hidden = 0]) =>
        isWhole(row, 0, MAX_ROWS - 1) &&
        (height === -1 || isNumber(height, 0, 409)) &&
        isWhole(hidden, 0, 1),
    ) &&
    runs(
      value.rowStyles,
      2,
      ([row = 0, style = 0]) => row < MAX_ROWS && style < Math.max(1, styles),
    ) &&
    ascending(value.rowStyles) &&
    runs(
      value.columnStyles,
      3,
      ([first = 0, last = 0, style = 0]) =>
        first <= last && last < MAX_COLUMNS && style < Math.max(1, styles),
    ) &&
    runs(
      merges,
      4,
      ([top = 0, left = 0, bottom = 0, right = 0]) =>
        bottom < MAX_ROWS && right < MAX_COLUMNS && top <= bottom && left <= right,
    ) &&
    cellsInShape(value, strings, styles)
  );
}

function isWorkbook(value: unknown): value is Workbook {
  if (!isRecord(value) || typeof value.date1904 !== 'boolean') return false;
  const { strings, formats, fonts, borders, styles, palette, sheets } = value;
  if (!every(strings, (entry) => typeof entry === 'string')) return false;
  const isFormat = (entry: unknown): boolean =>
    isWhole(entry, 0, 0xffff) || (typeof entry === 'string' && entry.length <= CODE_LENGTH);
  if (!every(formats, isFormat) || !every(fonts, isFont) || !every(borders, isBorder)) return false;
  if (fonts.length === 0 || borders.length === 0) return false;
  const style = (entry: unknown): boolean =>
    isStyle(entry, formats.length, fonts.length, borders.length);
  if (!every(styles, style) || !every(palette, isColor) || palette.length > 64) return false;
  if (!Array.isArray(sheets) || sheets.length === 0 || sheets.length > SHEET_LIMIT) return false;
  return sheets.every((sheet) => isSheet(sheet, strings.length, styles.length));
}

/** The worker's answer, or undefined when it is not in one of the shapes. */
export function readFromXlsxWorker(data: unknown): FromXlsxWorker | undefined {
  if (!isRecord(data)) return undefined;
  if (data.kind === 'failed') {
    const reason = data.reason;
    return reason === 'unreadable' || reason === 'too-large' || reason === 'renderer'
      ? { kind: 'failed', reason }
      : undefined;
  }
  return data.kind === 'read' && isWorkbook(data.workbook)
    ? { kind: 'read', workbook: data.workbook }
    : undefined;
}
