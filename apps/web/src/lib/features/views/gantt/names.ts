/**
 * The words on the axis of a Gantt chart, and the days in the name of a
 * bar. None of them is a word of ours (but for what joins two days, which
 * the caller gives): `Intl` gives the names in the reader's language. A
 * calendar date is
 * formatted as that date in UTC: it is a day, not a moment, and must not
 * move with anyone's time zone.
 */
import type { CalendarDate } from '$lib/logic/dates.ts';
import { displayDayNumber, displayMonth } from '$lib/logic/format.ts';
import type { GanttZoom } from '$lib/logic/view-members.ts';

const formatters = new Map<string, Intl.DateTimeFormat>();

const SHAPES = {
  year: { year: 'numeric' },
  month: { month: 'short' },
  day: { month: 'long', day: 'numeric' },
  dayOfYear: { year: 'numeric', month: 'long', day: 'numeric' },
} as const satisfies Record<string, Intl.DateTimeFormatOptions>;

function format(date: CalendarDate, locale: string, shape: keyof typeof SHAPES): string {
  const key = `${locale}\u0000${shape}`;
  let formatter = formatters.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { timeZone: 'UTC', ...SHAPES[shape] });
    formatters.set(key, formatter);
  }
  // `Date.UTC` would read the years 0 to 99 as 1900 to 1999.
  const day = new Date(0);
  day.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  return formatter.format(day);
}

/** The upper row: `November 2026`, or `2026` where the lower row is months. */
export function upperLabel(zoom: GanttZoom, date: CalendarDate, locale: string): string {
  return zoom === 'month' ? format(date, locale, 'year') : displayMonth(date, locale);
}

/** The lower row: the number of a day (of the first day of a week), or `Nov`. */
export function lowerLabel(zoom: GanttZoom, date: CalendarDate, locale: string): string {
  return zoom === 'month' ? format(date, locale, 'month') : displayDayNumber(date, locale);
}

/**
 * The days of a bar in words: `November 12`, or the first and the last day
 * of several. A chart reaches over years, so a day outside `year` (the
 * reader's current one) is written with its year, and then both ends are.
 *
 * Each end is formatted by itself and `range` joins them: formatting a
 * range in one go is a hundred times slower, and a chart names thousands.
 */
export function daysLabel(
  days: { first: CalendarDate; last: CalendarDate },
  locale: string,
  year: number,
  range: (first: string, last: string) => string,
): string {
  const inYear = (date: CalendarDate): boolean => Number(date.slice(0, 4)) === year;
  const shape = inYear(days.first) && inYear(days.last) ? 'day' : 'dayOfYear';
  const first = format(days.first, locale, shape);
  return days.first === days.last ? first : range(first, format(days.last, locale, shape));
}
