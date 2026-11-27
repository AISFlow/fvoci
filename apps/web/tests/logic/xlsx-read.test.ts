/**
 * The spreadsheet worker's reader (src/viewer-frame/xlsx-read.ts) on the
 * archive the frame hands it (`repack`): the Korean workbook's sheets,
 * values, styles, merges, frozen rows and drawings; the kinds of text a
 * cell holds; parts found through the relationships; and a workbook past
 * each of the worker's ceilings, stopped by that ceiling and no other, with
 * one at the ceiling still read. The frame takes the worker's answer only
 * in its shapes.
 */
import { describe, expect, it } from 'vitest';

import { readFromXlsxWorker } from '../../src/viewer-frame/xlsx-answer.ts';
import {
  BLANK,
  BOOLEAN,
  ERROR,
  MAX_COLUMN_PIXELS,
  NUMBER,
  STRING,
  type Workbook,
} from '../../src/viewer-frame/xlsx-protocol.ts';
import { failureOf, readWorkbook } from '../../src/viewer-frame/xlsx-read.ts';
import {
  CELL_LIMIT,
  COLUMN_RUN_LIMIT,
  FORMAT_LIMIT,
  MERGE_LIMIT,
  ROW_LIMIT,
  SHEET_LIMIT,
  STRING_LENGTH,
  STRING_LIMIT,
  STRING_TEXT_LIMIT,
  STYLE_LIMIT,
} from '../../src/viewer-frame/xlsx-xml.ts';
import { repack } from '../../src/viewer-frame/zip-guard.ts';
import {
  endlessFormulaXlsx,
  longStringsXlsx,
  lyingDimensionXlsx,
  manyCellsXlsx,
  manyColumnsXlsx,
  manyFormatsXlsx,
  manyMergesXlsx,
  manyRowsXlsx,
  manySheetsXlsx,
  manyStringsXlsx,
  manyStylesXlsx,
  pastTheEdgeXlsx,
} from '../support/hostile-xlsx.ts';
import { zip } from '../support/viewer-files.ts';
import {
  cell,
  koreanXlsx,
  ROW_COLUMN_STYLES,
  rowColumnXlsx,
  workbook,
  XLSX_TEXT,
} from '../support/viewer-xlsx.ts';

const buffer = (bytes: Buffer): ArrayBuffer => new Uint8Array(bytes).buffer;

/** Reads a workbook as the worker gets it: the archive the frame wrote of the parts it counted. */
async function read(bytes: Buffer): Promise<Workbook> {
  return readWorkbook(await repack(buffer(bytes)));
}

/** Why the worker would refuse the workbook: the ceiling, or another failure. */
async function refusal(bytes: Buffer): Promise<string> {
  try {
    await readWorkbook(buffer(bytes));
    return 'read';
  } catch (error) {
    const failure = failureOf(error);
    return failure.kind === 'failed' && failure.reason === 'too-large'
      ? `too-large: ${error instanceof Error ? error.message : ''}`
      : failure.kind === 'failed'
        ? failure.reason
        : '';
  }
}

/** The cells of a sheet, by reference: their kind and value (a string's text). */
function cellsOf(book: Workbook, index: number): Map<string, [number, number | string]> {
  const sheet = book.sheets[index];
  const found = new Map<string, [number, number | string]>();
  if (!sheet) return found;
  sheet.cellRows.forEach((row, at) => {
    const column = sheet.cellColumns[at] ?? 0;
    const kind = sheet.kinds[at] ?? BLANK;
    const value = sheet.values[at] ?? 0;
    const name = `${String.fromCharCode(65 + column)}${row + 1}`;
    found.set(name, [
      kind,
      kind === STRING || kind === ERROR ? (book.strings[value] ?? '') : value,
    ]);
  });
  return found;
}

