/**
 * One worksheet part read into what the frame draws: its cells and their
 * values (a formula's stored value, never calculated), its columns' widths
 * and rows' heights, the styles of whole rows and columns, what is hidden, its frozen panes, its merged cells,
 * whether it shows grid lines, and whether it has drawings (charts,
 * pictures, shapes) that are not drawn.
 *
 * Only what is there is counted and kept: what a part says of itself (its
 * `dimension`, a row's `spans`) is not believed, a run of columns stays a
 * run, and a merge stays its corners. A reference past a sheet's edge
 * (Excel's 1,048,576 rows and 16,384 columns) makes the file unreadable,
 * as it does for Excel.
 */
import { serialOf, type DateSystem } from './xlsx-dates.ts';
import { Cells } from './xlsx-cells.ts';
import {
  BLANK,
  BOOLEAN,
  ERROR,
  MAX_COLUMN_PIXELS,
  MAX_COLUMNS,
  MAX_ROWS,
  NUMBER,
  STRING,
  type Sheet,
} from './xlsx-protocol.ts';
import { ItemText, type Strings } from './xlsx-strings.ts';
import {
  attribute,
  cellAt,
  decimal,
  flag,
  readXml,
  Unreadable,
  unescapeText,
  whole,
  type Attributes,
  type Ceiling,
} from './xlsx-xml.ts';

/** What reading a sheet counts toward, and reads against. */
export interface SheetContext {
  strings: Strings;
  /** How many shared strings there are: a cell of type `s` names one of them. */
  shared: number;
  styles: number;
  system: DateSystem;
  /** Pixels of a column one character wide, at the workbook's default font. */
  digitWidth: number;
  ceilings: { cells: Ceiling; merges: Ceiling; rows: Ceiling; columns: Ceiling };
}

/** The longest a value (`v`) is read; a number is far shorter, and a text is cut at a cell's longest. */
const VALUE_LENGTH = 40_000;

/** Pixels of a column `width` characters wide (ECMA-376 18.3.1.13). */
export function columnPixels(width: number, digitWidth: number): number {
  const pixels = Math.trunc(((256 * width + Math.trunc(128 / digitWidth)) / 256) * digitWidth);
  return Math.min(MAX_COLUMN_PIXELS, Math.max(0, pixels));
}

/** A cell being read. */
interface Reading {
  row: number;
  column: number;
  type: string;
  style: number;
  value: string;
  inValue: boolean;
  inline?: ItemText;
}

class SheetReader {
  readonly #context: SheetContext;
  readonly cells = new Cells();
  readonly columns: number[] = [];
  readonly rows: number[] = [];
  /** Row and style of each row with a style of its own: in the order read. */
  readonly rowStyles: number[] = [];
  readonly columnStyles: number[] = [];
  readonly merges: number[] = [];
  drawings = false;
  gridLines = true;
  frozenRows = 0;
  frozenColumns = 0;
  defaultRowHeight = 15;
  defaultColumnWidth = 0;
  #views = 0;
  #row = -1;
  /** The row being read has a style of its own. */
  #rowStyled = false;
  #column = -1;
  #cell: Reading | undefined;

  constructor(context: SheetContext) {
    this.#context = context;
    this.defaultColumnWidth = this.#baseWidth(8);
  }

