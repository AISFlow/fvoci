/**
 * One pane of a sheet in the frame: a box of the view that shows a window
 * of rows and columns (the frozen ones, or those scrolled to), and draws in
 * it only what lies in that window: the grid lines, the rows, their cells
 * (and the empty ones a styled row or column fills or borders,
 * `xlsx-blank.ts`), and the merged ranges. As the window moves, what stays in it is kept and
 * moved, what leaves is taken out and what comes is made: the page holds
 * what the reader sees, not the sheet.
 *
 * However a sheet is made (rows a hair high, merges over merges, a million
 * cells in view), one drawing walks at most `ROW_WALK` rows, looks at no
 * more than `Budget.cells` cells and merges across the panes (drawn or
 * not: a cell under a merge is looked at too, and an empty one of a styled
 * row or column), draws `LINE_BUDGET` lines in
 * each direction and looks at `MERGE_BUDGET` merges; past them the rest of
 * the window stays empty.
 */
import type { Axis } from './xlsx-axis.ts';
import type { BlankStyles } from './xlsx-blank.ts';
import type { Looks } from './xlsx-look.ts';
import { BLANK, STRING, type Sheet } from './xlsx-protocol.ts';

export const ROW_WALK = 2_000;
export const CELL_BUDGET = 8_000;
export const LINE_BUDGET = 1_000;
export const MERGE_BUDGET = 500;
/** How many cells a text's run to the left or right is looked for across. */
const SPILL_WALK = 256;
/** Grid lines nearer each other than this, in pixels, are drawn as one. */
const LINE_GAP = 2;
/** Room at each side of a cell's text, in pixels at 100%. */
export const PADDING = 2;

/** The window a pane shows: its box in the view, the indices it may show, and where along the sheet it starts. */
export interface Window {
  left: number;
  top: number;
  width: number;
  height: number;
  /** The rows and columns it may show: from the first to before the second. */
  rows: [number, number];
  columns: [number, number];
  /** Where the pane's top and left edges lie along the sheet, in pixels at the zoom. */
  y: number;
  x: number;
}

/** What one drawing of every pane may still make. */
export interface Budget {
  cells: number;
}

/** A merged range: first row, first column, last row, last column. */
type Range = [number, number, number, number];

/** The first index of a sorted array at which the value is at least `value`, within `[low, high)`. */
export function lowerBound(
  array: ArrayLike<number>,
  value: number,
  low = 0,
  high = array.length,
): number {
  let from = low;
  let to = high;
  while (from < to) {
    const middle = (from + to) >> 1;
    if ((array[middle] ?? 0) < value) from = middle + 1;
    else to = middle;
  }
  return from;
}

/** Whether a merged range of `merges` covers the cell. */
const covers = (merges: readonly Range[], row: number, column: number): boolean =>
  merges.some(
    ([top, left, bottom, right]) =>
      row >= top && row <= bottom && column >= left && column <= right,
  );

/** Puts `elements` in `parent` in this order, before whatever else it holds. */
function order(parent: HTMLElement, elements: readonly HTMLElement[]): void {
  let cursor = parent.firstElementChild;
  for (const element of elements) {
    if (element === cursor) cursor = cursor.nextElementSibling;
    else parent.insertBefore(element, cursor);
  }
}

export class Pane {
  readonly element = document.createElement('div');
  readonly #lines = document.createElement('div');
  readonly #cells = document.createElement('div');
  readonly #sheet: Sheet;
  readonly #looks: Looks;
  readonly #rows: Axis;
  readonly #columns: Axis;
  readonly #blanks: BlankStyles;
  /** The columns in view of some width, and those of them whose style paints: of this drawing. */
  #inView: number[] | undefined;
  #painting: number[] | undefined;
  /** What is in the page, by key; what a drawing does not touch is taken out. */
  #drawn = new Map<string, HTMLElement>();
  #touched = new Set<string>();
  #zoom = 1;
  #y = 0;
  #x = 0;