describe('the Korean workbook', () => {
  it('its sheets shown, in order, the hidden one left out', async () => {
    const book = await read(koreanXlsx());
    expect(book.sheets.map((sheet) => sheet.name)).toEqual([...XLSX_TEXT.sheets]);
    // The table of strings is the workbook's; no cell shown is the hidden sheet's.
    const shown = book.sheets.flatMap((_, index) => [...cellsOf(book, index).values()]);
    expect(shown.map(([, value]) => value)).not.toContain('보이면 안 되는 시트');
    expect(book.sheets.map((sheet) => sheet.drawings)).toEqual([false, true]);
  });

  it('its values: shared and inline strings, numbers, a formula stored, an error, a boolean', async () => {
    const book = await read(koreanXlsx());
    const cells = cellsOf(book, 0);
    expect(cells.get('A1')).toEqual([STRING, XLSX_TEXT.title]);
    expect(cells.get('B3')).toEqual([NUMBER, 1_234_567.891]);
    expect(cells.get('B7')).toEqual([NUMBER, 7_777_900.712]);
    expect(cells.get('C7')).toEqual([ERROR, '#DIV/0!']);
    expect(cells.get('E7')).toEqual([BOOLEAN, 1]);
    expect(cells.get('A8')).toEqual([STRING, XLSX_TEXT.inline]);
    // In order of rows and columns, as the frame searches them.
    const sheet = book.sheets[0];
    const places = Array.from(
      sheet?.cellRows ?? [],
      (row, at) => row * 16_384 + (sheet?.cellColumns[at] ?? 0),
    );
    expect(places).toEqual([...places].sort((a, b) => a - b));
  });

  it('its merges, frozen rows, widths, heights and hidden column', async () => {
    const [sales, schedule] = (await read(koreanXlsx())).sheets;
    expect(Array.from(sales?.merges ?? [])).toEqual([0, 0, 0, 4]);
    expect([sales?.frozenRows, sales?.frozenColumns]).toEqual([2, 0]);
    expect(sales?.defaultRowHeight).toBe(16.5);
    // 12 characters of 맑은 고딕's 8-pixel digits, as Excel sizes them.
    expect(Array.from(sales?.columns.subarray(0, 4) ?? [])).toEqual([0, 0, 96, 0]);
    expect(Array.from(sales?.rows ?? [])).toEqual([0, 30, 0]);
    expect(Array.from(schedule?.columns.subarray(4, 8) ?? [])).toEqual([1, 1, 96, 1]);
  });

  it('its styles: formats by index, fonts, fills, borders, alignment', async () => {
    const book = await read(koreanXlsx());
    const style = (index: number) => book.styles[index];
    expect(book.formats[style(4)?.format ?? -1]).toBe('[$₩-412]#,##0');
    expect(book.formats[style(3)?.format ?? -1]).toBe(3);
    expect(book.fonts[style(1)?.font ?? -1]).toMatchObject({
      bold: true,
      size: 14,
      color: '#ffffff',
    });
    expect(book.fonts[style(8)?.font ?? -1]).toMatchObject({
      italic: true,
      underline: 1,
      color: '#c00000',
    });
    expect(style(1)?.fill).toBe('#4472c4');
    // Accent 6 of the default theme, lightened by 80% on its lightness (ECMA-376 18.3.1.15):
    // within a step of each channel of the #e2efda Excel shows.
    const fill = style(2)?.fill ?? '';
    [1, 3, 5].forEach((at, index) => {
      const channel = Number.parseInt(fill.slice(at, at + 2), 16);
      expect(Math.abs(channel - ([0xe2, 0xef, 0xda][index] ?? 0))).toBeLessThanOrEqual(1);
    });
    expect(book.borders[style(3)?.border ?? -1]?.left[0]).toBe(1);
    expect([style(1)?.horizontal, style(1)?.vertical, style(7)?.wrap]).toEqual([2, 2, true]);
  });

  it("gives the colour of a line it does not draw (a diagonal) to no side's", async () => {
    const stylesXml = `<fonts count="1"><font><sz val="11"/><name val="Arial"/></font></fonts><borders count="1"><border><left/><right/><top/><bottom style="thin"><color rgb="FF0000FF"/></bottom><diagonal style="thin"><color rgb="FFFF0000"/></diagonal></border></borders><cellXfs count="1"><xf numFmtId="0" fontId="0" borderId="0"/></cellXfs>`;
    const xml = `<sheetData><row r="1">${cell('A1', 1)}</row></sheetData>`;
    const book = await read(workbook({ sheets: [{ name: 'diagonal', xml }], stylesXml }));
    expect(book.borders[0]?.bottom).toEqual([1, '#0000ff']);
  });
});

