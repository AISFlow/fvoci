/**
 * A cell's number drawn as a date and a time, with the date tokens of a
 * format section (`yyyy-mm-dd`, `h:mm AM/PM`, `[h]:mm:ss`).
 *
 * A spreadsheet keeps a date as a number of days, in one of two systems:
 * from 1900 (day 1 is 1 January 1900) or from 1904 (day 0 is 1 January
 * 1904, as old Macs counted). The 1900 system counts 29 February 1900,
 * which never was, as day 60, as Lotus 1-2-3 did: days before it are a day
 * off from the calendar, and its weekdays run on as though it had been.
 * The time is the fraction of the day, rounded to what the code shows (to
 * the second unless it shows hundredths), which can carry it into the next
 * day.
 */
import type { Token } from './xlsx-codes.ts';

export type DateSystem = 1900 | 1904;

/** The largest day drawn: 31 December 9999, as in Excel; past it, or before day 0, `#` fills the cell. */
const LAST_DAY: Record<DateSystem, number> = { 1900: 2_958_465, 1904: 2_957_003 };
export const NOT_A_DATE = '########';

const MS_PER_DAY = 86_400_000;
/** 31 December 1899 and 1 January 1904, in milliseconds since 1970 (UTC). */
const START: Record<DateSystem, number> = {
  1900: Date.UTC(1899, 11, 31),
  1904: Date.UTC(1904, 0, 1),
};

interface Moment {
  year: number;
  /** 1 to 12. */
  month: number;
  /** 0 for the 1900 system's day 0 ("1900-01-00"), else 1 to 31. */
  day: number;
  /** 0 Sunday to 6 Saturday, as the spreadsheet counts. */
  weekday: number;
  hour: number;
  minute: number;
  second: number;
  /** The fraction of a second, in units of `10^-digits`. */
  subsecond: number;
  /** The whole value in seconds (rounded), for `[h]`, `[m]` and `[s]`. */
  total: number;
}

/** The day of the calendar a whole day of a system is. */
function calendarDay(
  day: number,
  system: DateSystem,
): { year: number; month: number; day: number } {
  if (system === 1900) {
    if (day === 0) return { year: 1900, month: 1, day: 0 };
    if (day === 60) return { year: 1900, month: 2, day: 29 };
  }
  // After the day that never was, the count is one ahead of the calendar.
  const days = system === 1900 && day > 60 ? day - 1 : day;
  const date = new Date(START[system] + days * MS_PER_DAY);
  return { year: date.getUTCFullYear(), month: date.getUTCMonth() + 1, day: date.getUTCDate() };
}

/** The moment a value is, rounded to `digits` decimals of a second; undefined past the dates drawn. */
export function momentOf(value: number, system: DateSystem, digits: number): Moment | undefined {
  if (!Number.isFinite(value) || value < 0 || value >= LAST_DAY[system] + 1) return undefined;
  const scale = 10 ** digits;
  const units = Math.round(value * 86_400 * scale);
  const perDay = 86_400 * scale;
  const day = Math.floor(units / perDay);
  if (day > LAST_DAY[system]) return undefined;
  const rest = units - day * perDay;
  const seconds = Math.floor(rest / scale);
  const date = calendarDay(day, system);
  return {
    ...date,
    weekday: (day + (system === 1900 ? 6 : 5)) % 7,
    hour: Math.floor(seconds / 3600),
    minute: Math.floor(seconds / 60) % 60,
    second: seconds % 60,
    subsecond: rest - seconds * scale,
    total: Math.floor(units / scale),
  };
}

/** Names of months and weekdays in a language, from the browser's own tables. */
function names(
  locale: string,
  kind: 'month' | 'weekday',
  width: 'short' | 'long' | 'narrow',
): string[] {
  const format = new Intl.DateTimeFormat(locale, {
    timeZone: 'UTC',
    ...(kind === 'month' ? { month: width } : { weekday: width }),
  });
  // 2023-01-01 was a Sunday: twelve months from it, and seven days.
  const count = kind === 'month' ? 12 : 7;
  return Array.from({ length: count }, (_, index) =>
    format.format(kind === 'month' ? Date.UTC(2023, index, 1) : Date.UTC(2023, 0, 1 + index)),
  );
}

const cache = new Map<string, string[]>();
function named(
  locale: string,
  kind: 'month' | 'weekday',
  width: 'short' | 'long' | 'narrow',
): string[] {
  const key = `${locale}|${kind}|${width}`;
  let found = cache.get(key);
  if (!found) {
    found = names(locale, kind, width);
    cache.set(key, found);
  }
  return found;
}

