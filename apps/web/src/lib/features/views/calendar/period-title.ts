/**
 * What the calendar's row calls what it shows: the month, or the days of the
 * week (the month's name short and the year only out of this one, to fit a
 * narrow pane), in the reader's language.
 */
import { firstOf, type Month } from '$lib/logic/calendar.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import { displayMonth } from '$lib/logic/format.ts';

import type { Scale } from './period.svelte.ts';

const rangeFormatters = new Map<string, Intl.DateTimeFormat>();

/** Noon of a calendar day in UTC, for a formatter that reads it in UTC. `Date.UTC` would read the years 0 to 99 as 1900 to 1999. */
function utcNoon(date: CalendarDate): Date {
  const day = new Date(0);
  day.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  day.setUTCHours(12);
  return day;
}

/**
 * What the calendar's row calls what it shows: "November 2026", or a week,
 * "Nov 8 – 14" in the year of `today` and "Dec 27, 2026 – Jan 2, 2027" out of
 * it. The year is left out where it goes without saying: a narrow pane has
 * room for little more than the days.
 */
export function periodTitle(
  scale: Scale,
  weeks: readonly (readonly CalendarDate[])[],
  month: Month,
  locale: string,
  today: CalendarDate,
): string {
  const week = weeks[0];
  const first = week?.[0];
  const last = week?.at(-1);
  if (scale === 'month' || !first || !last) return displayMonth(firstOf(month), locale);
  const year = today.slice(0, 4);
  const withYear = first.slice(0, 4) !== year || last.slice(0, 4) !== year;
  const key = `${locale}|${String(withYear)}`;
  let format = rangeFormatters.get(key);
  if (!format) {
    format = new Intl.DateTimeFormat(locale, {
      timeZone: 'UTC',
      calendar: 'gregory',
      ...(withYear ? { year: 'numeric' } : {}),
      // Short, so that the week fits the row of a narrow pane.
      month: 'short',
      day: 'numeric',
    });
    rangeFormatters.set(key, format);
  }
  return format.formatRange(utcNoon(first), utcNoon(last));
}