describe('what a cell holds, and where the parts are', () => {
  const sheetOf = (
    xml: string,
    strings: string[] = [],
    extra: Partial<Parameters<typeof workbook>[0]> = {},
  ) =>
    read(
      workbook({
        sheets: [{ name: 'one', xml: `<sheetData>${xml}</sheetData>` }],
        strings,
        ...extra,
      }),
    );

  it("text of every kind: a formula's, an inline one, a stored date, escapes, no phonetic guide", async () => {
    const book = await sheetOf(
      `<row r="1">${cell('A1', '결과', 'str')}${cell('B1', '', 'inlineStr', 0, '<is><r><t>굵게 </t></r><r><t>보통</t></r></is>')}${cell('C1', '2026-11-18', 'd')}${cell('D1', 0, 's')}${cell('E1', 'a_x000D_b_x005F_x0041_', 'str')}</row>`,
      [],
      { sharedXml: '<si><t>東京</t><rPh sb="0" eb="2"><t>とうきょう</t></rPh></si>' },
    );
    const cells = cellsOf(book, 0);
    expect(cells.get('A1')).toEqual([STRING, '결과']);
    expect(cells.get('B1')).toEqual([STRING, '굵게 보통']);
    expect(cells.get('C1')).toEqual([NUMBER, 46_344]);
    expect(cells.get('D1')).toEqual([STRING, '東京']);
    expect(cells.get('E1')).toEqual([STRING, 'a\rb_x0041_']);
  });

  it('rows and columns of a style of their own: a row only with customFormat, in order', async () => {
    const book = await read(rowColumnXlsx());
    const [sheet] = book.sheets;
    // Rows 2, 4, 7 and 12 (from 0: 1, 3, 6, 11); row 6 has no customFormat.
    expect([...(sheet?.rowStyles ?? [])]).toEqual([1, 1, 3, 3, 6, 1, 11, 4]);
    expect([...(sheet?.columnStyles ?? [])]).toEqual([2, 2, 2]);
    // E7, written empty in a styled row, stays: its own style (none) is what it shows.
    expect(cellsOf(book, 0).get('E7')).toEqual([BLANK, 0]);
    const other = await sheetOf(
      '<row r="3" s="99" customFormat="1"/><row r="1" s="1" customFormat="true"/><row r="3" s="1" customFormat="1"/><row r="2"><c r="A2"/></row>',
      [],
      { stylesXml: ROW_COLUMN_STYLES },
    );
    // A style not in the workbook is its first; a row written twice is its last; an empty cell
    // in a sheet without styled rows or columns is no cell.
    expect([...(other.sheets[0]?.rowStyles ?? [])]).toEqual([0, 1, 2, 1]);
    expect(cellsOf(other, 0).size).toBe(0);
  });

  it('a cell named twice is the last; a string out of the table and an empty value are no value', async () => {
    const book = await sheetOf(
      `<row r="1"><c r="B1"><v>1</v></c><c r="A1"><v>2</v></c><c r="B1"><v>3</v></c><c r="C1" t="s"><v>9</v></c><c r="D1"><v></v></c></row>`,
    );
    expect([...cellsOf(book, 0)]).toEqual([
      ['A1', [NUMBER, 2]],
      ['B1', [NUMBER, 3]],
    ]);
  });

  it('the parts are where the relationships say, whatever their names', async () => {
    const xml = (text: string) =>
      Buffer.from(`<?xml version="1.0" encoding="UTF-8"?>${text}`, 'utf8');
    const rels = 'http://schemas.openxmlformats.org/package/2006/relationships';
    const type = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
    const main = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
    const bytes = zip([
      {
        name: '_rels/.rels',
        data: xml(
          `<Relationships xmlns="${rels}"><Relationship Id="a" Type="${type}/officeDocument" Target="/book/Main.xml"/></Relationships>`,
        ),
      },
      {
        name: 'book/Main.xml',
        data: xml(
          `<x:workbook xmlns:x="${main}" xmlns:r="${type}"><x:sheets><x:sheet name="첫" sheetId="1" r:id="s1"/></x:sheets></x:workbook>`,
        ),
      },
      {
        name: 'book/_rels/Main.xml.rels',
        data: xml(
          `<Relationships xmlns="${rels}"><Relationship Id="s1" Type="${type}/worksheet" Target="../parts/one.xml"/></Relationships>`,
        ),
      },
      {
        name: 'PARTS/One.xml',
        data: xml(
          `<x:worksheet xmlns:x="${main}"><x:sheetData><x:row r="1"><x:c r="A1"><x:v>5</x:v></x:c></x:row></x:sheetData></x:worksheet>`,
        ),
      },
    ]);
    const book = await read(bytes);
    expect(book.sheets.map((sheet) => sheet.name)).toEqual(['첫']);
    expect(cellsOf(book, 0).get('A1')).toEqual([NUMBER, 5]);
  });

  it('a part that more sheets name is read once, as the first of them', async () => {
    const xml = `<sheetData><row r="1">${cell('A1', 1)}</row></sheetData>`;
    const book = await read(
      workbook({
        sheets: [{ name: 'one', xml }],
        sheetsXml: `<sheet name="one" sheetId="1" r:id="rIdSheet1"/><sheet name="again" sheetId="2" r:id="rIdSheet1"/>`,
      }),
    );
    expect(book.sheets.map((sheet) => sheet.name)).toEqual(['one']);
  });

  it('a lying dimension is not believed', async () => {
    const [sheet] = (await read(lyingDimensionXlsx())).sheets;
    expect(sheet?.cellRows).toHaveLength(1);
  });

  it.each([
    ['a cell past the last column', pastTheEdgeXlsx('XFE1')],
    ['a cell past the last row', pastTheEdgeXlsx('A1048577')],
    ['a reference of no cell', pastTheEdgeXlsx('A99999999999')],
    ['a row past the last', pastTheEdgeXlsx('A1', 1_048_577)],
  ])('%s makes the file unreadable', async (_, bytes) => {
    expect(await refusal(bytes)).toBe('unreadable');
  });

  it("a formula without end is not kept, and its text is cut at a cell's longest", async () => {
    const book = await read(endlessFormulaXlsx(2_000_000));
    expect(book.strings[0]).toHaveLength(STRING_LENGTH);
  });
});

