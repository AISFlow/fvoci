/**
 * The number format codes of a spreadsheet (`#,##0.00`, `yyyy-mm-dd`,
 * `[Red]0%`), read into sections of tokens: what `xlsx-format.ts` draws a
 * cell's value with.
 *
 * A code has up to four sections, for positive numbers, negative ones,
 * zero and text, unless conditions (`[>=100]`) say which number goes where.
 * A section is a run of literals (quoted, after `\`, or the characters that
 * stand for themselves) and of placeholders: digits (`0 # ?`), the decimal
 * point, thousands commas, `%`, an exponent (`E+`), the text (`@`), or the
 * parts of a date and a time. What this does not read is kept as a literal,
 * and a code it cannot make sense of is drawn as `General`.
 */
import { rgbColor } from './xlsx-protocol.ts';

/** A part of a date or a time, and how wide it is drawn. */
export type DatePart =
  | 'year'
  | 'month'
  | 'day'
  | 'weekday'
  | 'hour'
  | 'minute'
  | 'second'
  | 'hours'
  | 'minutes'
  | 'seconds'
  | 'ampm'
  | 'subsecond';

export type Token =
  | { kind: 'literal'; text: string }
  | { kind: 'digit'; char: '0' | '#' | '?' }
  | { kind: 'point' }
  | { kind: 'comma' }
  | { kind: 'percent' }
  | { kind: 'exponent'; sign: '+' | '-' }
  | { kind: 'slash' }
  | { kind: 'text' }
  | { kind: 'general' }
  /**
   * `width` is the number of letters (`yyyy` 4, `mmm` 3); for `ampm` it is 2
   * for `AM/PM` and 1 for `A/P`, and `lower` says the code wrote it so; for
   * a weekday written `aaa` (the Korean and Japanese forms), `local`.
   */
  | { kind: 'date'; part: DatePart; width: number; lower?: boolean; local?: boolean };

export type Condition = { op: '<' | '<=' | '>' | '>=' | '=' | '<>'; value: number };

export interface Section {
  tokens: Token[];
  /** The colour the section draws in: `#rrggbb`. */
  color?: string;
  condition?: Condition;
  /** A language tag from `[$-412]`: the language its names of months and days are in. */
  locale?: string;
  kind: 'number' | 'date' | 'text' | 'general';
}

/** The longest code read: Excel's own limit. A longer one is drawn as `General`. */
export const CODE_LENGTH_LIMIT = 255;

/** The colours a section names, as a workbook writes colours (`RRGGBB`). */
const NAMED_COLORS: Record<string, string> = {
  black: '000000',
  white: 'ffffff',
  red: 'ff0000',
  green: '00ff00',
  blue: '0000ff',
  yellow: 'ffff00',
  magenta: 'ff00ff',
  cyan: '00ffff',
};

/** Language tags of the Windows locale numbers that codes name most (`[$-412]`). */
const LOCALES: Record<number, string> = {
  0x409: 'en-US',
  0x809: 'en-GB',
  0x411: 'ja',
  0x412: 'ko',
  0x804: 'zh-CN',
  0x404: 'zh-TW',
  0x407: 'de',
  0x40c: 'fr',
};

