/**
 * A sheet drawn in the frame. The frame's page scrolls, as it does for the
 * other kinds: over a block as large as the sheet (`element`), in which a
 * layer the size of the view stays put (`position: sticky`) and holds what
 * is in view: the column letters and row numbers, and four panes
 * (`xlsx-pane.ts`) — the frozen rows and columns, their corner, and the
 * rest, which follows the scroll.
 *
 * A sheet goes on some way past its last cell, as it does in a spreadsheet,
 * and no further: a million empty rows are not drawn because a file says a
 * cell is at the bottom. A page taller or wider than `SCROLL_LIMIT` is made
 * that long and scrolls through the sheet at a scale: browsers stop laying
 * out somewhere past some millions of pixels, WebKit before the others.
 */
import { Axis } from './xlsx-axis.ts';
import { BlankStyles } from './xlsx-blank.ts';
import { PX_PER_POINT, type Looks } from './xlsx-look.ts';
import { CELL_BUDGET, Pane, type Window } from './xlsx-pane.ts';
import { MAX_COLUMNS, MAX_ROWS, type Sheet } from './xlsx-protocol.ts';
import { Ruler } from './xlsx-ruler.ts';

export const SCROLL_LIMIT = 4_000_000;
/** How far the sheet goes past its last row and column, and at least how far it goes. */
const MORE_ROWS = 50;
const MORE_COLUMNS = 10;
const LEAST_ROWS = 200;
const LEAST_COLUMNS = 52;
/** The height of the column letters, in pixels at 100%. */
const RULER_HEIGHT = 20;
/** A frozen part longer than this share of the view is not frozen: on a narrow screen it would leave no room for the rest. */
const FROZEN_SHARE = 0.5;

/** The last row and column the sheet uses: of its cells, its merges, its rows of their own and its frozen panes. */
export function extent(sheet: Sheet): [number, number] {
  // The cells are in order of rows: the last is in the last row.
  let rows = Math.max(sheet.cellRows.at(-1) ?? 0, sheet.frozenRows);
  let columns = sheet.frozenColumns;
  for (const column of sheet.cellColumns) if (column > columns) columns = column;
  for (let at = 0; at + 3 < sheet.merges.length; at += 4) {
    rows = Math.max(rows, sheet.merges[at + 2] ?? 0);
    columns = Math.max(columns, sheet.merges[at + 3] ?? 0);
  }
  for (let at = 0; at + 2 < sheet.rows.length; at += 3) rows = Math.max(rows, sheet.rows[at] ?? 0);
  // A styled row is in use, as Excel takes it; a styled column (often all of them) is not.
  rows = Math.max(rows, sheet.rowStyles.at(-2) ?? 0);
  return [rows, columns];
}

/** The sheet's rows and columns as lengths, in pixels at 100%. */
export function axesOf(sheet: Sheet): { rows: Axis; columns: Axis } {
  const [lastRow, lastColumn] = extent(sheet);
  const rowHeight = Math.round(sheet.defaultRowHeight * PX_PER_POINT);
  const rowRuns: [number, number, number][] = [];
  for (let at = 0; at + 2 < sheet.rows.length; at += 3) {
    const [row = 0, height = -1, hidden = 0] = sheet.rows.subarray(at, at + 3);
    const size = hidden === 1 ? 0 : height < 0 ? rowHeight : Math.round(height * PX_PER_POINT);
    rowRuns.push([row, row, size]);
  }
  const columnRuns: [number, number, number][] = [];
  for (let at = 0; at + 3 < sheet.columns.length; at += 4) {
    const [first = 0, last = 0, width = -1, hidden = 0] = sheet.columns.subarray(at, at + 4);
    columnRuns.push([first, last, hidden === 1 ? 0 : width < 0 ? sheet.defaultColumnWidth : width]);
  }
  return {
    rows: new Axis(
      Math.min(MAX_ROWS, Math.max(LEAST_ROWS, lastRow + 1 + MORE_ROWS)),
      rowHeight,
      rowRuns,
    ),
    columns: new Axis(
      Math.min(MAX_COLUMNS, Math.max(LEAST_COLUMNS, lastColumn + 1 + MORE_COLUMNS)),
      sheet.defaultColumnWidth,
      columnRuns,
    ),
  };
}

/** Where the scrolled part of the sheet starts, from where the page is scrolled to: the same, or at a scale. */
const offset = (scroll: number, pageRoom: number, sheetRoom: number): number =>
  pageRoom <= 0 || sheetRoom <= 0
    ? 0
    : (Math.min(Math.max(0, scroll), pageRoom) * sheetRoom) / pageRoom;