describe('a workbook past a ceiling is too large, and stopped by that ceiling alone', () => {
  // Each at its ceiling, then one past it; every other ceiling far off.
  const cases: [string, (count: number) => Buffer, number][] = [
    ['cells', (count) => manyCellsXlsx(count), CELL_LIMIT],
    ['strings', (count) => manyStringsXlsx(count), STRING_LIMIT],
    [
      'string text',
      (count) => longStringsXlsx(count, STRING_LENGTH),
      Math.floor(STRING_TEXT_LIMIT / STRING_LENGTH),
    ],
    // The one font, two fills and one border count as styles too.
    ['styles', (count) => manyStylesXlsx(count - 4), STYLE_LIMIT],
    ['number formats', (count) => manyFormatsXlsx(count), FORMAT_LIMIT],
    ['merged cells', (count) => manyMergesXlsx(count), MERGE_LIMIT],
    ['sheets', (count) => manySheetsXlsx(count), SHEET_LIMIT],
    ['rows', (count) => manyRowsXlsx(count), ROW_LIMIT],
    ['columns', (count) => manyColumnsXlsx(count), COLUMN_RUN_LIMIT],
  ];

  it.each(cases)(
    '%s',
    async (name, make, most) => {
      expect(await refusal(make(most))).toBe('read');
      expect(await refusal(make(most + 1))).toBe(`too-large: ${name}`);
    },
    60_000,
  );
});