/** Characters that stand for themselves in a code without quotes. */
const PLAIN = /^[$\-+/():!^&'~{}<>= ]$/;

/** What is in brackets: an elapsed time, a colour, a condition, a currency and a locale, or nothing read. */
function bracket(content: string, section: Section, palette: readonly string[]): Token | undefined {
  const elapsed = /^(h+|m+|s+)$/i.exec(content);
  if (elapsed) {
    const letter = content[0]?.toLowerCase();
    const part = letter === 'h' ? 'hours' : letter === 'm' ? 'minutes' : 'seconds';
    return { kind: 'date', part, width: content.length };
  }
  const lower = content.toLowerCase();
  if (Object.hasOwn(NAMED_COLORS, lower)) {
    section.color = rgbColor(NAMED_COLORS[lower]);
    return undefined;
  }
  const indexed = /^color\s*(\d{1,2})$/i.exec(content);
  if (indexed) {
    // `[Color1]` is the palette's ninth entry: the first eight repeat the named ones.
    section.color = palette[Number(indexed[1]) + 7];
    return undefined;
  }
  const condition = /^(<=|>=|<>|<|>|=)\s*(-?\d+(?:\.\d+)?(?:e[+-]?\d+)?)$/i.exec(content);
  if (condition) {
    section.condition = { op: condition[1] as Condition['op'], value: Number(condition[2]) };
    return undefined;
  }
  const currency = /^\$([^-]*)(?:-([0-9a-f]{1,8}))?$/i.exec(content);
  if (currency) {
    const locale = currency[2] ? LOCALES[Number.parseInt(currency[2], 16) & 0xffff] : undefined;
    if (locale) section.locale = locale;
    return currency[1] ? { kind: 'literal', text: currency[1] } : undefined;
  }
  return undefined;
}

/** How many times `letter` repeats from `at`, in either case. */
function run(code: string, at: number, letter: string): number {
  let end = at;
  while (end < code.length && code[end]?.toLowerCase() === letter) end += 1;
  return end - at;
}

/** The date token of a run of letters, and how long the run is; undefined when it is none. */
function dateRun(code: string, at: number): [Token, number] | undefined {
  const letter = code[at]?.toLowerCase() ?? '';
  if (code.slice(at, at + 5).toUpperCase() === 'AM/PM') {
    const lower = code.slice(at, at + 2) === 'am';
    return [{ kind: 'date', part: 'ampm', width: 2, lower }, 5];
  }
  if (code.slice(at, at + 3).toUpperCase() === 'A/P') {
    return [{ kind: 'date', part: 'ampm', width: 1, lower: code[at] === 'a' }, 3];
  }
  const length = run(code, at, letter);
  switch (letter) {
    case 'y':
      return [{ kind: 'date', part: 'year', width: length <= 2 ? 2 : 4 }, length];
    case 'e':
      return [{ kind: 'date', part: 'year', width: 4 }, length];
    case 'm':
      return [{ kind: 'date', part: 'month', width: Math.min(length, 5) }, length];
    case 'd':
      return length >= 3
        ? [{ kind: 'date', part: 'weekday', width: Math.min(length, 4) }, length]
        : [{ kind: 'date', part: 'day', width: length }, length];
    case 'a':
      return length >= 3
        ? [{ kind: 'date', part: 'weekday', width: Math.min(length, 4), local: true }, length]
        : undefined;
    case 'h':
      return [{ kind: 'date', part: 'hour', width: Math.min(length, 2) }, length];
    case 's':
      return [{ kind: 'date', part: 'second', width: Math.min(length, 2) }, length];
    default:
      return undefined;
  }
}

/**
 * The tokens of one section's text; fills its colour, condition and locale.
 * Undefined for a quote or a bracket that does not close.
 */
function tokenize(code: string, section: Section, palette: readonly string[]): Token[] | undefined {
  const tokens: Token[] = [];
  const literal = (text: string): void => {
    const last = tokens.at(-1);
    if (last?.kind === 'literal') last.text += text;
    else tokens.push({ kind: 'literal', text });
  };
  let at = 0;
  while (at < code.length) {
    const char = code[at] ?? '';
    if (char === '"') {
      const end = code.indexOf('"', at + 1);
      if (end < 0) return undefined;
      literal(code.slice(at + 1, end));
      at = end + 1;
    } else if (char === '\\') {
      literal(code[at + 1] ?? '');
      at += 2;
    } else if (char === '_') {
      // Room as wide as the next character: a space here.
      literal(' ');
      at += 2;
    } else if (char === '*') {
      // The next character repeated to fill the cell: left out.
      at += 2;
    } else if (char === '[') {
      const end = code.indexOf(']', at);
      if (end < 0) return undefined;
      const token = bracket(code.slice(at + 1, end), section, palette);
      if (token?.kind === 'literal') literal(token.text);
      else if (token) tokens.push(token);
      at = end + 1;
    } else if (code.slice(at, at + 7).toLowerCase() === 'general') {
      tokens.push({ kind: 'general' });
      at += 7;
    } else {
      at += symbol(code, at, tokens, literal);
    }
  }
  return tokens;
}

/** Reads the token at `at` into `tokens`; returns how many characters it took. */
function symbol(
  code: string,
  at: number,
  tokens: Token[],
  literal: (text: string) => void,
): number {
  const char = code[at] ?? '';
  const previous = tokens.at(-1);
  if (char === '0' || char === '#' || char === '?') {
    // Zeros after a second's point are its fraction (`ss.00`).
    if (char === '0' && previous?.kind === 'date' && previous.part === 'subsecond') {
      previous.width += 1;
      return 1;
    }
    tokens.push({ kind: 'digit', char });
    return 1;
  }
  if (char === '.') {
    const second = previous?.kind === 'date' && /^(second|seconds)$/.test(previous.part);
    if (second && code[at + 1] === '0') {
      tokens.push({ kind: 'date', part: 'subsecond', width: 0 });
      return 1;
    }
    tokens.push({ kind: 'point' });
    return 1;
  }
  if (char === ',') tokens.push({ kind: 'comma' });
  else if (char === '%') tokens.push({ kind: 'percent' });
  else if (char === '@') tokens.push({ kind: 'text' });
  else if (char === '/') tokens.push({ kind: 'slash' });
  else if ((char === 'E' || char === 'e') && (code[at + 1] === '+' || code[at + 1] === '-')) {
    tokens.push({ kind: 'exponent', sign: code[at + 1] as '+' | '-' });
    return 2;
  } else {
    const date = dateRun(code, at);
    if (date) {
      tokens.push(date[0]);
      return date[1];
    }
    // `g` (an era's name) and `b` (a Buddhist year) are left out; anything else stands for itself.
    if (!/[gb]/i.test(char) || PLAIN.test(char)) literal(char);
  }
  return 1;
}

/** Reads `m` as minutes where it follows an hour or comes before a second, as Excel does. */
function minutes(tokens: Token[]): void {
  const dates = tokens.filter(
    (token): token is Extract<Token, { kind: 'date' }> => token.kind === 'date',
  );
  dates.forEach((token, index) => {
    if (token.part !== 'month' || token.width > 2) return;
    const before = dates[index - 1]?.part;
    const after = dates[index + 1]?.part;
    if (before === 'hour' || before === 'hours' || after === 'second' || after === 'seconds') {
      token.part = 'minute';
    }
  });
}

/** Splits a code at the semicolons that are not quoted, escaped or bracketed. */
function split(code: string): string[] {
  const parts: string[] = [];
  let start = 0;
  let quoted = false;
  let bracketed = false;
  for (let at = 0; at < code.length; at += 1) {
    const char = code[at];
    if (char === '\\' && !quoted) at += 1;
    else if (char === '"') quoted = !quoted;
    else if (!quoted && char === '[') bracketed = true;
    else if (!quoted && char === ']') bracketed = false;
    else if (!quoted && !bracketed && char === ';') {
      parts.push(code.slice(start, at));
      start = at + 1;
    }
  }
  parts.push(code.slice(start));
  return parts;
}

/** Whether a section is a fraction (`# ?/?`): digits on both sides of a slash. */
function isFraction(tokens: readonly Token[]): boolean {
  const slash = tokens.findIndex((token) => token.kind === 'slash');
  return (
    slash > 0 &&
    tokens.slice(0, slash).some((token) => token.kind === 'digit') &&
    tokens.slice(slash).some((token) => token.kind === 'digit')
  );
}

function kindOf(tokens: readonly Token[]): Section['kind'] {
  if (tokens.some((token) => token.kind === 'date')) return 'date';
  if (tokens.some((token) => token.kind === 'digit')) return 'number';
  if (tokens.some((token) => token.kind === 'text')) return 'text';
  if (tokens.some((token) => token.kind === 'general')) return 'general';
  // A section of literals only (`"-"`) draws them for its numbers.
  return 'number';
}

const GENERAL: Section[] = [{ tokens: [{ kind: 'general' }], kind: 'general' }];

/**
 * The sections of a code: four at most (more are ignored, as Excel does).
 * `palette` gives the colours of `[ColorN]`.
 */
export function readCode(code: string, palette: readonly string[] = []): Section[] {
  if (code.length > CODE_LENGTH_LIMIT || code.trim() === '') return GENERAL;
  const sections: Section[] = [];
  for (const part of split(code).slice(0, 4)) {
    const section: Section = { tokens: [], kind: 'general' };
    const tokens = tokenize(part, section, palette);
    if (!tokens) return GENERAL;
    minutes(tokens);
    // A fraction is not drawn as one: as `General`, in the section's colour.
    section.tokens = isFraction(tokens) ? [{ kind: 'general' }] : tokens;
    section.kind = kindOf(section.tokens);
    sections.push(section);
  }
  return sections;
}
