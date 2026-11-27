/**
 * What a person does to the value of a date property while editing it: pick
 * a day, type a date, set a time of day, turn the time or the end of a range
 * on and off.
 *
 * A value without a time is a day on the calendar and no time zone is
 * involved in changing it. A value with a time is a moment, and everything a
 * person reads or enters for it (its day, its hour and minute) is on the
 * clock of the reader's time zone, which is always an argument here.
 *
 * Clocks that change make two kinds of time a person can ask for but that do
 * not name exactly one moment:
 *
 * - A time the clocks skip (02:30 on the night they go forward). The moment
 *   taken is the one that far past the last moment before the skip, so 02:30
 *   becomes 03:30. Reading the value back shows 03:30: what is stored is what
 *   is shown.
 * - A time that happens twice (01:30 on the night the clocks go back). The
 *   first of the two is taken. A value that already is the second one stays
 *   the second one while only its day, hour or minute is changed to another
 *   time that also happens twice.
 */
import { CalendarDateTime, fromAbsolute, toZoned } from '@internationalized/date';

import {
  dateOfInstant,
  parseCalendarDate,
  type CalendarDate,
  type DatePoint,
  type DateValue,
} from './dates.ts';
import type { HourCycle } from './format.ts';

/** What a clock and a calendar show at one moment, somewhere. */
export interface WallTime {
  date: CalendarDate;
  /** 0 to 23. */
  hour: number;
  /** 0 to 59. */
  minute: number;
}

/** Which of the two moments a repeated time means. */
export type Occurrence = 'earlier' | 'later';

/** What the clocks of `timeZone` show at a moment. */
export function wallTimeOf(instant: number, timeZone: string): WallTime {
  const zoned = fromAbsolute(instant, timeZone);
  return { date: dateOfInstant(instant, timeZone), hour: zoned.hour, minute: zoned.minute };
}

/**
 * The moment the clocks of `timeZone` show a date and time, to the minute.
 * See the top of the file for times the clocks skip or show twice.
 */
export function instantOfWallTime(
  wall: WallTime,
  timeZone: string,
  occurrence: Occurrence = 'earlier',
): number {
  const plain = new CalendarDateTime(
    Number(wall.date.slice(0, 4)),
    Number(wall.date.slice(5, 7)),
    Number(wall.date.slice(8, 10)),
    wall.hour,
    wall.minute,
  );
  // `compatible` takes the first of two moments; both ways take the later
  // reading of a time the clocks skip.
  return toZoned(plain, timeZone, occurrence === 'later' ? 'later' : 'compatible')
    .toDate()
    .getTime();
}

/** Whether a moment is the first or the second time its clock reading happens. */
export function occurrenceOf(instant: number, timeZone: string): Occurrence {
  const wall = wallTimeOf(instant, timeZone);
  const first = instantOfWallTime(wall, timeZone, 'earlier');
  const second = instantOfWallTime(wall, timeZone, 'later');
  return first !== second && instant >= second ? 'later' : 'earlier';
}

/** One end of a value moved to another day; a moment keeps its time of day. */
export function withDay(point: DatePoint, day: CalendarDate, timeZone: string): DatePoint {
  if (point.instant === undefined) return { date: day };
  const wall = wallTimeOf(point.instant, timeZone);
  return {
    instant: instantOfWallTime(
      { ...wall, date: day },
      timeZone,
      occurrenceOf(point.instant, timeZone),
    ),
  };
}

/** A moment moved to another time of the same day on the reader's clock. */
export function withTimeOfDay(
  instant: number,
  hour: number,
  minute: number,
  timeZone: string,
): number {
  const { date } = wallTimeOf(instant, timeZone);
  return instantOfWallTime({ date, hour, minute }, timeZone, occurrenceOf(instant, timeZone));
}

/** The time of day a value gets when a time is first added to it: nine in the morning. */
export const DEFAULT_HOUR = 9;

/**
 * The value with a time of day on each end. A day becomes nine in the
 * morning of that day on the reader's clock; the end of a range that ends on
 * the day it starts becomes an hour later, so that the range is not empty.
 */
export function withTime(value: DateValue, timeZone: string): DateValue {
  const { from, to } = value;
  if (!from) return {};
  const start =
    from.instant ?? instantOfWallTime({ date: from.date, hour: DEFAULT_HOUR, minute: 0 }, timeZone);
  if (!to) return { from: { instant: start } };
  const end =
    to.instant ??
    instantOfWallTime(
      {
        date: to.date,
        hour: to.date === dateOfInstant(start, timeZone) ? DEFAULT_HOUR + 1 : DEFAULT_HOUR,
        minute: 0,
      },
      timeZone,
    );
  return { from: { instant: start }, to: { instant: Math.max(start, end) } };
}

