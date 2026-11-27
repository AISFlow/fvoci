/**
 * Calendar dates, and the value of a date property.
 *
 * A date without a time of day is a day on the calendar, the same day for
 * every reader whatever their time zone: "due on the 7th" is the 7th in
 * Seoul and in Lisbon. It is not a moment. A moment (when a card was created,
 * or a date that carries a time) falls on different days for different
 * readers, and "its day" always means the day in the reader's time zone.
 *
 * The reader's time zone is always an argument: an IANA name such as
 * `Asia/Seoul`, taken from the reader's settings. Nothing in the logic asks
 * the device what time zone it is in, and nothing uses the parts of `Date`
 * that answer in device time (`getHours`, `new Date(year, month, day)` and
 * the like); a lint rule keeps it that way. So a result never depends on
 * where the code happens to run.
 *
 * Nothing here knows how a date is stored either; wire.ts converts.
 */
import { CalendarDate as ZonedCalendarDate, toZoned } from '@internationalized/date';

declare const calendarDateBrand: unique symbol;

/** A day on the calendar, written `YYYY-MM-DD`. Two of them compare as text. */
export type CalendarDate = string & { readonly [calendarDateBrand]: true };

const DAY = 24 * 60 * 60 * 1000;
const pad = (value: number, width: number): string => String(value).padStart(width, '0');

/** The date with these numbers; `month` is 1 to 12. Numbers out of range roll over, as in `Date`. */
export function calendarDate(year: number, month: number, day: number): CalendarDate {
  return dateOfUtc(utcMidnight(year, month, day));
}

/** Midnight UTC of a day. `Date.UTC` would read the years 0 to 99 as 1900 to 1999. */
function utcMidnight(year: number, month: number, day: number): number {
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  return date.getTime();
}

/**
 * Reads `YYYY-MM-DD`, from the year 0001 to 9999; undefined for anything
 * else, and for days that do not exist.
 */
export function parseCalendarDate(text: string): CalendarDate | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (!match || match[1] === '0000') return undefined;
  const date = calendarDate(Number(match[1]), Number(match[2]), Number(match[3]));
  return date === text ? date : undefined;
}

/** The UTC day a moment falls on. */
export function dateOfUtc(instant: number): CalendarDate {
  const date = new Date(instant);
  return `${pad(date.getUTCFullYear(), 4)}-${pad(date.getUTCMonth() + 1, 2)}-${pad(date.getUTCDate(), 2)}` as CalendarDate;
}

/** Midnight UTC at the start of a date: a number to do arithmetic with, not a moment anyone lives. */
function utcOf(date: CalendarDate): number {
  return utcMidnight(Number(date.slice(0, 4)), Number(date.slice(5, 7)), Number(date.slice(8, 10)));
}

const formatters = new Map<string, Intl.DateTimeFormat>();

/** The first and the last day a date can be: the years 1 to 9999, as the server has them. */
const FIRST_DATE = '0001-01-01' as CalendarDate;
const LAST_DATE = '9999-12-31' as CalendarDate;

/**
 * The day a moment falls on for a reader in `timeZone`. A moment of the
 * first or the last day there is (in UTC) may be in the year 0 or 10000 on
 * the reader's clock, by a few hours: it is on that first or last day, for
 * a day of five digits would sort before every other as text, and the year
 * before the year 1 is written "1" with another era.
 */
export function dateOfInstant(instant: number, timeZone: string): CalendarDate {
  let formatter = formatters.get(timeZone);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat('en-u-ca-gregory-nu-latn', {
      timeZone,
      era: 'short',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    });
    formatters.set(timeZone, formatter);
  }
  const parts: Record<string, string> = {};
  for (const part of formatter.formatToParts(instant)) parts[part.type] = part.value;
  const year = Number(parts.year);
  if (parts.era === 'BC' || year < 1) return FIRST_DATE;
  if (year > 9999) return LAST_DATE;
  return calendarDate(year, Number(parts.month), Number(parts.day));
}

/**
 * The moment a day begins for a reader in `timeZone`: midnight there, or the
 * first moment after it on a day whose midnight the clocks skip. Days are not
 * all 24 hours long: where clocks change, one is 23 hours and one 25.
 */
export function startOfDay(date: CalendarDate, timeZone: string): number {
  const day = new ZonedCalendarDate(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)),
    Number(date.slice(8, 10)),
  );
  return toZoned(day, timeZone).toDate().getTime();
}

/** The first moment that is no longer part of a day for a reader in `timeZone`. */
export function endOfDay(date: CalendarDate, timeZone: string): number {
  return startOfDay(addDays(date, 1), timeZone);
}

export function addDays(date: CalendarDate, days: number): CalendarDate {
  return dateOfUtc(utcOf(date) + days * DAY);
}

/** How many days `later` is after `earlier`; negative when it is before. */
export function daysBetween(earlier: CalendarDate, later: CalendarDate): number {
  return Math.round((utcOf(later) - utcOf(earlier)) / DAY);
}

/** One end of a date value: a day, or a moment when the value carries a time of day. */
export type DatePoint =
  { date: CalendarDate; instant?: undefined } | { instant: number; date?: undefined };

/** The value of a date property: a day or a moment, or a range from one to another. */
export interface DateValue {
  from?: DatePoint;
  /** The last day or moment of a range, itself included. */
  to?: DatePoint;
}

/** The day of one end of a value, for the reader. */
export function dateOfPoint(point: DatePoint, timeZone: string): CalendarDate {
  return point.date ?? dateOfInstant(point.instant, timeZone);
}

/** The first and the last day a value covers; the same day twice when it is not a range. */
export function daysOf(
  value: DateValue,
  timeZone: string,
): { first: CalendarDate; last: CalendarDate } | undefined {
  if (!value.from) return undefined;
  const first = dateOfPoint(value.from, timeZone);
  const last = value.to ? dateOfPoint(value.to, timeZone) : first;
  // A range stored backwards still covers the same days.
  return first <= last ? { first, last } : { first: last, last: first };
}

/**
 * Orders two ends of values: by the day each falls on for the reader; on the
 * same day one without a time comes before one with a time, and two moments
 * keep their order.
 */
export function comparePoints(first: DatePoint, second: DatePoint, timeZone: string): number {
  const firstDay = dateOfPoint(first, timeZone);
  const secondDay = dateOfPoint(second, timeZone);
  if (firstDay !== secondDay) return firstDay < secondDay ? -1 : 1;
  if (first.instant === undefined || second.instant === undefined) {
    return Number(first.instant !== undefined) - Number(second.instant !== undefined);
  }
  return Math.sign(first.instant - second.instant);
}
