/**
 * A sheet drawn in the frame (src/viewer-frame/xlsx-grid.ts, xlsx-pane.ts,
 * xlsx-axis.ts): what the workbook says is taken as text (values, the
 * literals of a format code and the sheet's name that look like markup, or
 * like markup escaped, are drawn as they are and never read as markup); on
 * a sheet too dense to draw all of in view, the frozen headings are drawn
 * first and every cell looked at spends the budget, a merge's too; hidden
 * rows hidden one by one are one run to step over.
 */
import { Blob as NodeBlob } from 'node:buffer';

import { afterEach, describe, expect, it, vi } from 'vitest';

import { Axis } from '../../src/viewer-frame/xlsx-axis.ts';
import { SheetView } from '../../src/viewer-frame/xlsx-grid.ts';
import { Looks } from '../../src/viewer-frame/xlsx-look.ts';
import { CELL_BUDGET } from '../../src/viewer-frame/xlsx-pane.ts';
import { readWorkbook } from '../../src/viewer-frame/xlsx-read.ts';
import { repack } from '../../src/viewer-frame/zip-guard.ts';
import { cell, esc, ROW_COLUMN_STYLES, rowColumnXlsx, workbook } from '../support/viewer-xlsx.ts';

const VALUES = ['<img src="x" onerror="globalThis.drawn = true">', '<b>굵게</b>', '&lt;i&gt;'];
const NAME = '<script>표</script>';
const CODE = '"<u>"0"</u>"';
const INLINE = '<svg onload="globalThis.drawn = true"></svg>';
/** Columns and rows a pixel each: a 1000 by 600 view holds every cell of the sheets below. */
const TINY = `<sheetFormatPr defaultRowHeight="0.75"/><cols><col min="1" max="16384" width="0.1" customWidth="1"/></cols>`;

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.replaceChildren();
  Reflect.deleteProperty(document.documentElement, 'clientWidth');
  Reflect.deleteProperty(document.documentElement, 'clientHeight');
});

/**
 * A budget a tenth of the frame's: what a drawing does once its budget is
 * spent is the same at any budget, and jsdom takes seconds to make the 8,000
 * cells of the frame's.
 */
const SMALL_BUDGET = CELL_BUDGET / 10;

/**
 * The first sheet of the workbook, read as the worker reads it and drawn in a view of 1000 by
 * 600, looking at `cells` cells (the frame's budget, left out).
 */
async function drawn(bytes: Buffer, cells?: number): Promise<SheetView> {
  // jsdom's Blob cannot stream, the worker's can.
  vi.stubGlobal('Blob', NodeBlob);
  const book = await readWorkbook(await repack(new Uint8Array(bytes).buffer));
  vi.unstubAllGlobals();
  const [sheet] = book.sheets;
  if (!sheet) throw new Error('no sheet');
  // The frame's size: jsdom lays nothing out.
  Object.defineProperty(document.documentElement, 'clientWidth', {
    value: 1000,
    configurable: true,
  });
  Object.defineProperty(document.documentElement, 'clientHeight', {
    value: 600,
    configurable: true,
  });
  const view = new SheetView(sheet, new Looks(book, 'ko'), cells);
  document.body.append(view.element);
  view.draw();
  return view;
}

/** Rows of `columns` cells holding numbers, from row `from`. */
function rowsOf(count: number, columns: number, from = 1): string {
  let xml = '';
  for (let row = from; row < from + count; row += 1) {
    let cells = '';
    for (let column = 0; column < columns; column += 1) {
      cells += `<c r="${letters(column)}${row}"><v>${row}</v></c>`;
    }
    xml += `<row r="${row}">${cells}</row>`;
  }
  return xml;
}

function letters(index: number): string {
  let name = '';
  for (let rest = index + 1; rest > 0; rest = Math.floor((rest - 1) / 26)) {
    name = String.fromCharCode(65 + ((rest - 1) % 26)) + name;
  }
  return name;
}

