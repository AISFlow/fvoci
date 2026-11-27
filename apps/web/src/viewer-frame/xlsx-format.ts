/**
 * A cell's value as the spreadsheet shows it: its number format (a
 * built-in one by number, or the workbook's own code) applied to its
 * stored value. A formula's value is the one the file stored; nothing is
 * calculated here.
 *
 * Read and drawn: `General`; digits with thousands, decimals, percent,
 * scaling commas and exponents; literals and currency symbols (`[$₩-412]`);
 * colours (`[Red]`); conditions; dates and times of both date systems,
 * elapsed times and AM/PM; text sections (`@`). A fraction (`# ?/?`) is
 * drawn as `General`. A code that cannot be read draws the value as it is.
 */
import { readCode, type Section } from './xlsx-codes.ts';
import { drawDate, type DateSystem } from './xlsx-dates.ts';
import { drawDigits, general } from './xlsx-digits.ts';

/** The built-in formats by number: the ones ECMA-376 lists, and the Korean ones Excel writes as numbers. */
const BUILT_IN: Record<number, string> = {
  0: 'General',
  1: '0',
  2: '0.00',
  3: '#,##0',
  4: '#,##0.00',
  5: '"$"#,##0_);("$"#,##0)',
  6: '"$"#,##0_);[Red]("$"#,##0)',
  7: '"$"#,##0.00_);("$"#,##0.00)',
  8: '"$"#,##0.00_);[Red]("$"#,##0.00)',
  9: '0%',
  10: '0.00%',
  11: '0.00E+00',
  12: '# ?/?',
  13: '# ??/??',
  15: 'd-mmm-yy',
  16: 'd-mmm',
  17: 'mmm-yy',
  18: 'h:mm AM/PM',
  19: 'h:mm:ss AM/PM',
  20: 'h:mm',
  21: 'h:mm:ss',
  27: 'yyyy"년" mm"월" dd"일"',
  28: 'mm-dd',
  29: 'mm-dd',
  30: 'mm-dd-yy',
  31: 'yyyy"년" mm"월" dd"일"',
  32: 'h"시" mm"분"',
  33: 'h"시" mm"분" ss"초"',
  34: 'yyyy-mm-dd',
  35: 'yyyy-mm-dd',
  36: 'yyyy"년" mm"월" dd"일"',
  37: '#,##0 ;(#,##0)',
  38: '#,##0 ;[Red](#,##0)',
  39: '#,##0.00;(#,##0.00)',
  40: '#,##0.00;[Red](#,##0.00)',
  41: '_(* #,##0_);_(* (#,##0);_(* "-"_);_(@_)',
  42: '_("$"* #,##0_);_("$"* (#,##0);_("$"* "-"_);_(@_)',
  43: '_(* #,##0.00_);_(* (#,##0.00);_(* "-"??_);_(@_)',
  44: '_("$"* #,##0.00_);_("$"* (#,##0.00);_("$"* "-"??_);_(@_)',
  45: 'mm:ss',
  46: '[h]:mm:ss',
  47: 'mmss.0',
  48: '##0.0E+0',
  49: '@',
  50: 'yyyy"년" mm"월" dd"일"',
  51: 'mm-dd',
  52: 'yyyy-mm-dd',
  53: 'yyyy-mm-dd',
  54: 'mm-dd',
  55: 'yyyy-mm-dd',
  56: 'yyyy-mm-dd',
  57: 'yyyy"년" mm"월" dd"일"',
  58: 'mm-dd',
};

/** The short date (14) and the date and time (22): as the reader's system writes them. */
function systemDate(language: string, withTime: boolean): string {
  const date = language.startsWith('ko')
    ? 'yyyy-mm-dd'
    : language.startsWith('ja') || language.startsWith('zh')
      ? 'yyyy/m/d'
      : 'm/d/yyyy';
  return withTime ? `${date} h:mm` : date;
}

