/**
 * The days of a diary as its pages have them. A page is a day of the year
 * without its year (`MonthDay`, `11-21`), and holds that day of every year
 * shown: the latest on top, the years before it below, nearest first. A page
 * of `02-29` holds the leap years alone.
 *
 * Today is always an argument: the day of the reader's device in their time
 * zone (`features/diary/today.svelte.ts`). Nothing here reads a clock.
 */
import { addDays, parseCalendarDate, type CalendarDate } from './dates.ts';

declare const monthDayBrand: unique symbol;

/** A day of the year, written `MM-DD`; `02-29` is one of them. */
export type MonthDay = string & { readonly [monthDayBrand]: true };

/** A leap year, for stepping through the days of a year that has `02-29`. */
const LEAP_YEAR = '2000';

/** The first year a diary's page can show: the server takes the years 1 to 9999. */
export const FIRST_YEAR = 1;

/** Reads `MM-DD`, `02-29` included; undefined for anything else. */
export function parseMonthDay(text: string): MonthDay | undefined {
  return /^\d{2}-\d{2}$/.test(text) && parseCalendarDate(`${LEAP_YEAR}-${text}`)
    ? (text as MonthDay)
    : undefined;
}

/** The day in a leap year, where a month and a day alone are to be named. */
export function inLeapYear(monthDay: MonthDay): CalendarDate {
  return `${LEAP_YEAR}-${monthDay}` as CalendarDate;
}

export function monthDayOf(date: CalendarDate): MonthDay {
  return date.slice(5) as MonthDay;
}

export function yearOf(date: CalendarDate): number {
  return Number(date.slice(0, 4));
}

/** The day of the year after (`1`) or before (`-1`), through `02-29` and round the end of the year. */
export function stepMonthDay(monthDay: MonthDay, by: 1 | -1): MonthDay {
  return monthDayOf(addDays(`${LEAP_YEAR}-${monthDay}` as CalendarDate, by));
}

/** The day of the year in `year`; undefined for `02-29` of a year that has none, and out of the years 1 to 9999. */
export function dateIn(year: number, monthDay: MonthDay): CalendarDate | undefined {
  if (!Number.isInteger(year) || year < FIRST_YEAR || year > 9999) return undefined;
  return parseCalendarDate(`${String(year).padStart(4, '0')}-${monthDay}`);
}

/**
 * The days a page shows, the latest first: from this year (today's), or a
 * later year written on (on a device in a time zone ahead), down to `from`
 * and to any earlier year written on, each year the day is in. This year's
 * comes first even before it is there (a day later in the year than today):
 * it is shown, not written in (`writable`).
 */
export function pageDays(
  monthDay: MonthDay,
  today: CalendarDate,
  from: number,
  written: readonly CalendarDate[],
): CalendarDate[] {
  const years = written.map(yearOf);
  const last = Math.max(yearOf(today), ...years);
  const first = Math.max(FIRST_YEAR, Math.min(from, yearOf(today), ...years));
  const days: CalendarDate[] = [];
  for (let year = last; year >= first; year -= 1) {
    const date = dateIn(year, monthDay);
    if (date) days.push(date);
  }
  return days;
}

/** A day is written in from the day it is: none after today. */
export function writable(date: CalendarDate, today: CalendarDate): boolean {
  return date <= today;
}
