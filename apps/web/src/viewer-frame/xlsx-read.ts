/**
 * Reads a workbook (XLSX) in the spreadsheet worker into what the frame
 * draws (`xlsx-protocol.ts`): the package's relationships lead from its
 * root to the workbook part, and from it to the sheets (in the workbook's
 * order; hidden ones left out), the shared strings, the styles and the
 * theme. Chart sheets are kept as sheets of nothing but drawings.
 *
 * Everything is read at once, and the ceilings (`xlsx-xml.ts`) hold for the
 * whole workbook: the worker answers once and is ended, so nothing is read
 * later, when the viewer's time limit no longer runs.
 */
import { Archive, relationshipsOf, resolvePart } from './xlsx-archive.ts';
import { DEFAULT_THEME, readTheme } from './xlsx-colors.ts';
import { drawingSheet, readSheet, type SheetContext } from './xlsx-sheet.ts';
import { readSharedStrings, Strings } from './xlsx-strings.ts';
import { readStyles, type Styles } from './xlsx-styles.ts';
import { TooMuch } from './worker-xml.ts';
import type { FromXlsxWorker, Workbook } from './xlsx-protocol.ts';
import { attribute, ceilings, flag, readXml, SHEET_NAME_LENGTH, Unreadable } from './xlsx-xml.ts';

const RELATIONSHIP = /\/relationships\/(\w+)$/;

interface Relationship {
  type: string;
  target: string;
}

/** A part's relationships by their ids, targets resolved; none when it has no relationships part. */
async function relationships(archive: Archive, part: string): Promise<Map<string, Relationship>> {
  const found = new Map<string, Relationship>();
  const text = await archive.text(relationshipsOf(part));
  if (text === undefined) return found;
  readXml(text, {
    open: (name, attributes) => {
      if (name !== 'Relationship' || attribute(attributes, 'TargetMode') === 'External') return;
      const id = attribute(attributes, 'Id');
      const target = attribute(attributes, 'Target');
      const type = RELATIONSHIP.exec(attribute(attributes, 'Type') ?? '')?.[1];
      if (id && target && type) found.set(id, { type, target: resolvePart(part, target) });
    },
  });
  return found;
}

interface SheetEntry {
  name: string;
  id: string;
  hidden: boolean;
}

/** The workbook part: its sheets in order and its date system. */
function readWorkbookPart(text: string): { sheets: SheetEntry[]; date1904: boolean } {
  const sheets: SheetEntry[] = [];
  let date1904 = false;
  readXml(text, {
    open: (name, attributes) => {
      if (name === 'workbookPr') date1904 = flag(attribute(attributes, 'date1904'));
      else if (name === 'sheet') {
        const state = attribute(attributes, 'state');
        // A sheet must have a name; one without is called as Excel would call it.
        const named = (attribute(attributes, 'name') ?? '').slice(0, SHEET_NAME_LENGTH);
        sheets.push({
          name: named === '' ? `Sheet${sheets.length + 1}` : named,
          id: attribute(attributes, 'id') ?? '',
          hidden: state === 'hidden' || state === 'veryHidden',
        });
      }
    },
  });
  return { sheets, date1904 };
}

/** The first of a part's relationships of a type. */
const ofType = (links: Map<string, Relationship>, type: string): string | undefined =>
  [...links.values()].find((link) => link.type === type)?.target;

/** Pixels of one digit of the default font: Excel sizes columns by it. */
function digitWidth(styles: Styles): number {
  const font = styles.fonts[0];
  const korean = font ? /맑은|Malgun|굴림|Gulim|돋움|Dotum/i.test(font.family) : false;
  return Math.max(1, Math.round(((korean ? 8 : 7) * (font?.size ?? 11)) / 11));
}

/** Reads the workbook; throws `Unreadable` or `TooMuch` (from `worker-xml.ts`). */
export async function readWorkbook(bytes: ArrayBuffer): Promise<Workbook> {
  const archive = new Archive(bytes);
  const limits = ceilings();
  const root = await relationships(archive, '');
  const main = ofType(root, 'officeDocument') ?? 'xl/workbook.xml';
  const text = await archive.text(main);
  if (text === undefined) throw new Unreadable('no workbook part');
  const { sheets: entries, date1904 } = readWorkbookPart(text);
  const links = await relationships(archive, main);

  const theme = ofType(links, 'theme');
  const themeText = theme === undefined ? undefined : await archive.text(theme);
  const themeColors = themeText === undefined ? DEFAULT_THEME : readTheme(themeText);
  const stylesPart = ofType(links, 'styles');
  const stylesText = stylesPart === undefined ? undefined : await archive.text(stylesPart);
  const styles = readStyles(stylesText ?? '<styleSheet/>', themeColors, limits);

  const strings = new Strings(limits.strings, limits.text);
  const sharedPart = ofType(links, 'sharedStrings');
  const sharedText = sharedPart === undefined ? undefined : await archive.text(sharedPart);
  if (sharedText !== undefined) readSharedStrings(sharedText, strings);

  const context: SheetContext = {
    strings,
    shared: strings.table.length,
    styles: styles.styles.length,
    system: date1904 ? 1904 : 1900,
    digitWidth: digitWidth(styles),
    ceilings: limits,
  };
  const sheets = [];
  // A part is one sheet: one that more entries name is read for the first (Excel names each once).
  const taken = new Set<string>();
  for (const entry of entries) {
    if (entry.hidden) continue;
    limits.sheets.add();
    const link = links.get(entry.id);
    if (link?.type === 'chartsheet') {
      sheets.push(drawingSheet(entry.name, context.digitWidth));
      continue;
    }
    if (link?.type !== 'worksheet' || taken.has(link.target)) continue;
    taken.add(link.target);
    const sheetText = await archive.text(link.target);
    if (sheetText === undefined) continue;
    sheets.push(readSheet(entry.name, sheetText, context));
  }
  if (sheets.length === 0) throw new Unreadable('no sheet to show');
  return finish(styles, strings, date1904, sheets);
}

/**
 * The answer: the styles' format numbers made indices into one list, of the
 * workbook's own codes and the numbers of built-in ones (which the frame
 * writes in the reader's language).
 */
function finish(
  styles: Styles,
  strings: Strings,
  date1904: boolean,
  sheets: Workbook['sheets'],
): Workbook {
  const formats: (string | number)[] = [];
  const index = new Map<number, number>();
  for (const style of styles.styles) {
    let at = index.get(style.format);
    if (at === undefined) {
      at = formats.length;
      formats.push(styles.codes.get(style.format) ?? style.format);
      index.set(style.format, at);
    }
    style.format = at;
  }
  return {
    date1904,
    strings: strings.table,
    formats,
    fonts: styles.fonts,
    borders: styles.borders,
    styles: styles.styles,
    palette: styles.palette,
    sheets,
  };
}

/** Why a workbook could not be read, from what was thrown. */
export function failureOf(error: unknown): FromXlsxWorker {
  // A string or an array past the engine's limits is a workbook too large too.
  if (error instanceof TooMuch || error instanceof RangeError) {
    return { kind: 'failed', reason: 'too-large' };
  }
  return { kind: 'failed', reason: error instanceof Unreadable ? 'unreadable' : 'renderer' };
}
