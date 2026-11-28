/**
 * The relative periods a date condition `isWithin` compares with: a closed
 * list of tokens a view keeps (`thisWeek`, …), and the days each one covers
 * for a reader on a day. The days are worked out every time, from the
 * reader's today and the day their week starts on, so that "this week" is
 * the reader's week wherever and whenever they look.
 */
import { firstOf, lastOf, monthOf, weekOf } from './calendar.ts';
import { addDays, type CalendarDate } from './dates.ts';
import type { Weekday } from './regional.ts';

/** The periods, in the order a list of them is shown. The server accepts the same eight. */
export const FILTER_PERIODS = [
  'thisWeek',
  'nextWeek',
  'lastWeek',
  'thisMonth',
  'next7Days',
  'next30Days',
  'last7Days',
  'last30Days',
] as const;

export type FilterPeriod = (typeof FILTER_PERIODS)[number];

export function isFilterPeriod(value: string | undefined): value is FilterPeriod {
  return FILTER_PERIODS.some((period) => period === value);
}

/** The first and the last day of a span of days, both included. */
export interface DaySpan {
  first: CalendarDate;
  last: CalendarDate;
}

/**
 * The days `period` covers for a reader whose today is `today` and whose
 * week starts on `weekStart`.
 *
 * A week is the seven days of the reader's calendar week; a month the days
 * of today's month. "Next n days" and "last n days" are n days that include
 * today: today and the n − 1 days after it, or the n − 1 days before it and
 * today.
 */
export function daysOfPeriod(
  period: FilterPeriod,
  today: CalendarDate,
  weekStart: Weekday,
): DaySpan {
  const week = (shift: number): DaySpan => {
    const days = weekOf(addDays(today, shift * 7), weekStart);
    return { first: days[0] ?? today, last: days[6] ?? today };
  };
  switch (period) {
    case 'thisWeek':
      return week(0);
    case 'nextWeek':
      return week(1);
    case 'lastWeek':
      return week(-1);
    case 'thisMonth':
      return { first: firstOf(monthOf(today)), last: lastOf(monthOf(today)) };
    case 'next7Days':
      return { first: today, last: addDays(today, 6) };
    case 'next30Days':
      return { first: today, last: addDays(today, 29) };
    case 'last7Days':
      return { first: addDays(today, -6), last: today };
    case 'last30Days':
      return { first: addDays(today, -29), last: today };
  }
}

/** Whether two spans of days have a day in common. */
export function spansMeet(one: DaySpan, other: DaySpan): boolean {
  return one.first <= other.last && other.first <= one.last;
}