describe("the worker's answer", () => {
  it('is taken in its shapes', async () => {
    const book = await read(koreanXlsx());
    expect(readFromXlsxWorker({ kind: 'read', workbook: book })).toEqual({
      kind: 'read',
      workbook: book,
    });
    expect(readFromXlsxWorker({ kind: 'failed', reason: 'too-large' })).toEqual({
      kind: 'failed',
      reason: 'too-large',
    });
  });

  it.each([
    ['a reason of another kind', () => ({ kind: 'failed', reason: 'locked' })],
    [
      'a string past the table',
      (book: Workbook) => {
        book.sheets[0]?.values.set([book.strings.length + 5], 0);
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'cells out of order',
      (book: Workbook) => {
        book.sheets[0]?.cellRows.reverse();
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'a colour that is no colour',
      (book: Workbook) => {
        if (book.fonts[0]) book.fonts[0].color = 'red;background:url(x)';
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'a font family that is CSS',
      (book: Workbook) => {
        if (book.fonts[0]) book.fonts[0].family = 'x", serif; color: red';
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'a style of a font that is not there',
      (book: Workbook) => {
        if (book.styles[0]) book.styles[0].font = book.fonts.length;
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'a row of a style not there',
      (book: Workbook) => {
        if (book.sheets[0]) book.sheets[0].rowStyles = Uint32Array.of(1, book.styles.length);
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'rows of a style out of order',
      (book: Workbook) => {
        if (book.sheets[0]) book.sheets[0].rowStyles = Uint32Array.of(5, 0, 2, 0);
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'a run of columns of a style backwards',
      (book: Workbook) => {
        if (book.sheets[0]) book.sheets[0].columnStyles = Uint32Array.of(3, 2, 0);
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'styles of rows in an array of another kind',
      (book: Workbook) => {
        if (book.sheets[0]) (book.sheets[0] as { rowStyles: unknown }).rowStyles = [1, 0];
        return { kind: 'read', workbook: book };
      },
    ],
    [
      'arrays of another kind',
      (book: Workbook) => {
        if (book.sheets[0]) (book.sheets[0] as { kinds: unknown }).kinds = [0, 1];
        return { kind: 'read', workbook: book };
      },
    ],
  ])('not with %s', async (_, spoil) => {
    expect(readFromXlsxWorker(spoil(await read(koreanXlsx())))).toBeUndefined();
  });

  it('takes whatever a workbook Excel writes reads to: the widest column, a large font, a style naming what is not there', async () => {
    // A default font of 72 points: a column of Excel's widest, 255 characters, is far past a screen.
    const stylesXml = `<fonts count="1"><font><sz val="72"/><name val="Arial"/></font></fonts><borders count="1"><border/></borders><cellXfs count="2"><xf numFmtId="0" fontId="0" borderId="0"/><xf numFmtId="0" fontId="3" borderId="2"/></cellXfs>`;
    const xml = `<sheetFormatPr baseColWidth="255" defaultRowHeight="15"/><cols><col min="1" max="1" width="255" customWidth="1"/></cols><sheetData><row r="1">${cell('A1', 1, undefined, 1)}</row></sheetData>`;
    const book = await read(workbook({ sheets: [{ name: 'wide', xml }], stylesXml }));
    expect(readFromXlsxWorker({ kind: 'read', workbook: book })).toEqual({
      kind: 'read',
      workbook: book,
    });
    expect(book.sheets[0]?.columns[2]).toBe(MAX_COLUMN_PIXELS);
    expect(book.sheets[0]?.defaultColumnWidth).toBe(MAX_COLUMN_PIXELS);
    // The style that names a font and a border not there gets the first of each.
    expect([book.styles[1]?.font, book.styles[1]?.border]).toEqual([0, 0]);
  });
});
