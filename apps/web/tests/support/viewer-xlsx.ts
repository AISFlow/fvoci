/**
 * Workbooks (XLSX) for the tests of the viewer, made here rather than kept
 * as binaries: a workbook in Korean of two sheets shown and one hidden
 * (merged cells, number formats, dates, frozen rows, a drawing), a large
 * sheet, and the writer the hostile workbooks of `hostile-xlsx.ts` use.
 * Every file is ours, so the tests carry no third-party content.
 */
import { zip } from './viewer-files.ts';

const MAIN = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PACKAGE_RELS = 'http://schemas.openxmlformats.org/package/2006/relationships';
const XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';
const TYPES = 'http://schemas.openxmlformats.org/package/2006/content-types';
const SHEET_TYPE = 'application/vnd.openxmlformats-officedocument.spreadsheetml';

/** Text made safe inside an element or an attribute. */
export const esc = (text: string): string =>
  text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');

export interface SheetPart {
  name: string;
  /** What the worksheet holds inside `<worksheet>`: its views, columns, data, merges. */
  xml: string;
  state?: 'hidden' | 'veryHidden';
  /** A chart sheet, rather than a worksheet. */
  chart?: boolean;
}

export interface WorkbookParts {
  sheets: SheetPart[];
  /** The shared strings, as plain text. */
  strings?: readonly string[];
  /** The shared strings part's items, as XML, when not `strings`. */
  sharedXml?: string;
  /** What the styles part holds inside `<styleSheet>`. */
  stylesXml?: string;
  date1904?: boolean;
  /** The workbook part's sheets, as XML, when not the sheets'. */
  sheetsXml?: string;
}

/** A workbook of the parts, as Excel lays them out. */
export function workbook({
  sheets,
  strings = [],
  sharedXml,
  stylesXml,
  date1904 = false,
  sheetsXml,
}: WorkbookParts): Buffer {
  const part = (name: string, xml: string) => ({ name, data: Buffer.from(XML + xml, 'utf8') });
  const sheetPath = (sheet: SheetPart, index: number): string =>
    sheet.chart ? `chartsheets/sheet${index + 1}.xml` : `worksheets/sheet${index + 1}.xml`;
  const entries = sheets
    .map((sheet, index) => {
      const state = sheet.state ? ` state="${sheet.state}"` : '';
      return `<sheet name="${esc(sheet.name)}" sheetId="${index + 1}"${state} r:id="rIdSheet${index + 1}"/>`;
    })
    .join('');
  const links = sheets
    .map((sheet, index) => {
      const type = sheet.chart ? 'chartsheet' : 'worksheet';
      return `<Relationship Id="rIdSheet${index + 1}" Type="${R}/${type}" Target="${sheetPath(sheet, index)}"/>`;
    })
    .join('');
  const shared =
    sharedXml ??
    strings.map((text) => `<si><t xml:space="preserve">${esc(text)}</t></si>`).join('');
  const overrides = sheets
    .map((sheet, index) => {
      const kind = sheet.chart ? 'chartsheet' : 'worksheet';
      return `<Override PartName="/xl/${sheetPath(sheet, index)}" ContentType="${SHEET_TYPE}.${kind}+xml"/>`;
    })
    .join('');
  return zip([
    part(
      '[Content_Types].xml',
      `<Types xmlns="${TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="${SHEET_TYPE}.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="${SHEET_TYPE}.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="${SHEET_TYPE}.sharedStrings+xml"/>${overrides}</Types>`,
    ),
    part(
      '_rels/.rels',
      `<Relationships xmlns="${PACKAGE_RELS}"><Relationship Id="rId1" Type="${R}/officeDocument" Target="xl/workbook.xml"/></Relationships>`,
    ),
    part(
      'xl/workbook.xml',
      `<workbook xmlns="${MAIN}" xmlns:r="${R}"><workbookPr${date1904 ? ' date1904="1"' : ''}/><sheets>${sheetsXml ?? entries}</sheets></workbook>`,
    ),
    part(
      'xl/_rels/workbook.xml.rels',
      `<Relationships xmlns="${PACKAGE_RELS}">${links}<Relationship Id="rIdStyles" Type="${R}/styles" Target="styles.xml"/><Relationship Id="rIdStrings" Type="${R}/sharedStrings" Target="sharedStrings.xml"/></Relationships>`,
    ),
    part('xl/styles.xml', `<styleSheet xmlns="${MAIN}">${stylesXml ?? PLAIN_STYLES}</styleSheet>`),
    part('xl/sharedStrings.xml', `<sst xmlns="${MAIN}">${shared}</sst>`),
    ...sheets.map((sheet, index) =>
      part(
        `xl/${sheetPath(sheet, index)}`,
        sheet.chart
          ? `<chartsheet xmlns="${MAIN}" xmlns:r="${R}">${sheet.xml}</chartsheet>`
          : `<worksheet xmlns="${MAIN}" xmlns:r="${R}">${sheet.xml}</worksheet>`,
      ),
    ),
  ]);
}

