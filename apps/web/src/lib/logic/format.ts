/**
 * Text for dates, durations, numbers and sizes.
 *
 * Nothing here reads the language, the time zone or the clock of the device:
 * the caller passes them, from the reader's settings. A moment is a number
 * of milliseconds since the epoch.
 */
import { dateOfInstant, startOfDay, type CalendarDate } from './dates.ts';

/** How the hours of a time are written: `h12` is 3:20 PM, `h23` is 15:20. */
export type HourCycle = 'h12' | 'h23';

/** What a function needs in order to turn a value into text for a person. */
export interface DisplayContext {
  /** BCP 47 tag, such as `ko` or `en-US`. */
  locale: string;
  /** The reader's time zone, an IANA name such as `Asia/Seoul`. */
  timeZone: string;
  /** The current moment; defaults to the clock. Decides whether a date shows its year. */
  now?: number;
  /** How hours are written; defaults to the habit of the locale. */
  hourCycle?: HourCycle;
  /** Shown for a select value whose option no longer exists. */
  unknownOption?: string;
  /** The title of a card of the board; undefined for a card that is not there (deleted). */
  cardTitle?: (cardId: string) => string | undefined;
  /** The name a person is shown by, from the identifier of the user; a CSV names people by it. */
  personName?: (userId: string) => string;
}

const isValid = (instant: number): boolean => Number.isFinite(instant);

interface YearRange {
  year: number;
  from: number;
  until: number;
}

const currentYears = new Map<string, YearRange>();
const dateFormatters = new Map<string, Intl.DateTimeFormat>();

/** One formatter per language, zone and shape: card lists format thousands of dates. */
function dateFormatter(
  locale: string,
  timeZone: string,
  shape: string,
  options: Intl.DateTimeFormatOptions,
): Intl.DateTimeFormat {
  const key = `${locale}\u0000${timeZone}\u0000${shape}`;
  let formatter = dateFormatters.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { timeZone, ...options });
    dateFormatters.set(key, formatter);
  }
  return formatter;
}

/** The reader's current year and its millisecond bounds in their time zone. */
function currentYearRange(context: DisplayContext): YearRange {
  const now = context.now ?? Date.now();
  const cached = currentYears.get(context.timeZone);
  if (cached && now >= cached.from && now < cached.until) return cached;
  const year = Number(dateOfInstant(now, context.timeZone).slice(0, 4));
  if (year <= 1 || year >= 9999) return { year, from: now, until: now };
  const range: YearRange = {
    year,
    from: startOfDay(`${String(year).padStart(4, '0')}-01-01` as CalendarDate, context.timeZone),
    until: startOfDay(
      `${String(year + 1).padStart(4, '0')}-01-01` as CalendarDate,
      context.timeZone,
    ),
  };
  currentYears.set(context.timeZone, range);
  return range;
}

/** The reader's current year: dates in it are shown without the year. */
function thisYear(context: DisplayContext): number {
  return currentYearRange(context).year;
}

function yearOption(instant: number, context: DisplayContext): 'numeric' | undefined {
  const current = currentYearRange(context);
  if (instant >= current.from && instant < current.until) return undefined;
  if (current.from < current.until) return 'numeric';
  const year = Number(dateOfInstant(instant, context.timeZone).slice(0, 4));
  return year === current.year ? undefined : 'numeric';
}

/** The day of a moment for the reader: `July 09`, or `July 09, 2025` in another year. */
export function displayDate(instant: number, context: DisplayContext): string {
  if (!isValid(instant)) return '';
  const year = yearOption(instant, context);
  return dateFormatter(context.locale, context.timeZone, year ? 'date-y' : 'date', {
    year,
    month: 'long',
    day: '2-digit',
  }).format(instant);
}

/**
 * A calendar date as text: `July 09`, with the year unless it is the
 * reader's current year. A calendar date has no time zone, so the day shown
 * is the same for every reader.
 */
export function displayCalendarDate(date: CalendarDate, context: DisplayContext): string {
  const year = Number(date.slice(0, 4));
  const withYear = year !== thisYear(context);
  return dateFormatter(context.locale, 'UTC', withYear ? 'date-y' : 'date', {
    year: withYear ? 'numeric' : undefined,
    month: 'long',
    day: '2-digit',
  }).format(Date.UTC(year, Number(date.slice(5, 7)) - 1, Number(date.slice(8, 10))));
}