describe('what a sheet says', () => {
  it('draws values, format literals and the sheet name that look like markup as text', async () => {
    const cells = [
      cell('A1', 0, 's'),
      cell('B1', 1, 's'),
      cell('C1', 2, 's'),
      cell('D1', 5, undefined, 1),
      cell('E1', '', 'inlineStr', 0, `<is><t>${esc(INLINE)}</t></is>`),
    ].join('');
    const view = await drawn(
      workbook({
        sheets: [{ name: NAME, xml: `<sheetData><row r="1">${cells}</row></sheetData>` }],
        strings: VALUES,
        stylesXml: `<numFmts count="1"><numFmt numFmtId="164" formatCode="${esc(CODE)}"/></numFmts><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164" applyNumberFormat="1"/></cellXfs>`,
      }),
    );
    const texts = [...view.element.querySelectorAll('[role="cell"]')].map(
      (each) => each.textContent,
    );
    expect(texts).toEqual([...VALUES, '<u>5</u>', INLINE]);
    expect(view.element.querySelector('img, b, i, u, svg, script')).toBeNull();
    expect(view.element.querySelector('[role="table"]')?.getAttribute('aria-label')).toBe(NAME);
    expect(Reflect.get(globalThis, 'drawn')).toBeUndefined();
  });
});

describe('a sheet too dense to draw all of', () => {
  it('draws its frozen headings first, and the rest with what the budget leaves', async () => {
    // 11 rows of 100 cells in view, the first frozen: more cells than the budget.
    const pane = `<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" state="frozen"/></sheetView></sheetViews>`;
    const view = await drawn(
      workbook({
        sheets: [{ name: 'dense', xml: `${pane}${TINY}<sheetData>${rowsOf(11, 100)}</sheetData>` }],
      }),
      SMALL_BUDGET,
    );
    const heading = view.element.querySelectorAll('[role="row"][aria-rowindex="1"] [role="cell"]');
    expect(heading).toHaveLength(100);
    const all = view.element.querySelectorAll('[role="cell"]');
    expect(all.length).toBeLessThanOrEqual(SMALL_BUDGET);
    expect(all.length).toBeGreaterThan(SMALL_BUDGET - 100);
  });

  it('spends the budget on the cells under a merge too, which are looked at though not drawn', async () => {
    // 100 rows of 100 cells in view, all but the last of each under one merge: the frame's own
    // budget is spent on cells that are looked at and make no element.
    const xml = `${TINY}<sheetData>${rowsOf(100, 100)}</sheetData><mergeCells count="1"><mergeCell ref="A1:CU100"/></mergeCells>`;
    const view = await drawn(workbook({ sheets: [{ name: 'merged', xml }] }));
    const rows = view.element.querySelectorAll('[role="row"]').length;
    // The merge, and the cells right of it.
    const plain = view.element.querySelectorAll('[role="cell"]').length - 1;
    // Each row walked looks at its 99 cells under the merge and draws the one after them.
    expect(plain + 99 * rows).toBeLessThanOrEqual(CELL_BUDGET + 99);
    // The budget is the frame's: all of it is spent but less than two rows' worth.
    expect(plain + 99 * rows).toBeGreaterThan(CELL_BUDGET - 200);
    expect(plain).toBeLessThan(100);
    expect(plain).toBeGreaterThan(0);
  });
});

describe('the rows along a sheet', () => {
  it('are one run where rows next to each other hide one by one, and step over it at once', () => {
    const hidden = Array.from(
      { length: 100_000 },
      (_, index) => [index + 1, index + 1, 0] as const,
    );
    const axis = new Axis(200_000, 20, hidden);
    expect(axis.next(0)).toBe(100_001);
    expect(axis.size(50_000)).toBe(0);
    expect(axis.start(100_001)).toBe(20);
    expect(axis.indexAt(20)).toBe(100_001);
    expect(axis.total).toBe(100_000 * 20);
  });
});