/** A band of the view, down (rows) or across (columns): half of a pane's window. */
type Band<Along extends 'down' | 'across'> = Along extends 'down'
  ? Pick<Window, 'top' | 'height' | 'rows' | 'y'>
  : Pick<Window, 'left' | 'width' | 'columns' | 'x'>;

/** How the view is laid out at one zoom and size of the frame, in pixels. */
interface Layout {
  width: number;
  height: number;
  rulerWidth: number;
  rulerHeight: number;
  frozenRows: number;
  frozenColumns: number;
  frozenWidth: number;
  frozenHeight: number;
  /** How far the scrolled part goes past what the view shows of it, along each axis. */
  roomDown: number;
  roomAcross: number;
  /** How far the page scrolls, along each axis. */
  pageDown: number;
  pageAcross: number;
}

export class SheetView {
  /** The block as large as the sheet, which the page scrolls over. */
  readonly element = document.createElement('div');
  readonly #layer = document.createElement('div');
  readonly #corner = document.createElement('div');
  readonly #dividers = [document.createElement('div'), document.createElement('div')] as const;
  readonly #sheet: Sheet;
  readonly #rows: Axis;
  readonly #columns: Axis;
  /** The corner, the frozen rows, the frozen columns, the rest: in the order they are read. */
  readonly #panes: [Pane, Pane, Pane, Pane];
  /** The letters of the frozen columns and the rest's, the numbers of the frozen rows and the rest's. */
  readonly #rulers: [Ruler, Ruler, Ruler, Ruler];
  /** How many cells one drawing looks at. */
  readonly #cells: number;
  #zoom = 1;

  /** `cells` is the budget of one drawing: the frame leaves it at `CELL_BUDGET`. */
  constructor(sheet: Sheet, looks: Looks, cells = CELL_BUDGET) {
    this.#sheet = sheet;
    this.#cells = cells;
    const { rows, columns } = axesOf(sheet);
    this.#rows = rows;
    this.#columns = columns;
    const blanks = new BlankStyles(
      sheet,
      (style) => looks.paints(style),
      (style) => !looks.bordered(style),
    );
    const pane = (): Pane => new Pane(sheet, looks, rows, columns, blanks);
    this.#panes = [pane(), pane(), pane(), pane()];
    this.#rulers = [
      new Ruler(columns, true),
      new Ruler(columns, true),
      new Ruler(rows, false),
      new Ruler(rows, false),
    ];
    this.element.className = 'xlsx-sheet';
    this.#layer.className = 'xlsx-layer';
    this.#layer.setAttribute('role', 'table');
    this.#layer.setAttribute('aria-label', sheet.name);
    this.#layer.setAttribute('aria-rowcount', String(rows.count));
    this.#layer.setAttribute('aria-colcount', String(columns.count));
    this.#corner.className = 'xlsx-ruler xlsx-ruler-corner';
    this.#corner.setAttribute('aria-hidden', 'true');
    for (const divider of this.#dividers) divider.className = 'xlsx-divider';
    this.#layer.append(
      ...this.#panes.map((each) => each.element),
      ...this.#dividers,
      ...this.#rulers.map((each) => each.element),
      this.#corner,
    );
    this.element.append(this.#layer);
  }