  constructor(sheet: Sheet, looks: Looks, rows: Axis, columns: Axis, blanks: BlankStyles) {
    this.#sheet = sheet;
    this.#looks = looks;
    this.#rows = rows;
    this.#columns = columns;
    this.#blanks = blanks;
    this.element.className = 'xlsx-pane';
    this.#lines.className = 'xlsx-lines';
    this.#lines.setAttribute('aria-hidden', 'true');
    this.#cells.setAttribute('role', 'rowgroup');
    this.element.append(this.#lines, this.#cells);
  }

  /** Drops everything drawn: the next drawing makes it anew. */
  clear(): void {
    this.#lines.replaceChildren();
    this.#cells.replaceChildren();
    this.#drawn = new Map();
  }

  draw(window: Window, zoom: number, budget: Budget): void {
    if (zoom !== this.#zoom) {
      this.clear();
      this.#zoom = zoom;
    }
    this.#y = window.y;
    this.#x = window.x;
    const box = this.element.style;
    box.left = `${window.left}px`;
    box.top = `${window.top}px`;
    box.width = `${Math.max(0, window.width)}px`;
    box.height = `${Math.max(0, window.height)}px`;
    this.#touched = new Set();
    this.#inView = undefined;
    this.#painting = undefined;
    const empty = window.rows[0] >= window.rows[1] || window.columns[0] >= window.columns[1];
    if (!empty && window.width > 0 && window.height > 0) {
      const rows = this.#visibleRows(window);
      const columns = this.#visibleColumns(window);
      if (this.#sheet.gridLines) this.#drawLines(rows, columns);
      this.#drawRows(rows, columns, budget);
    }
    for (const [key, element] of this.#drawn) {
      if (!this.#touched.has(key)) {
        element.remove();
        this.#drawn.delete(key);
      }
    }
  }

  /** The rows of some size in the window, in order, at most `ROW_WALK` of them. */
  #visibleRows(window: Window): number[] {
    const zoom = this.#zoom;
    const first = Math.max(window.rows[0], this.#rows.indexAt(window.y / zoom));
    const last = Math.min(
      window.rows[1] - 1,
      this.#rows.indexAt((window.y + window.height) / zoom),
    );
    const rows: number[] = [];
    for (let row = first; row <= last && rows.length < ROW_WALK; row = this.#rows.next(row)) {
      if (this.#rows.size(row) > 0) rows.push(row);
    }
    return rows;
  }

  #visibleColumns(window: Window): [number, number] {
    const zoom = this.#zoom;
    const first = this.#columns.indexAt(window.x / zoom);
    const last = this.#columns.indexAt((window.x + window.width) / zoom);
    return [Math.max(first, window.columns[0]), Math.min(last, window.columns[1] - 1)];
  }

  /** The element under `key`, made by `make` when it is not in the page; marked as drawn. */
  #take(key: string, make: () => HTMLElement): HTMLElement {
    let element = this.#drawn.get(key);
    if (!element) {
      element = make();
      this.#drawn.set(key, element);
    }
    this.#touched.add(key);
    return element;
  }

  #drawLines(rows: readonly number[], [first, last]: [number, number]): void {
    const zoom = this.#zoom;
    const lines: HTMLElement[] = [];
    let previous = -Infinity;
    for (const row of rows) {
      const y = Math.round(this.#rows.start(row + 1) * zoom - this.#y);
      if (y - previous < LINE_GAP || lines.length >= LINE_BUDGET) continue;
      previous = y;
      const line = this.#take(`h${row}`, () => lineElement('xlsx-line-h'));
      line.style.top = `${y - 1}px`;
      lines.push(line);
    }
    previous = -Infinity;
    const across = lines.length;
    for (
      let column = first;
      column <= last && lines.length - across < LINE_BUDGET;
      column = this.#columns.next(column)
    ) {
      const x = Math.round(this.#columns.start(column + 1) * zoom - this.#x);
      if (x - previous < LINE_GAP) continue;
      previous = x;
      const line = this.#take(`v${column}`, () => lineElement('xlsx-line-v'));
      line.style.left = `${x - 1}px`;
      lines.push(line);
    }
    order(this.#lines, lines);
  }

  /** The merged ranges that reach into the rows and columns in view, at most `MERGE_BUDGET`. */
  #visibleMerges(rows: readonly number[], [first, last]: [number, number]): Range[] {
    const found: Range[] = [];
    const top = rows[0];
    const bottom = rows.at(-1);
    if (top === undefined || bottom === undefined) return found;
    const merges = this.#sheet.merges;
    for (let at = 0; at + 3 < merges.length && found.length < MERGE_BUDGET; at += 4) {
      const range: Range = [
        merges[at] ?? 0,
        merges[at + 1] ?? 0,
        merges[at + 2] ?? 0,
        merges[at + 3] ?? 0,
      ];
      if (range[0] <= bottom && range[2] >= top && range[1] <= last && range[3] >= first) {
        found.push(range);
      }
    }
    return found;
  }

  #drawRows(rows: readonly number[], columns: [number, number], budget: Budget): void {
    const merges = this.#visibleMerges(rows, columns);
    // Each merge hangs from the first of its rows in view.
    const hosted = new Map<number, Range[]>();
    for (const range of merges) {
      const host = Math.max(range[0], rows[0] ?? range[0]);
      const at = rows.find((row) => row >= host) ?? host;
      hosted.set(at, [...(hosted.get(at) ?? []), range]);
    }
    const cells = new Map<number, HTMLElement[]>();
    for (const row of rows) {
      if (budget.cells <= 0) break;
      const inRow = this.#rowCells(row, columns, merges, budget);
      for (const range of hosted.get(row) ?? []) {
        if (budget.cells <= 0) break;
        inRow.push(this.#merge(range, row));
        budget.cells -= 1;
      }
      cells.set(row, inRow);
    }
    // The empty cells of styled rows and columns with what the written ones left of the budget.
    const blanks = new Map<number, HTMLElement[]>();
    for (const row of rows) {
      if (budget.cells <= 0) break;
      blanks.set(row, this.#blankCells(row, columns, merges, budget));
    }
    const rowElements: HTMLElement[] = [];
    for (const row of rows) {
      const read = cells.get(row) ?? [];
      const inRow = [...read, ...(blanks.get(row) ?? [])];
      if (inRow.length === 0) continue;
      const element = this.#row(row);
      // A row of nothing but empty styled cells holds nothing to read.
      if (read.length === 0) element.setAttribute('aria-hidden', 'true');
      else element.removeAttribute('aria-hidden');
      order(element, inRow);
      rowElements.push(element);
    }
    order(this.#cells, rowElements);
  }

  /** The cells of a row in view, in order; first the one whose text runs in from the left. */
  #rowCells(
    row: number,
    columns: [number, number],
    merges: readonly Range[],
    budget: Budget,
  ): HTMLElement[] {
    const sheet = this.#sheet;
    const start = lowerBound(sheet.cellRows, row);
    const end = lowerBound(sheet.cellRows, row + 1, start);
    const first = lowerBound(sheet.cellColumns, columns[0], start, end);
    const drawn: HTMLElement[] = [];
    // The nearest cell to the left with a value: its text may run into view.
    let left = first - 1;
    while (left >= start && left > first - SPILL_WALK && sheet.kinds[left] === BLANK) left -= 1;
    if (left >= start) {
      const element = this.#cell(left, row, columns, merges, end, true);
      if (element) drawn.push(element);
      budget.cells -= 1;
    }
    // Every cell looked at spends the budget, drawn or not (under a merge, in a hidden column),
    // so that what a draw walks is bounded as what it makes is.
    for (let at = first; at < end && budget.cells > 0; at += 1) {
      if ((sheet.cellColumns[at] ?? 0) > columns[1]) break;
      budget.cells -= 1;
      const element = this.#cell(at, row, columns, merges, end, false);
      if (element) drawn.push(element);
    }
    return drawn;
  }

  /** The empty cells of a row in view that its or their column's style paints, in runs, made or kept. */
  #blankCells(
    row: number,
    columns: [number, number],
    merges: readonly Range[],
    budget: Budget,
  ): HTMLElement[] {
    const sheet = this.#sheet;
    const blanks = this.#blanks;
    if (blanks.row(row) < 0 && !blanks.columnsPaint) return [];
    // The columns in view of some width, found once a drawing.
    const inView = (this.#inView ??= this.#columns.sized(columns[0], columns[1], CELL_BUDGET));
    this.#painting ??= inView.filter((column) => blanks.paints(blanks.column(column)));
    const start = lowerBound(sheet.cellRows, row);
    const end = lowerBound(sheet.cellRows, row + 1, start);
    // From the first written cell in view: the columns are asked of in order.
    let at = lowerBound(sheet.cellColumns, columns[0], start, end);
    const runs = blanks.runs(row, inView, this.#painting, {
      written: (column) => {
        while (at < end && (sheet.cellColumns[at] ?? 0) < column) at += 1;
        return at < end && sheet.cellColumns[at] === column;
      },
      covered: (column) => covers(merges, row, column),
      next: (column) => this.#columns.next(column),
      budget,
    });
    return runs.map(([first, last, style]) => {
      const element = this.#take(`b${row}:${first}:${last}:${style}`, () => {
        const blank = this.#make(-1, first, row, row, last);
        // Not a cell to read: it holds nothing.
        for (const name of ['role', 'aria-colindex', 'aria-colspan']) blank.removeAttribute(name);
        blank.setAttribute('aria-hidden', 'true');
        this.#looks.paint(blank, this.#looks.style(style));
        return blank;
      });
      element.style.left = `${this.#columns.start(first) * this.#zoom - this.#x}px`;
      return element;
    });
  }