  /** The default width of a column `characters` wide, as Excel rounds it: to whole eighths of 8 pixels. */
  #baseWidth(characters: number): number {
    return Math.min(
      MAX_COLUMN_PIXELS,
      Math.ceil((characters * this.#context.digitWidth + 5) / 8) * 8,
    );
  }

  open(name: string, attributes: Attributes): void {
    const cell = this.#cell;
    if (cell) {
      if (name === 'v') cell.inValue = true;
      else if (name === 'is') cell.inline = new ItemText();
      else cell.inline?.open(name);
      return;
    }
    switch (name) {
      case 'row':
        this.#openRow(attributes);
        break;
      case 'c':
        this.#openCell(attributes);
        break;
      case 'col':
        this.#openColumns(attributes);
        break;
      case 'mergeCell':
        this.#merge(attribute(attributes, 'ref') ?? '');
        break;
      case 'sheetFormatPr':
        this.#format(attributes);
        break;
      case 'sheetView':
        this.#views += 1;
        if (this.#views === 1) this.gridLines = flag(attribute(attributes, 'showGridLines'), true);
        break;
      case 'pane':
        this.#pane(attributes);
        break;
      case 'drawing':
      case 'picture':
        this.drawings = true;
        break;
    }
  }

  close(name: string): void {
    const cell = this.#cell;
    if (!cell) return;
    if (name === 'c') {
      this.#closeCell(cell);
      this.#cell = undefined;
    } else if (name === 'v') cell.inValue = false;
    else cell.inline?.close(name);
  }

  text(text: string): void {
    const cell = this.#cell;
    if (!cell) return;
    if (cell.inValue) cell.value += text.slice(0, Math.max(0, VALUE_LENGTH - cell.value.length));
    else cell.inline?.text(text);
  }

  #openRow(attributes: Attributes): void {
    const given = attribute(attributes, 'r');
    const row = given === undefined ? this.#row + 1 : (whole(given, 1, MAX_ROWS) ?? 0) - 1;
    if (row < 0 || row >= MAX_ROWS) throw new Unreadable('a row past the edge');
    this.#row = row;
    this.#column = -1;
    const height = decimal(attribute(attributes, 'ht'), 0, 409);
    const hidden = flag(attribute(attributes, 'hidden'));
    // A row's style counts only with `customFormat` (ECMA-376 18.3.1.73).
    const style = flag(attribute(attributes, 'customFormat'))
      ? this.#style(attribute(attributes, 's'))
      : undefined;
    this.#rowStyled = style !== undefined;
    // A row of the default height is kept as one of no height of its own.
    const sized = hidden || (height !== undefined && height !== this.defaultRowHeight);
    if (sized || style !== undefined) this.#context.ceilings.rows.add();
    if (sized) this.rows.push(row, height ?? -1, hidden ? 1 : 0);
    if (style !== undefined) this.rowStyles.push(row, style);
  }

  /** A style index as a cell, a row or a column gives it: one of the workbook's, or 0. */
  #style(given: string | undefined): number {
    const style = whole(given, 0, 0xffffffff) ?? 0;
    return style < this.#context.styles ? style : 0;
  }

  #openCell(attributes: Attributes): void {
    const reference = attribute(attributes, 'r');
    let row = Math.max(0, this.#row);
    let column = this.#column + 1;
    if (reference !== undefined) {
      const at = cellAt(reference);
      if (!at) throw new Unreadable('a cell past the edge');
      [row, column] = at;
    }
    if (column >= MAX_COLUMNS) throw new Unreadable('a cell past the edge');
    this.#column = column;
    this.#context.ceilings.cells.add();
    this.#cell = {
      row,
      column,
      type: attribute(attributes, 't') ?? 'n',
      style: this.#style(attribute(attributes, 's')),
      value: '',
      inValue: false,
    };
  }

  #closeCell(cell: Reading): void {
    const { strings, shared, system } = this.#context;
    const add = (kind: number, value: number): void => {
      this.cells.add(cell.row, cell.column, kind, cell.style, value);
    };
    // A cell written without a value stays when it has a style, or when its row or a column
    // has one: its own (0 when it names none) is the one it shows, not theirs.
    const kept = cell.style !== 0 || this.#rowStyled || this.columnStyles.length > 0;
    const value = cell.value;
    switch (cell.type) {
      case 's': {
        const index = whole(value, 0, shared - 1);
        if (index !== undefined) add(STRING, index);
        else if (kept) add(BLANK, 0);
        return;
      }
      case 'inlineStr':
        add(STRING, strings.intern(cell.inline?.take() ?? unescapeText(value)));
        return;
      case 'str':
        add(STRING, strings.intern(unescapeText(value)));
        return;
      case 'b':
        add(BOOLEAN, value.trim() === '1' || value.trim() === 'true' ? 1 : 0);
        return;
      case 'e':
        add(ERROR, strings.intern(value.trim()));
        return;
      case 'd': {
        const serial = serialOf(value, system);
        if (serial !== undefined) add(NUMBER, serial);
        else if (value.trim() !== '') add(STRING, strings.intern(value.trim()));
        return;
      }
      default: {
        const number = value.trim() === '' ? Number.NaN : Number(value);
        if (Number.isFinite(number)) add(NUMBER, number);
        else if (kept) add(BLANK, 0);
      }
    }
  }

  #openColumns(attributes: Attributes): void {
    const first = whole(attribute(attributes, 'min'), 1, MAX_COLUMNS);
    const last = whole(attribute(attributes, 'max'), 1, MAX_COLUMNS);
    if (first === undefined || last === undefined || last < first) return;
    this.#context.ceilings.columns.add();
    const width = decimal(attribute(attributes, 'width'), 0, 255);
    const hidden = flag(attribute(attributes, 'hidden'));
    const pixels = width === undefined ? -1 : columnPixels(width, this.#context.digitWidth);
    this.columns.push(first - 1, last - 1, pixels, hidden ? 1 : 0);
    const style = attribute(attributes, 'style');
    if (style !== undefined) this.columnStyles.push(first - 1, last - 1, this.#style(style));
  }

  #merge(reference: string): void {
    const [from = '', to = from] = reference.split(':');
    const start = cellAt(from);
    const end = cellAt(to);
    if (!start || !end) throw new Unreadable('a merge past the edge');
    this.#context.ceilings.merges.add();
    const [top, bottom] = [Math.min(start[0], end[0]), Math.max(start[0], end[0])];
    const [left, right] = [Math.min(start[1], end[1]), Math.max(start[1], end[1])];
    // One cell is no merge.
    if (top !== bottom || left !== right) this.merges.push(top, left, bottom, right);
  }

  #format(attributes: Attributes): void {
    this.defaultRowHeight = decimal(attribute(attributes, 'defaultRowHeight'), 0, 409) ?? 15;
    const width = decimal(attribute(attributes, 'defaultColWidth'), 0, 255);
    const base = decimal(attribute(attributes, 'baseColWidth'), 0, 255);
    if (width !== undefined)
      this.defaultColumnWidth = columnPixels(width, this.#context.digitWidth);
    else if (base !== undefined) this.defaultColumnWidth = this.#baseWidth(base);
  }

  #pane(attributes: Attributes): void {
    if (this.#views !== 1) return;
    const state = attribute(attributes, 'state');
    if (state !== 'frozen' && state !== 'frozenSplit') return;
    this.frozenColumns = whole(attribute(attributes, 'xSplit'), 0, MAX_COLUMNS) ?? 0;
    this.frozenRows = whole(attribute(attributes, 'ySplit'), 0, MAX_ROWS) ?? 0;
  }
}

