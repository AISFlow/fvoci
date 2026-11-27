/**
 * Small workbooks that would make the spreadsheet worker hold too much: one
 * for each of its ceilings (`src/viewer-frame/xlsx-xml.ts`), each past that
 * ceiling by one and within all the others, and those whose parts say what
 * they are not (a `dimension` of the whole sheet, a cell past its edge, a
 * formula without end). Each is a few kilobytes packed.
 */
import { PLAIN_STYLES, workbook } from './viewer-xlsx.ts';

const repeat = (text: string, times: number): string => text.repeat(times);

/** A sheet of `count` cells, `perRow` to a row, each `cell` (by default empty). */
export function manyCellsXlsx(count: number, perRow = 10_000, cell = '<c/>'): Buffer {
  const rows: string[] = [];
  for (let left = count, row = 1; left > 0; left -= perRow, row += 1) {
    rows.push(`<row r="${row}">${repeat(cell, Math.min(perRow, left))}</row>`);
  }
  return workbook({ sheets: [{ name: 'cells', xml: `<sheetData>${rows.join('')}</sheetData>` }] });
}

/** A workbook of `count` shared strings, each empty. */
export function manyStringsXlsx(count: number): Buffer {
  return workbook({
    sheets: [{ name: 'strings', xml: '<sheetData/>' }],
    sharedXml: repeat('<si/>', count),
  });
}

/** A workbook of `items` shared strings of `length` characters each. */
export function longStringsXlsx(items: number, length: number): Buffer {
  const item = `<si><t>${'a'.repeat(length)}</t></si>`;
  return workbook({
    sheets: [{ name: 'text', xml: '<sheetData/>' }],
    sharedXml: repeat(item, items),
  });
}

/** A styles part of `count` cell styles, past the one font, two fills and one border. */
export function manyStylesXlsx(count: number): Buffer {
  const styles = PLAIN_STYLES.replace(
    /<cellXfs.*$/,
    `<cellXfs>${repeat('<xf/>', count)}</cellXfs>`,
  );
  return workbook({ sheets: [{ name: 'styles', xml: '<sheetData/>' }], stylesXml: styles });
}

/** A styles part of `count` number formats. */
export function manyFormatsXlsx(count: number): Buffer {
  const formats = Array.from(
    { length: count },
    (_, index) => `<numFmt numFmtId="${164 + (index % 1000)}" formatCode="0"/>`,
  );
  return workbook({
    sheets: [{ name: 'formats', xml: '<sheetData/>' }],
    stylesXml: `<numFmts>${formats.join('')}</numFmts>${PLAIN_STYLES}`,
  });
}

/** A sheet of `count` merges, all of the same two cells. */
export function manyMergesXlsx(count: number): Buffer {
  const xml = `<sheetData/><mergeCells>${repeat('<mergeCell ref="A1:B2"/>', count)}</mergeCells>`;
  return workbook({ sheets: [{ name: 'merges', xml }] });
}

/** A workbook that lists `count` sheets, all the one worksheet part. */
export function manySheetsXlsx(count: number): Buffer {
  const sheetsXml = Array.from(
    { length: count },
    (_, index) => `<sheet name="s${index}" sheetId="${index + 1}" r:id="rIdSheet1"/>`,
  ).join('');
  return workbook({ sheets: [{ name: 's', xml: '<sheetData/>' }], sheetsXml });
}

/** A sheet that sets the height of row 1, `count` times over. */
export function manyRowsXlsx(count: number): Buffer {
  return workbook({
    sheets: [
      {
        name: 'rows',
        xml: `<sheetData>${repeat('<row r="1" ht="20" customHeight="1"/>', count)}</sheetData>`,
      },
    ],
  });
}

/** A sheet of `count` runs of columns, all of column A. */
export function manyColumnsXlsx(count: number): Buffer {
  return workbook({
    sheets: [
      {
        name: 'columns',
        xml: `<cols>${repeat('<col min="1" max="1" width="9"/>', count)}</cols><sheetData/>`,
      },
    ],
  });
}

/** A sheet whose `dimension` says it is the whole sheet, and has one cell. */
export function lyingDimensionXlsx(): Buffer {
  const xml =
    '<dimension ref="A1:XFD1048576"/><sheetData><row r="1" spans="1:16384"><c r="A1"><v>1</v></c></row></sheetData>';
  return workbook({ sheets: [{ name: 'dimension', xml }] });
}

/** A sheet with a cell at this reference, and a row at this number. */
export function pastTheEdgeXlsx(reference: string, row = 1): Buffer {
  const xml = `<sheetData><row r="${row}"><c r="${reference}"><v>1</v></c></row></sheetData>`;
  return workbook({ sheets: [{ name: 'edge', xml }] });
}

/** A cell of a formula of `length` characters, whose stored value is a text of as many. */
export function endlessFormulaXlsx(length: number): Buffer {
  const xml = `<sheetData><row r="1"><c r="A1" t="str"><f>${'1+'.repeat(length / 2)}1</f><v>${'가'.repeat(length)}</v></c></row></sheetData>`;
  return workbook({ sheets: [{ name: 'formula', xml }] });
}