/** The short all-digits form a person types: `07/09/2026` in the United States. */
export function inputDate(instant: number, context: DisplayContext): string {
  if (!isValid(instant)) return '';
  return dateFormatter(context.locale, context.timeZone, 'input-date', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(instant);
}

/** A moment on the reader's clock: `July 09 at 3:20 PM`, with the year in another year. */
export function displayDateTime(instant: number, context: DisplayContext): string {
  if (!isValid(instant)) return '';
  const year = yearOption(instant, context);
  const cycle = context.hourCycle ?? '';
  return dateFormatter(
    context.locale,
    context.timeZone,
    `datetime\u0000${year ? 'y' : ''}\u0000${cycle}`,
    {
      year,
      month: 'long',
      day: '2-digit',
      hour: 'numeric',
      minute: 'numeric',
      ...(context.hourCycle ? { hourCycle: context.hourCycle } : {}),
    },
  ).format(instant);
}

type CoarseUnit = 'second' | 'minute' | 'hour' | 'day' | 'month' | 'year';

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * A length of time as one rounded number and one unit, the way people say
 * it: 50 minutes is "an hour", 25 hours is "a day". Under 45 seconds the
 * count is 0 seconds.
 *
 * Each unit is rounded first and the rounded number decides: up to 44
 * seconds, 44 minutes, 21 hours, 25 days and 10 months stay in their unit.
 */
function coarse(milliseconds: number): [number, CoarseUnit] {
  const length = Math.abs(milliseconds);
  const seconds = Math.round(length / SECOND);
  const minutes = Math.round(length / MINUTE);
  const hours = Math.round(length / HOUR);
  const days = Math.round(length / DAY);
  // 4800 months are 146097 days: the average month of the Gregorian calendar.
  const exactMonths = ((length / DAY) * 4800) / 146097;
  const months = Math.round(exactMonths);
  const years = Math.round(exactMonths / 12);
  if (seconds < 45) return [0, 'second'];
  if (minutes <= 1) return [1, 'minute'];
  if (minutes < 45) return [minutes, 'minute'];
  if (hours <= 1) return [1, 'hour'];
  if (hours < 22) return [hours, 'hour'];
  if (days <= 1) return [1, 'day'];
  if (days < 26) return [days, 'day'];
  if (months <= 1) return [1, 'month'];
  if (months < 11) return [months, 'month'];
  if (years <= 1) return [1, 'year'];
  return [years, 'year'];
}

/** `3 hours ago`, `in 2 days`, `now`: how far a moment is from the current one. */
export function relativeTime(instant: number, locale: string, now: number = Date.now()): string {
  const difference = instant - now;
  const [count, unit] = coarse(difference);
  return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(
    difference < 0 ? -count : count,
    unit,
  );
}

/** `3 hours`, `2 months`: how long a span of time is. */
export function humanizeDuration(milliseconds: number, locale: string): string {
  const [count, unit] = coarse(milliseconds);
  // Under 45 seconds the exact count is more useful than "0 seconds".
  const value = unit === 'second' ? Math.round(Math.abs(milliseconds) / SECOND) : count;
  return new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'long' }).format(value);
}

/** Rounds to a number of decimal places. */
export function roundTo(value: number, decimalPlaces: number): number {
  const factor = 10 ** decimalPlaces;
  return Math.round(value * factor) / factor;
}

// The size of a file lives on its own, for the read-only view of a body, which imports no dates.
export { humanFileSize } from './file-size.ts';

// ------------------------------------------------------- a calendar's names

/*
 * The names a calendar writes: of a month, of a day, of a weekday. None of
 * them is a word of ours: `Intl` gives the names and the order of the parts
 * in the reader's language. A calendar date is formatted as that date in UTC:
 * it is a day, not a moment, and must not move with anyone's time zone.
 */

const calendarFormatters = new Map<string, Intl.DateTimeFormat>();

/** One formatter per language and shape: a month draws hundreds of names. */
function calendarFormatter(
  locale: string,
  shape: string,
  options: Intl.DateTimeFormatOptions,
): Intl.DateTimeFormat {
  const key = `${locale}\u0000${shape}`;
  let formatter = calendarFormatters.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { timeZone: 'UTC', ...options });
    calendarFormatters.set(key, formatter);
  }
  return formatter;
}

/** Midnight UTC of a date, the years 0 to 99 too (`Date.UTC` would read them as 1900 to 1999). */
function utcDay(date: CalendarDate): Date {
  const day = new Date(0);
  day.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  return day;
}

/** `November 2026`: the month a date is in. */
export function displayMonth(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'month', { year: 'numeric', month: 'long' }).format(
    utcDay(date),
  );
}

/** `November 12`. */
export function displayDay(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'day', { month: 'long', day: 'numeric' }).format(utcDay(date));
}

/** `Thursday, November 12`. */
export function displayDayLong(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'day-long', {
    month: 'long',
    day: 'numeric',
    weekday: 'long',
  }).format(utcDay(date));
}

/** `November 8 – 14`: from one day to another, the parts they share written once. */
export function displayDayRange(first: CalendarDate, last: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'day', { month: 'long', day: 'numeric' }).formatRange(
    utcDay(first),
    utcDay(last),
  );
}

/** `2026`: the year of a date, with the word for "year" some languages add. */
export function displayYear(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'year', { year: 'numeric' }).format(utcDay(date));
}

/** `Saturday, November 21, 2026`. */
export function displayDayWithYear(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'day-year', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
    weekday: 'long',
  }).format(utcDay(date));
}

/** `Thu`: the short name of the weekday of a date. */
export function displayWeekday(date: CalendarDate, locale: string): string {
  return calendarFormatter(locale, 'weekday', { weekday: 'short' }).format(utcDay(date));
}

/** `12`: the number of a day in its month, without the word for "day" some languages add. */
export function displayDayNumber(date: CalendarDate, locale: string): string {
  return (
    calendarFormatter(locale, 'number', { day: 'numeric' })
      .formatToParts(utcDay(date))
      .find((part) => part.type === 'day')?.value ?? ''
  );
}

/** `8:00 AM`: the time of a moment on the reader's clock. */
export function displayTime(instant: number, context: DisplayContext): string {
  if (!isValid(instant)) return '';
  const cycle = context.hourCycle ?? '';
  return calendarFormatter(context.locale, `time\u0000${context.timeZone}\u0000${cycle}`, {
    timeZone: context.timeZone,
    hour: 'numeric',
    minute: '2-digit',
    ...(context.hourCycle ? { hourCycle: context.hourCycle } : {}),
  }).format(instant);
}