/** Pairs of row and style in order of rows, the last of a row kept (a row written twice). */
function byRow(pairs: readonly number[]): Uint32Array {
  const last = new Map<number, number>();
  for (let at = 0; at + 1 < pairs.length; at += 2) last.set(pairs[at] ?? 0, pairs[at + 1] ?? 0);
  return Uint32Array.from([...last].sort(([a], [b]) => a - b).flat());
}

/** Reads a worksheet's part. */
export function readSheet(name: string, text: string, context: SheetContext): Sheet {
  const reader = new SheetReader(context);
  readXml(text, {
    open: (tag, attributes) => {
      reader.open(tag, attributes);
    },
    close: (tag) => {
      reader.close(tag);
    },
    text: (value) => {
      reader.text(value);
    },
  });
  return {
    name,
    drawings: reader.drawings,
    gridLines: reader.gridLines,
    frozenRows: reader.frozenRows,
    frozenColumns: reader.frozenColumns,
    defaultRowHeight: reader.defaultRowHeight,
    defaultColumnWidth: reader.defaultColumnWidth,
    columns: Float64Array.from(reader.columns),
    rows: Float64Array.from(reader.rows),
    rowStyles: byRow(reader.rowStyles),
    columnStyles: Uint32Array.from(reader.columnStyles),
    ...reader.cells.done(),
    merges: Uint32Array.from(reader.merges),
  };
}

/** A sheet of nothing: a chart sheet, which is all drawing. */
export function drawingSheet(name: string, digitWidth: number): Sheet {
  return {
    name,
    drawings: true,
    gridLines: false,
    frozenRows: 0,
    frozenColumns: 0,
    defaultRowHeight: 15,
    defaultColumnWidth: Math.ceil((8 * digitWidth + 5) / 8) * 8,
    columns: new Float64Array(),
    rows: new Float64Array(),
    rowStyles: new Uint32Array(),
    columnStyles: new Uint32Array(),
    cellRows: new Uint32Array(),
    cellColumns: new Uint16Array(),
    kinds: new Uint8Array(),
    styles: new Uint32Array(),
    values: new Float64Array(),
    merges: new Uint32Array(),
  };
}