  #layout(): Layout {
    const zoom = this.#zoom;
    const rows = this.#rows;
    const columns = this.#columns;
    const width = document.documentElement.clientWidth;
    const height = document.documentElement.clientHeight;
    const rulerHeight = Math.round(RULER_HEIGHT * zoom);
    const rulerWidth = Math.round((String(rows.count).length * 7 + 12) * zoom);
    let frozenRows = Math.min(this.#sheet.frozenRows, rows.count);
    let frozenHeight = rows.start(frozenRows) * zoom;
    if (frozenHeight > (height - rulerHeight) * FROZEN_SHARE) [frozenRows, frozenHeight] = [0, 0];
    let frozenColumns = Math.min(this.#sheet.frozenColumns, columns.count);
    let frozenWidth = columns.start(frozenColumns) * zoom;
    if (frozenWidth > (width - rulerWidth) * FROZEN_SHARE) [frozenColumns, frozenWidth] = [0, 0];
    const restDown = (rows.total - rows.start(frozenRows)) * zoom;
    const restAcross = (columns.total - columns.start(frozenColumns)) * zoom;
    const pageHeight = Math.min(SCROLL_LIMIT, rulerHeight + frozenHeight + restDown);
    const pageWidth = Math.min(SCROLL_LIMIT, rulerWidth + frozenWidth + restAcross);
    return {
      width,
      height,
      rulerWidth,
      rulerHeight,
      frozenRows,
      frozenColumns,
      frozenWidth,
      frozenHeight,
      roomDown: Math.max(0, rulerHeight + frozenHeight + restDown - height),
      roomAcross: Math.max(0, rulerWidth + frozenWidth + restAcross - width),
      pageDown: pageHeight - height,
      pageAcross: pageWidth - width,
    };
  }

  /** Lays the view out for the frame's size and draws what is in it. */
  draw(): void {
    const zoom = this.#zoom;
    const layout = this.#layout();
    const { width, height, rulerWidth, rulerHeight } = layout;
    this.element.style.width = `${layout.pageAcross + width}px`;
    this.element.style.height = `${layout.pageDown + height}px`;
    this.#layer.style.width = `${width}px`;
    this.#layer.style.height = `${height}px`;
    this.#layer.style.setProperty('--xlsx-ruler-font', `${(11 * zoom).toFixed(2)}px`);
    const [frozenDown, down] = this.#bandsDown(layout);
    const [frozenAcross, across] = this.#bandsAcross(layout);
    const budget = { cells: this.#cells };
    // The frozen parts first: each is at most half the view, and they are the headings that say
    // what the rest is; on a sheet too dense for the budget, the rest's far side goes blank first.
    const [corner, frozenRows, frozenColumns, rest] = this.#panes;
    corner.draw({ ...frozenDown, ...frozenAcross }, zoom, budget);
    frozenRows.draw({ ...frozenDown, ...across }, zoom, budget);
    frozenColumns.draw({ ...down, ...frozenAcross }, zoom, budget);
    rest.draw({ ...down, ...across }, zoom, budget);
    const [frozenLetters, letters, frozenNumbers, numbers] = this.#rulers;
    for (const [ruler, band] of [
      [frozenLetters, frozenAcross],
      [letters, across],
    ] as const) {
      const { left, width: wide, columns, x } = band;
      ruler.draw({ left, top: 0, width: wide, height: rulerHeight, range: columns, from: x }, zoom);
    }
    for (const [ruler, band] of [
      [frozenNumbers, frozenDown],
      [numbers, down],
    ] as const) {
      const { top, height: tall, rows, y } = band;
      ruler.draw({ left: 0, top, width: rulerWidth, height: tall, range: rows, from: y }, zoom);
    }
    this.#corner.style.width = `${rulerWidth}px`;
    this.#corner.style.height = `${rulerHeight}px`;
    const [dividerAcross, dividerDown] = this.#dividers;
    dividerAcross.style.cssText =
      frozenDown.height > 0
        ? `left:0;top:${down.top - 1}px;width:${width}px;height:1px`
        : 'display:none';
    dividerDown.style.cssText =
      frozenAcross.width > 0
        ? `top:0;left:${across.left - 1}px;height:${height}px;width:1px`
        : 'display:none';
  }

  /** The frozen rows' band of the view and the rest's: where each lies, its rows, and where along the sheet. */
  #bandsDown(layout: Layout): [Band<'down'>, Band<'down'>] {
    const top = layout.rulerHeight + layout.frozenHeight;
    const y =
      this.#rows.start(layout.frozenRows) * this.#zoom +
      offset(window.scrollY, layout.pageDown, layout.roomDown);
    return [
      { top: layout.rulerHeight, height: layout.frozenHeight, rows: [0, layout.frozenRows], y: 0 },
      { top, height: layout.height - top, rows: [layout.frozenRows, this.#rows.count], y },
    ];
  }

  /** The frozen columns' band of the view and the rest's. */
  #bandsAcross(layout: Layout): [Band<'across'>, Band<'across'>] {
    const left = layout.rulerWidth + layout.frozenWidth;
    const x =
      this.#columns.start(layout.frozenColumns) * this.#zoom +
      offset(window.scrollX, layout.pageAcross, layout.roomAcross);
    return [
      {
        left: layout.rulerWidth,
        width: layout.frozenWidth,
        columns: [0, layout.frozenColumns],
        x: 0,
      },
      { left, width: layout.width - left, columns: [layout.frozenColumns, this.#columns.count], x },
    ];
  }

  /** Draws at another zoom, keeping the cell at the top left of the scrolled part where it is. */
  zoom(zoom: number): void {
    const before = this.#layout();
    const down = offset(window.scrollY, before.pageDown, before.roomDown) / this.#zoom;
    const across = offset(window.scrollX, before.pageAcross, before.roomAcross) / this.#zoom;
    this.#zoom = zoom;
    for (const pane of this.#panes) pane.clear();
    for (const ruler of this.#rulers) ruler.clear();
    this.draw();
    const after = this.#layout();
    const back = (wanted: number, page: number, room: number): number =>
      room <= 0 ? 0 : (Math.min(wanted * zoom, room) * page) / room;
    window.scrollTo(
      back(across, after.pageAcross, after.roomAcross),
      back(down, after.pageDown, after.roomDown),
    );
    this.draw();
  }
}