/** The styles of a workbook that has no others: one font, the two fills Excel requires, no border. */
export const PLAIN_STYLES =
  '<fonts count="1"><font><sz val="11"/><name val="맑은 고딕"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/></border></borders><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellXfs>';

/** A cell: a number, a shared string (`s`), or another type, in a style. */
export function cell(
  reference: string,
  value: string | number,
  type?: string,
  style = 0,
  inner?: string,
): string {
  const typed = type ? ` t="${type}"` : '';
  const styled = style ? ` s="${style}"` : '';
  return `<c r="${reference}"${typed}${styled}>${inner ?? `<v>${esc(String(value))}</v>`}</c>`;
}

/** A serial date of the 1900 system, for a day (and a time of it). */
export function serial1900(
  year: number,
  month: number,
  day: number,
  hours = 0,
  minutes = 0,
): number {
  const days = (Date.UTC(year, month - 1, day) - Date.UTC(1899, 11, 30)) / 86_400_000;
  return days + (hours * 60 + minutes) / 1440;
}

/** What the Korean workbook says, for the tests to look for. */
export const XLSX_TEXT = {
  sheets: ['매출', '일정'],
  hidden: '숨김',
  title: '2026년 분기별 매출',
  headers: ['분기', '매출', '증감률', '마감일', '비고'],
  quarters: ['1분기', '2분기', '3분기', '4분기'],
  /** The sales as drawn: in won, with thousands separated. */
  sales: ['₩1,234,568', '₩2,345,679', '₩987,654', '₩3,210,000'],
  rates: ['12.5%', '-3.0%', '0.0%', '45.7%'],
  deadlines: ['2026년 3월 31일', '2026년 6월 30일', '2026년 9월 30일', '2026년 12월 31일'],
  note: '회의에서 정한 대로 다음 분기 목표를 다시 잡기로 했습니다. 긴 메모는 칸 안에서 줄을 바꿉니다.',
  total: '7,777,901',
  inline: '인라인 문자열',
  colored: '빨간 기울임 밑줄',
  schedule: '분기 일정',
  /** The meetings as drawn: dates and times, in the formats of the sheet. */
  meetings: ['2026-11-18 09:30', '2026-11-25 14:00'],
  builtIn: '2026-12-01',
} as const;

const KOREAN_STYLES = `<numFmts count="4"><numFmt numFmtId="164" formatCode="[$₩-412]#,##0"/><numFmt numFmtId="165" formatCode="yyyy&quot;년&quot; m&quot;월&quot; d&quot;일&quot;"/><numFmt numFmtId="166" formatCode="0.0%"/><numFmt numFmtId="167" formatCode="yyyy-mm-dd hh:mm"/></numFmts><fonts count="4"><font><sz val="11"/><name val="맑은 고딕"/></font><font><b/><sz val="14"/><color rgb="FFFFFFFF"/><name val="맑은 고딕"/></font><font><i/><u/><sz val="11"/><color rgb="FFC00000"/><name val="맑은 고딕"/></font><font><b/><sz val="11"/><name val="맑은 고딕"/></font></fonts><fills count="4"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF4472C4"/><bgColor indexed="64"/></patternFill></fill><fill><patternFill patternType="solid"><fgColor theme="9" tint="0.79998168889431442"/><bgColor indexed="64"/></patternFill></fill></fills><borders count="3"><border><left/><right/><top/><bottom/></border><border><left style="thin"><color indexed="64"/></left><right style="thin"><color indexed="64"/></right><top style="thin"><color indexed="64"/></top><bottom style="thin"><color indexed="64"/></bottom></border><border><left/><right/><top/><bottom style="medium"><color rgb="FF000000"/></bottom></border></borders><cellXfs count="11"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="1" fillId="2" borderId="0" applyAlignment="1"><alignment horizontal="center" vertical="center"/></xf><xf numFmtId="0" fontId="3" fillId="3" borderId="2" applyAlignment="1"><alignment horizontal="center"/></xf><xf numFmtId="3" fontId="3" fillId="0" borderId="1"/><xf numFmtId="164" fontId="0" fillId="0" borderId="1"/><xf numFmtId="166" fontId="0" fillId="0" borderId="1"/><xf numFmtId="165" fontId="0" fillId="0" borderId="1"/><xf numFmtId="0" fontId="0" fillId="0" borderId="1" applyAlignment="1"><alignment vertical="top" wrapText="1"/></xf><xf numFmtId="0" fontId="2" fillId="0" borderId="0"/><xf numFmtId="167" fontId="0" fillId="0" borderId="0"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0"/></cellXfs>`;