/** The code of a format by its number, given the workbook's own (`numFmts`). */
export function codeOf(id: number, own: ReadonlyMap<number, string>, language: string): string {
  const mine = own.get(id);
  if (mine !== undefined) return mine;
  if (id === 14) return systemDate(language, false);
  if (id === 22) return systemDate(language, true);
  return BUILT_IN[id] ?? 'General';
}

/** What a drawn value is: its text, and the colour its section asks for. */
export interface Drawn {
  text: string;
  color?: string;
}

/** Whether a number meets a section's condition. */
function meets(section: Section | undefined, value: number): boolean {
  const condition = section?.condition;
  if (!condition) return false;
  switch (condition.op) {
    case '<':
      return value < condition.value;
    case '<=':
      return value <= condition.value;
    case '>':
      return value > condition.value;
    case '>=':
      return value >= condition.value;
    case '=':
      return value === condition.value;
    case '<>':
      return value !== condition.value;
  }
}

/** Whether a condition is met by numbers below zero alone. */
function onlyNegative(condition: Section['condition']): boolean {
  if (!condition) return false;
  return (
    (condition.op === '<' && condition.value <= 0) || (condition.op === '<=' && condition.value < 0)
  );
}

/** The section a number is drawn with, and whether a minus goes before what it draws. */
function sectionFor(
  sections: readonly Section[],
  value: number,
): { section: Section | undefined; minus: boolean } {
  // A fourth section is the text's; a lone text section draws numbers as they are.
  const numbers =
    sections.length === 4 ? sections.slice(0, 3) : sections.filter((each) => each.kind !== 'text');
  const [first, second, third] = numbers;
  const negative = value < 0;
  if (first?.condition || second?.condition) {
    // A section whose condition takes only numbers below zero (`[<0]`) writes its own sign.
    const signed = (section: Section | undefined) => ({
      section,
      minus: negative && !onlyNegative(section?.condition),
    });
    if (meets(first, value)) return signed(first);
    if (meets(second, value)) return signed(second);
    // Neither condition: the third section, or the second when it has none.
    return signed(third ?? (second?.condition ? undefined : second));
  }
  // The negative section draws the number without its sign: the code writes what it wants.
  if (negative && second) return { section: second, minus: false };
  if (value === 0 && third) return { section: third, minus: false };
  return { section: first, minus: negative };
}

/** A cell's number drawn with a format code. */
export class NumberFormat {
  readonly #sections: Section[];
  readonly #system: DateSystem;
  readonly #language: string;

  constructor(code: string, system: DateSystem, language: string, palette: readonly string[]) {
    this.#sections = readCode(code, palette);
    this.#system = system;
    this.#language = language;
  }

  number(value: number): Drawn {
    const { section, minus } = sectionFor(this.#sections, value);
    if (!section || section.kind === 'text') return { text: general(value) };
    const size = Math.abs(value);
    let text: string;
    if (section.kind === 'date') {
      text = drawDate(value, section.tokens, this.#system, this.#language, section.locale);
    } else {
      const drawn =
        section.kind === 'number'
          ? drawDigits(size, section.tokens)
          : section.tokens
              .map((token) =>
                token.kind === 'literal'
                  ? token.text
                  : token.kind === 'general'
                    ? general(size)
                    : '',
              )
              .join('');
      // A number below zero keeps its minus, at the very front, even drawn as zeros:
      // Excel draws -0.001 with "0.00" as "-0.00", and with `"Q1 "0.00` as "-Q1 0.00".
      text = minus ? `-${drawn}` : drawn;
    }
    return section.color ? { text, color: section.color } : { text };
  }

  /** A text cell drawn with the format's text section, or as it is. */
  text(value: string): Drawn {
    const section =
      this.#sections.length === 4
        ? this.#sections[3]
        : this.#sections.find((candidate) => candidate.kind === 'text');
    if (!section) return { text: value };
    const text = section.tokens
      .map((token) => (token.kind === 'literal' ? token.text : token.kind === 'text' ? value : ''))
      .join('');
    return section.color ? { text, color: section.color } : { text };
  }
}