/** The value without its times: each end becomes the day it falls on for the reader. */
export function withoutTime(value: DateValue, timeZone: string): DateValue {
  const day = (point: DatePoint): DatePoint => ({
    date: point.date ?? dateOfInstant(point.instant, timeZone),
  });
  const { from, to } = value;
  if (!from) return {};
  return to ? { from: day(from), to: day(to) } : { from: day(from) };
}

/** Whether the value carries a time of day. */
export function hasTime(value: DateValue): boolean {
  return value.from?.instant !== undefined;
}

const position = (point: DatePoint): string | number => point.instant ?? point.date;

/**
 * Keeps a range from running backwards after one of its ends was edited: the
 * end the person just set stays where they put it and the other end is pulled
 * to it. A value whose ends are of two kinds keeps only its start.
 */
export function orderRange(value: DateValue, edited: 'from' | 'to'): DateValue {
  const { from, to } = value;
  if (!from || !to) return value;
  if ((from.instant === undefined) !== (to.instant === undefined)) return { from };
  if (position(from) <= position(to)) return value;
  return edited === 'from' ? { from, to: from } : { from: to, to };
}

/** The order in which a language writes the three numbers of a date. */
type DatePart = 'year' | 'month' | 'day';

function dateOrder(locale: string): readonly [DatePart, DatePart, DatePart] {
  const parts = new Intl.DateTimeFormat(locale, {
    timeZone: 'UTC',
    calendar: 'gregory',
    numberingSystem: 'latn',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).formatToParts(0);
  const [first, second, third, ...more] = parts
    .map((part) => part.type)
    .filter((type): type is DatePart => type === 'year' || type === 'month' || type === 'day');
  return first && second && third && more.length === 0
    ? [first, second, third]
    : ['year', 'month', 'day'];
}

/**
 * The short all-digits form of a calendar date that a person types:
 * `03/05/2026` in the United States, `05/03/2026` in Spain, `2026. 03. 05.`
 * in Korea.
 */
export function typedDateText(date: CalendarDate, locale: string): string {
  // `Date.UTC` would read the years 0 to 99 as 1900 to 1999.
  const noon = new Date(0);
  noon.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  return new Intl.DateTimeFormat(locale, {
    timeZone: 'UTC',
    calendar: 'gregory',
    numberingSystem: 'latn',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(noon.getTime());
}

/**
 * Reads a date a person typed. The numbers are taken in the order the
 * language writes them, whatever stands between them (`3/5/2026`, `3.5.2026`,
 * `2026. 3. 5.`); a text that starts with a four-digit year is read as year,
 * month, day in every language. Undefined when it is not a day that exists.
 */
export function parseTypedDate(text: string, locale: string): CalendarDate | undefined {
  const numbers = text.match(/\d+/g);
  if (numbers?.length !== 3) return undefined;
  const [first = '', second = '', third = ''] = numbers;
  const order = first.length === 4 ? (['year', 'month', 'day'] as const) : dateOrder(locale);
  const read = { year: '', month: '', day: '' };
  read[order[0]] = first;
  read[order[1]] = second;
  read[order[2]] = third;
  if (read.year.length !== 4 || read.month.length > 2 || read.day.length > 2) return undefined;
  return parseCalendarDate(
    `${read.year}-${read.month.padStart(2, '0')}-${read.day.padStart(2, '0')}`,
  );
}

/**
 * What a language calls the two halves of the day on a twelve-hour clock,
 * and whether it says so before the hour (`오후 3:20`) or after (`3:20 PM`).
 */
export function dayPeriods(locale: string): { am: string; pm: string; first: boolean } {
  const format = new Intl.DateTimeFormat(locale, {
    timeZone: 'UTC',
    hour: 'numeric',
    minute: '2-digit',
    hourCycle: 'h12',
  });
  const parts = (hour: number): Intl.DateTimeFormatPart[] =>
    format.formatToParts(Date.UTC(2000, 0, 1, hour));
  const name = (hour: number, fallback: string): string =>
    parts(hour).find((part) => part.type === 'dayPeriod')?.value ?? fallback;
  const types = parts(15).map((part) => part.type);
  return {
    am: name(9, 'AM'),
    pm: name(15, 'PM'),
    first: types.includes('dayPeriod') && types.indexOf('dayPeriod') < types.indexOf('hour'),
  };
}

/** The hours a person can choose from, as a clock of this kind numbers them. */
export function hourChoices(hourCycle: HourCycle): number[] {
  return hourCycle === 'h12'
    ? [12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
    : Array.from({ length: 24 }, (_, hour) => hour);
}

/** An hour of the day as a twelve-hour clock shows it. */
export function toTwelveHour(hour: number): { hour: number; period: 'am' | 'pm' } {
  return { hour: hour % 12 === 0 ? 12 : hour % 12, period: hour < 12 ? 'am' : 'pm' };
}

/** The hour of the day for a reading of a twelve-hour clock. */
export function fromTwelveHour(hour: number, period: 'am' | 'pm'): number {
  return (hour % 12) + (period === 'pm' ? 12 : 0);
}