/** The sales sheet: a merged title, a frozen header, sums in won, rates, dates, a wrapped note. */
function salesSheet(strings: string[]): string {
  const at = (text: string): number => {
    const index = strings.indexOf(text);
    if (index >= 0) return index;
    strings.push(text);
    return strings.length - 1;
  };
  const { title, headers, quarters, note } = XLSX_TEXT;
  const sales = [1_234_567.891, 2_345_678.5, 987_654.321, 3_210_000];
  const rates = [0.125, -0.03, 0, 0.4567];
  const deadlines = [
    serial1900(2026, 3, 31),
    serial1900(2026, 6, 30),
    serial1900(2026, 9, 30),
    serial1900(2026, 12, 31),
  ];
  const rows = [
    `<row r="1" ht="30" customHeight="1">${cell('A1', at(title), 's', 1)}</row>`,
    `<row r="2">${headers.map((text, index) => cell(`${'ABCDE'[index]}2`, at(text), 's', 2)).join('')}</row>`,
    ...quarters.map((quarter, index) => {
      const row = index + 3;
      const noted = index === 0 ? cell(`E${row}`, at(note), 's', 7) : '';
      return `<row r="${row}">${cell(`A${row}`, at(quarter), 's', 7)}${cell(`B${row}`, sales[index] ?? 0, undefined, 4)}${cell(`C${row}`, rates[index] ?? 0, undefined, 5)}${cell(`D${row}`, deadlines[index] ?? 0, undefined, 6)}${noted}</row>`;
    }),
    `<row r="7">${cell('A7', at('합계'), 's', 2)}${cell('B7', '', undefined, 3, '<f>SUM(B3:B6)</f><v>7777900.712</v>')}${cell('C7', '', 'e', 0, '<f>1/0</f><v>#DIV/0!</v>')}${cell('E7', 1, 'b')}</row>`,
    `<row r="8">${cell('A8', '', 'inlineStr', 0, `<is><t>${XLSX_TEXT.inline}</t></is>`)}${cell('B8', at(XLSX_TEXT.colored), 's', 8)}</row>`,
  ];
  const view =
    '<sheetViews><sheetView tabSelected="1" workbookViewId="0"><pane ySplit="2" topLeftCell="A3" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>';
  const columns =
    '<cols><col min="1" max="1" width="12" customWidth="1"/><col min="2" max="2" width="16" customWidth="1"/><col min="3" max="3" width="10" customWidth="1"/><col min="4" max="4" width="20" customWidth="1"/><col min="5" max="5" width="34" customWidth="1"/></cols>';
  // `dimension` says less than the sheet has: it is not believed.
  return `<dimension ref="A1:B2"/>${view}<sheetFormatPr defaultRowHeight="16.5"/>${columns}<sheetData>${rows.join('')}</sheetData><mergeCells count="1"><mergeCell ref="A1:E1"/></mergeCells>`;
}

/** The schedule: meetings with their times, a hidden column, a tall row, a date of a built-in format, a drawing. */
function scheduleSheet(strings: string[]): string {
  strings.push(XLSX_TEXT.schedule, '숨긴 열');
  const [first, second] = [serial1900(2026, 11, 18, 9, 30), serial1900(2026, 11, 25, 14, 0)];
  const rows = [
    `<row r="1">${cell('A1', strings.length - 2, 's', 2)}${cell('B1', strings.length - 1, 's')}</row>`,
    `<row r="2">${cell('A2', first, undefined, 9)}</row>`,
    `<row r="3" ht="40" customHeight="1">${cell('A3', second, undefined, 9)}</row>`,
    `<row r="4">${cell('A4', serial1900(2026, 12, 1), undefined, 10)}</row>`,
  ];
  const columns =
    '<cols><col min="1" max="1" width="20" customWidth="1"/><col min="2" max="2" width="12" hidden="1" customWidth="1"/></cols>';
  return `${columns}<sheetData>${rows.join('')}</sheetData><drawing r:id="rId1"/>`;
}