describe('rows and columns of a style of their own', () => {
  /** The empty cells drawn in row `row` (from 1) for its row's or column's style. */
  const blanks = (view: SheetView, row: number): HTMLElement[] => [
    ...view.element.querySelectorAll<HTMLElement>(
      `[role="row"][aria-rowindex="${row}"] .xlsx-cell[aria-hidden="true"]`,
    ),
  ];
  const fills = (cells: HTMLElement[]): string[] => [
    ...new Set(cells.map((each) => each.style.backgroundColor)),
  ];
  const RED = 'rgb(255, 0, 0)';
  const BLUE = 'rgb(0, 0, 255)';

  it('show on the cells not written: a cell over its row, a row over its column', async () => {
    const view = await drawn(rowColumnXlsx());
    // Row 2 is red across, column C too; B2, written, is of its own style (none): two runs of
    // red, one each side of it.
    const second = blanks(view, 2);
    expect(second).toHaveLength(2);
    expect(fills(second)).toEqual([RED]);
    expect(second.map((each) => each.getAttribute('aria-colspan'))).toEqual([null, null]);
    const written = view.element.querySelectorAll<HTMLElement>(
      '[role="row"][aria-rowindex="2"] [role="cell"]',
    );
    expect([...written].map((each) => [each.textContent, each.style.backgroundColor])).toEqual([
      ['7', ''],
    ]);
    // A row of no style of its own: only column C, blue; a row of nothing to read.
    expect(fills(blanks(view, 3))).toEqual([BLUE]);
    expect(blanks(view, 3)).toHaveLength(1);
    const rowOf = (row: number) => view.element.querySelector(`[aria-rowindex="${row}"]`);
    expect(rowOf(3)?.getAttribute('aria-hidden')).toBe('true');
    expect(rowOf(2)?.hasAttribute('aria-hidden')).toBe(false);
    // Row 4 is bold alone: it stands over column C, and paints nothing.
    expect(blanks(view, 4)).toHaveLength(0);
    // Row 6 names a style without customFormat, which Excel does not take.
    expect(fills(blanks(view, 6))).toEqual([BLUE]);
    // Row 7 is red but at E7, written empty of no style, which is its own.
    expect(blanks(view, 7)).toHaveLength(second.length);
    expect(view.element.querySelector('[role="row"][aria-rowindex="7"] [role="cell"]')).not.toBe(
      null,
    );
    // A merge of no cell written takes its first cell's column's style.
    const merged = view.element.querySelector<HTMLElement>('.xlsx-merged');
    expect(merged?.style.backgroundColor).toBe(BLUE);
    // Row 12 is bordered: each of its empty cells is drawn alone, with the edges.
    const bordered = blanks(view, 12);
    expect(bordered.length).toBeGreaterThan(5);
    expect(bordered.every((each) => each.querySelector('.xlsx-edges'))).toBe(true);
    // Nothing of them is read out: they hold nothing.
    expect(second.every((each) => !each.hasAttribute('role'))).toBe(true);
  });

  it('a styled column across the whole sheet spends the budget like any cell', async () => {
    // Every column bordered (each cell drawn alone), 1-pixel columns and rows: far more empty
    // cells in view than the budget.
    const xml = `<sheetFormatPr defaultRowHeight="0.75"/><cols><col min="1" max="16384" width="0.1" style="4" customWidth="1"/></cols><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>`;
    const view = await drawn(
      workbook({ sheets: [{ name: 'bordered', xml }], stylesXml: ROW_COLUMN_STYLES }),
      SMALL_BUDGET,
    );
    const drawnBlanks = view.element.querySelectorAll('.xlsx-cell[aria-hidden="true"]').length;
    expect(drawnBlanks).toBeLessThanOrEqual(SMALL_BUDGET);
    expect(drawnBlanks).toBeGreaterThan(SMALL_BUDGET / 2);
  });

  it('the written cells are drawn before any empty one: a styled sheet loses no value', async () => {
    // As dense as the sheet above that fills the budget with values, every column bordered.
    const xml = `<sheetFormatPr defaultRowHeight="0.75"/><cols><col min="1" max="16384" width="0.1" style="4" customWidth="1"/></cols><sheetData>${rowsOf(7, 100)}</sheetData>`;
    const view = await drawn(
      workbook({ sheets: [{ name: 'dense', xml }], stylesXml: ROW_COLUMN_STYLES }),
      SMALL_BUDGET,
    );
    // Every value in view, though the empty cells around them are more than the budget.
    expect(view.element.querySelectorAll('[role="cell"]')).toHaveLength(7 * 100);
    const empty = view.element.querySelectorAll('.xlsx-cell[aria-hidden="true"]').length;
    expect(empty).toBe(SMALL_BUDGET - 7 * 100);
  });
});