  #row(row: number): HTMLElement {
    const element = this.#take(`r${row}`, () => {
      const made = document.createElement('div');
      made.className = 'xlsx-row';
      made.setAttribute('role', 'row');
      made.setAttribute('aria-rowindex', String(row + 1));
      return made;
    });
    element.style.top = `${this.#rows.start(row) * this.#zoom - this.#y}px`;
    return element;
  }

  /** A cell's element in view, made or kept; undefined for a cell a merge covers or one out of view. */
  #cell(
    index: number,
    row: number,
    columns: [number, number],
    merges: readonly Range[],
    rowEnd: number,
    leftOfView: boolean,
  ): HTMLElement | undefined {
    const sheet = this.#sheet;
    const column = sheet.cellColumns[index] ?? 0;
    if (this.#columns.size(column) === 0) return undefined;
    if (covers(merges, row, column)) return undefined;
    const shown = this.#looks.shown(sheet, index);
    const style = this.#looks.style(sheet.styles[index] ?? 0);
    const spills =
      shown.align === 'left' && !style.wrap && sheet.kinds[index] === STRING && shown.text !== '';
    if (leftOfView && !spills) return undefined;
    const reach = spills
      ? this.#reach(index, row, column, columns[1] + 1, merges, rowEnd)
      : this.#columns.size(column);
    const x = this.#columns.start(column) * this.#zoom - this.#x;
    if (leftOfView && x + reach * this.#zoom <= 0) return undefined;
    const element = this.#take(`c${index}`, () => this.#make(index, column, row, row));
    element.style.left = `${x}px`;
    if (spills) {
      element.classList.add('xlsx-spills');
      this.#fit(element, reach);
    }
    return element;
  }

  /**
   * How far a cell's text may run to the right, in pixels at 100%: to the
   * next cell with a value or a merge, or to `end`, the column past the view.
   */
  #reach(
    index: number,
    row: number,
    column: number,
    end: number,
    merges: readonly Range[],
    rowEnd: number,
  ): number {
    const sheet = this.#sheet;
    let stop = end;
    for (let next = index + 1; next < rowEnd && next <= index + SPILL_WALK; next += 1) {
      const at = sheet.cellColumns[next] ?? 0;
      if (at >= stop) break;
      if (sheet.kinds[next] !== BLANK) {
        stop = at;
        break;
      }
    }
    for (const [top, left, bottom] of merges) {
      if (row >= top && row <= bottom && left > column && left < stop) stop = left;
    }
    const own = this.#columns.size(column);
    return Math.max(own, this.#columns.start(stop) - this.#columns.start(column));
  }

  /** Sets how wide a running text is, `reach` pixels at 100%. */
  #fit(element: HTMLElement, reach: number): void {
    const text = element.querySelector<HTMLElement>('.xlsx-text');
    if (text) text.style.width = `${Math.max(0, (reach - 2 * PADDING) * this.#zoom)}px`;
  }

  /**
   * A cell's element over rows `top` to `bottom` and columns `column` to
   * `right` (more than one for a merge): its fill, its text and its borders,
   * of the cell at `index` (-1 for an empty one).
   */
  #make(index: number, column: number, top: number, bottom: number, right = column): HTMLElement {
    const sheet = this.#sheet;
    const zoom = this.#zoom;
    const width = (this.#columns.start(right + 1) - this.#columns.start(column)) * zoom;
    const height = (this.#rows.start(bottom + 1) - this.#rows.start(top)) * zoom;
    const element = document.createElement('div');
    element.className = 'xlsx-cell';
    element.setAttribute('role', 'cell');
    element.setAttribute('aria-colindex', String(column + 1));
    if (bottom > top) element.setAttribute('aria-rowspan', String(bottom - top + 1));
    if (right > column) element.setAttribute('aria-colspan', String(right - column + 1));
    element.style.width = `${width}px`;
    element.style.height = `${height}px`;
    element.style.padding = `0 ${PADDING * zoom}px`;
    if (index < 0) return element;
    const style = this.#looks.style(sheet.styles[index] ?? 0);
    const shown = this.#looks.shown(sheet, index);
    this.#looks.dress(element, style, shown, zoom);
    if (shown.text !== '') {
      const text = document.createElement('span');
      text.className = style.wrap ? 'xlsx-text xlsx-wrap' : 'xlsx-text';
      text.style.width = `${Math.max(0, width - 2 * PADDING * zoom)}px`;
      // Without wrapping a cell shows its text on one line, line breaks and all.
      text.textContent = style.wrap ? shown.text : shown.text.replaceAll(/[\r\n]+/g, ' ');
      element.append(text);
    }
    const edges = this.#looks.edges(style);
    if (edges) element.append(edges);
    return element;
  }

  /** A merged range's element, hung from row `host`: its first cell drawn over the whole range. */
  #merge([top, left, bottom, right]: Range, host: number): HTMLElement {
    const sheet = this.#sheet;
    const start = lowerBound(sheet.cellRows, top);
    const end = lowerBound(sheet.cellRows, top + 1, start);
    const at = lowerBound(sheet.cellColumns, left, start, end);
    const anchor = at < end && sheet.cellColumns[at] === left ? at : -1;
    const element = this.#take(`m${top}:${left}:${bottom}:${right}`, () => {
      const made = this.#make(anchor, left, top, bottom, right);
      made.classList.add('xlsx-merged');
      // A range whose first cell is not written shows that cell's row's or column's style.
      const blank = anchor < 0 ? this.#blanks.at(top, left) : -1;
      if (this.#blanks.paints(blank)) this.#looks.paint(made, this.#looks.style(blank));
      return made;
    });
    const zoom = this.#zoom;
    // From its row's top: the row it hangs from may be below its first.
    element.style.top = `${(this.#rows.start(top) - this.#rows.start(host)) * zoom}px`;
    element.style.left = `${this.#columns.start(left) * zoom - this.#x}px`;
    return element;
  }
}

function lineElement(className: string): HTMLElement {
  const line = document.createElement('div');
  line.className = className;
  return line;
}