/** A workbook in Korean: the sales and the schedule shown, a third sheet hidden. */
export function koreanXlsx(): Buffer {
  const strings: string[] = [];
  const sales = salesSheet(strings);
  const schedule = scheduleSheet(strings);
  strings.push('보이면 안 되는 시트');
  const hidden = `<sheetData><row r="1">${cell('A1', strings.length - 1, 's')}</row></sheetData>`;
  return workbook({
    sheets: [
      { name: XLSX_TEXT.sheets[0], xml: sales },
      { name: XLSX_TEXT.hidden, xml: hidden, state: 'hidden' },
      { name: XLSX_TEXT.sheets[1], xml: schedule },
    ],
    strings,
    stylesXml: KOREAN_STYLES,
  });
}

/** A sheet of `rows` rows and `columns` columns, each cell its row and column; the first row frozen. */
export function largeXlsx(rows: number, columns: number): Buffer {
  const letters = (index: number): string => {
    let name = '';
    for (let rest = index + 1; rest > 0; rest = Math.floor((rest - 1) / 26)) {
      name = String.fromCharCode(65 + ((rest - 1) % 26)) + name;
    }
    return name;
  };
  const lines: string[] = [];
  for (let row = 1; row <= rows; row += 1) {
    const cells: string[] = [];
    for (let column = 0; column < columns; column += 1) {
      const reference = `${letters(column)}${row}`;
      cells.push(
        column === 0
          ? cell(reference, `행 ${row}`, 'str')
          : `<c r="${reference}"><v>${row * 100 + column}</v></c>`,
      );
    }
    lines.push(`<row r="${row}">${cells.join('')}</row>`);
  }
  const view =
    '<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" state="frozen"/></sheetView></sheetViews>';
  return workbook({
    sheets: [{ name: '큰 시트', xml: `${view}<sheetData>${lines.join('')}</sheetData>` }],
  });
}

/**
 * Styles for whole rows and columns: 1 a red fill, 2 a blue fill, 3 bold
 * alone (nothing an empty cell shows), 4 a thin border all round.
 */
export const ROW_COLUMN_STYLES =
  '<fonts count="2"><font><sz val="11"/><name val="맑은 고딕"/></font><font><b/><sz val="11"/><name val="맑은 고딕"/></font></fonts><fills count="4"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFFF0000"/></patternFill></fill><fill><patternFill patternType="solid"><fgColor rgb="FF0000FF"/></patternFill></fill></fills><borders count="2"><border><left/><right/><top/><bottom/></border><border><left style="thin"/><right style="thin"/><top style="thin"/><bottom style="thin"/></border></borders><cellXfs count="5"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="0" fillId="2" borderId="0" applyFill="1"/><xf numFmtId="0" fontId="0" fillId="3" borderId="0" applyFill="1"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" applyFont="1"/><xf numFmtId="0" fontId="0" fillId="0" borderId="1" applyBorder="1"/></cellXfs>';

/** The text in A1 of `rowColumnXlsx`. */
export const ROW_COLUMN_TITLE = '행과 열의 서식';

/**
 * A sheet of `ROW_COLUMN_STYLES` whose rows and columns have styles of
 * their own: column C blue; row 2 red (and B2 written in it, of no style);
 * row 4 bold alone, over column C; row 6 red without `customFormat`, which
 * Excel does not take; row 7 red with E7 written empty and of no style; the
 * merge C9:D10 of no cell written; row 12 bordered.
 */
export function rowColumnXlsx(): Buffer {
  const data = [
    `<row r="1"><c r="A1" t="inlineStr"><is><t>${esc(ROW_COLUMN_TITLE)}</t></is></c></row>`,
    '<row r="2" s="1" customFormat="1"><c r="B2"><v>7</v></c></row>',
    '<row r="4" s="3" customFormat="1"/>',
    '<row r="6" s="1"/>',
    '<row r="7" s="1" customFormat="1"><c r="E7"/></row>',
    '<row r="12" s="4" customFormat="1"/>',
  ].join('');
  return workbook({
    sheets: [
      {
        name: '서식',
        xml: `<cols><col min="3" max="3" width="9" style="2" customWidth="1"/></cols><sheetData>${data}</sheetData><mergeCells count="1"><mergeCell ref="C9:D10"/></mergeCells>`,
      },
    ],
    stylesXml: ROW_COLUMN_STYLES,
  });
}