const pad = (value: number, width: number): string => String(value).padStart(width, '0');

/** The morning and the afternoon in the languages that say them otherwise than "AM" and "PM". */
const HALVES: Record<string, [string, string]> = {
  ko: ['오전', '오후'],
  ja: ['午前', '午後'],
  zh: ['上午', '下午'],
};

/**
 * "AM/PM" as the section's locale says it (`[$-412]AM/PM` is "오전" and
 * "오후"), in English otherwise, as Excel draws it; "A/P" is always a letter.
 */
function half(hour: number, token: Extract<Token, { kind: 'date' }>, locale: string): string {
  const afternoon = hour >= 12;
  const own = HALVES[locale.slice(0, 2)];
  if (own && token.width === 2) return own[afternoon ? 1 : 0];
  const text = token.width === 2 ? (afternoon ? 'PM' : 'AM') : afternoon ? 'P' : 'A';
  return token.lower ? text.toLowerCase() : text;
}

/** One date token drawn for a moment. */
function part(
  token: Extract<Token, { kind: 'date' }>,
  moment: Moment,
  twelve: boolean,
  locale: string,
  language: string,
): string {
  const { width } = token;
  switch (token.part) {
    case 'year':
      return width === 2 ? pad(moment.year % 100, 2) : pad(moment.year, 4);
    case 'month':
      if (width <= 2) return pad(moment.month, width);
      return (
        named(locale, 'month', width === 3 ? 'short' : width === 4 ? 'long' : 'narrow')[
          moment.month - 1
        ] ?? ''
      );
    case 'day':
      return pad(moment.day, width);
    case 'weekday': {
      const where = token.local ? language : locale;
      return named(where, 'weekday', width === 3 ? 'short' : 'long')[moment.weekday] ?? '';
    }
    case 'hour': {
      const hour = twelve ? moment.hour % 12 || 12 : moment.hour;
      return pad(hour, width);
    }
    case 'minute':
      return pad(moment.minute, width);
    case 'second':
      return pad(moment.second, width);
    case 'hours':
      return pad(Math.floor(moment.total / 3600), width);
    case 'minutes':
      return pad(Math.floor(moment.total / 60), width);
    case 'seconds':
      return pad(moment.total, width);
    case 'ampm':
      return half(moment.hour, token, locale);
    case 'subsecond':
      return width === 0 ? '' : `.${pad(moment.subsecond, width)}`;
  }
}

/**
 * Draws `value` with a date section's tokens. Month and weekday names and
 * "AM/PM" are in the section's locale (`[$-412]`), else in English as
 * Excel's own; the weekdays of `aaa` are in the reader's `language`, as
 * Excel has them in the system's.
 */
export function drawDate(
  value: number,
  tokens: readonly Token[],
  system: DateSystem,
  language: string,
  locale = 'en-US',
): string {
  const dates = tokens.filter(
    (token): token is Extract<Token, { kind: 'date' }> => token.kind === 'date',
  );
  const digits = dates.find((token) => token.part === 'subsecond')?.width ?? 0;
  const moment = momentOf(value, system, Math.min(digits, 3));
  if (!moment) return NOT_A_DATE;
  const twelve = dates.some((token) => token.part === 'ampm');
  let out = '';
  for (const token of tokens) {
    if (token.kind === 'date') out += part(token, moment, twelve, locale, language);
    else if (token.kind === 'literal') out += token.text;
    else if (token.kind === 'point') out += '.';
    else if (token.kind === 'slash') out += '/';
    else if (token.kind === 'percent') out += '%';
    else if (token.kind === 'comma') out += ',';
    else if (token.kind === 'digit' && token.char === '0') out += '0';
  }
  return out;
}

/**
 * The value of an ISO date written in a cell (`t="d"`), as a number of days
 * of the system; undefined when it is not one.
 */
export function serialOf(iso: string, system: DateSystem): number | undefined {
  const match =
    /^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2})(?::(\d{2})(\.\d+)?)?)?(Z|[+-]\d{2}:\d{2})?$/.exec(
      iso.trim(),
    );
  if (!match) return undefined;
  const [, y, mo, d, h = '0', mi = '0', s = '0', fraction = ''] = match;
  const time = Date.UTC(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s));
  if (!Number.isFinite(time)) return undefined;
  let days = (time - START[system]) / MS_PER_DAY + Number(`0${fraction}`) / 86_400;
  // Days from 1 March 1900 are counted one on, past the day that never was.
  if (system === 1900 && days >= 60) days += 1;
  return days < 0 ? undefined : days;
}
