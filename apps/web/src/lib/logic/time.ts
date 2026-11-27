/**
 * Pure time-tracking calculations: period boundaries (inclusive `[from, to]`),
 * duration formatting, date-time input conversion, and live running-timer
 * summary totals.
 *
 * Every calendar and clock calculation takes an explicit IANA `timeZone`
 * argument — never reading the device clock's time zone (`fvoci/no-device-time`).
 */
import { CalendarDateTime, toZoned } from '@internationalized/date';

import {
  addDays,
  calendarDate,
  dateOfInstant,
  endOfDay,
  startOfDay,
  type CalendarDate,
} from './dates.ts';
import type { HourCycle } from './format.ts';
import type { Weekday } from './regional.ts';

/** Allowed activity categories for a time entry, in display order. */
export const TIME_ENTRY_KINDS = ['lecture', 'study', 'work', 'other'] as const;

export type TimeEntryKind = (typeof TIME_ENTRY_KINDS)[number];

/** Maximum Unicode scalar length of a time entry note (matches server). */
export const TIME_NOTE_MAX_CHARS = 1000;

/** Maximum valid epoch millisecond timestamp accepted by the server. */
export const TIME_MAX_TIMESTAMP = 8_640_000_000_000_000;

/** Period granularity for the personal "My time" summary screen. */
export type TimePeriod = 'day' | 'week' | 'month';

export interface TimePeriodBounds {
  /** Inclusive start timestamp (epoch ms). */
  from: number;
  /** Inclusive end timestamp (epoch ms): `startOfNextPeriod - 1`. */
  to: number;
  firstDay: CalendarDate;
  lastDay: CalendarDate;
}

export interface DurationParts {
  hours: number;
  minutes: number;
  seconds: number;
}

export interface KindTotals {
  lecture: number;
  study: number;
  work: number;
  other: number;
}

export interface SummaryLike {
  totalMs: number;
  byKind: KindTotals;
  count: number;
}

export interface RunningLike {
  kind: TimeEntryKind;
  startedAt: number;
  endedAt: number | null;
}

const pad2 = (value: number): string => String(value).padStart(2, '0');

/** Splits a non-negative millisecond duration into whole hours, minutes, and seconds. */
export function durationParts(ms: number): DurationParts {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return { hours, minutes, seconds };
}

/** Formats elapsed milliseconds as `HH:MM:SS` (clamped at 00:00:00). */
export function formatTimerClock(ms: number): string {
  const { hours, minutes, seconds } = durationParts(ms);
  return `${pad2(hours)}:${pad2(minutes)}:${pad2(seconds)}`;
}

/** ISO weekday (`1` = Monday .. `7` = Sunday) for a calendar date. */
function isoWeekdayOf(date: CalendarDate): Weekday {
  const probe = new Date(0);
  probe.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  const day = probe.getUTCDay();
  return (day === 0 ? 7 : day) as Weekday;
}

/**
 * Computes the inclusive `[from, to]` millisecond bounds and local calendar
 * day span for `period` containing `anchor` in `timeZone`.
 *
 * Uses `startOfDay(nextPeriod) - 1` so DST 23h/25h days are handled accurately
 * and the inclusive `to` parameter never bleeds into the next period.
 */
export function periodBounds(
  anchor: CalendarDate,
  period: TimePeriod,
  timeZone: string,
  weekStart: Weekday,
): TimePeriodBounds {
  if (period === 'day') {
    return {
      from: startOfDay(anchor, timeZone),
      to: endOfDay(anchor, timeZone) - 1,
      firstDay: anchor,
      lastDay: anchor,
    };
  }
  if (period === 'week') {
    const offset = (isoWeekdayOf(anchor) - weekStart + 7) % 7;
    const firstDay = addDays(anchor, -offset);
    const lastDay = addDays(firstDay, 6);
    return {
      from: startOfDay(firstDay, timeZone),
      to: endOfDay(lastDay, timeZone) - 1,
      firstDay,
      lastDay,
    };
  }
  const year = Number(anchor.slice(0, 4));
  const month = Number(anchor.slice(5, 7));
  const firstDay = calendarDate(year, month, 1);
  const nextMonthFirst = calendarDate(year, month + 1, 1);
  const lastDay = addDays(nextMonthFirst, -1);
  return {
    from: startOfDay(firstDay, timeZone),
    to: startOfDay(nextMonthFirst, timeZone) - 1,
    firstDay,
    lastDay,
  };
}

/** Moves `anchor` backward (`-1`) or forward (`1`) by one `period`. */
export function shiftPeriod(anchor: CalendarDate, period: TimePeriod, delta: -1 | 1): CalendarDate {
  if (period === 'day') return addDays(anchor, delta);
  if (period === 'week') return addDays(anchor, delta * 7);
  const year = Number(anchor.slice(0, 4));
  const month = Number(anchor.slice(5, 7));
  const targetFirst = calendarDate(year, month + delta, 1);
  const targetYear = Number(targetFirst.slice(0, 4));
  const targetMonth = Number(targetFirst.slice(5, 7));
  const nextAfterTarget = calendarDate(targetYear, targetMonth + 1, 1);
  const daysInTarget = Number(addDays(nextAfterTarget, -1).slice(8, 10));
  const day = Math.min(Number(anchor.slice(8, 10)), daysInTarget);
  return calendarDate(targetYear, targetMonth, day);
}

/**
 * Adds a live running timer's elapsed duration into a server `summary` when
 * the timer started within `[range.from, range.to]` and matches `filterKind`.
 * Server summaries already count the running entry with `0` ms.
 */
export function liveSummary(
  summary: SummaryLike,
  active: RunningLike | null,
  now: number,
  range: { from: number; to: number },
  filterKind: TimeEntryKind | 'all' = 'all',
): SummaryLike {
  if (
    active?.endedAt !== null ||
    active.startedAt < range.from ||
    active.startedAt > range.to ||
    (filterKind !== 'all' && active.kind !== filterKind)
  ) {
    return summary;
  }
  const extra = Math.max(0, now - active.startedAt);
  if (extra === 0) return summary;
  return {
    count: summary.count,
    totalMs: summary.totalMs + extra,
    byKind: {
      ...summary.byKind,
      [active.kind]: summary.byKind[active.kind] + extra,
    },
  };
}

/** Formats an epoch ms instant as `YYYY-MM-DDTHH:MM` in `timeZone` for `<input type="datetime-local">`. */
export function toDateTimeInput(instant: number, timeZone: string): string {
  const formatter = new Intl.DateTimeFormat('en-u-ca-gregory-nu-latn', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  });
  const parts: Record<string, string> = {};
  for (const part of formatter.formatToParts(instant)) parts[part.type] = part.value;
  const hour = parts.hour === '24' ? '00' : (parts.hour ?? '00');
  return `${parts.year ?? '1970'}-${parts.month ?? '01'}-${parts.day ?? '01'}T${hour}:${parts.minute ?? '00'}`;
}

/** Parses a `YYYY-MM-DDTHH:MM` (or with `:SS`) input value in `timeZone` back to epoch ms. */
export function fromDateTimeInput(value: string, timeZone: string): number | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(value.trim());
  if (!match) return undefined;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const hour = Number(match[4]);
  const minute = Number(match[5]);
  const second = match[6] ? Number(match[6]) : 0;
  if (
    year < 1 ||
    month < 1 ||
    month > 12 ||
    day < 1 ||
    day > 31 ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return undefined;
  }
  const check = calendarDate(year, month, day);
  if (
    Number(check.slice(0, 4)) !== year ||
    Number(check.slice(5, 7)) !== month ||
    Number(check.slice(8, 10)) !== day
  ) {
    return undefined;
  }
  try {
    const dt = new CalendarDateTime(year, month, day, hour, minute, second);
    const ms = toZoned(dt, timeZone).toDate().getTime();
    return Number.isFinite(ms) && ms >= 0 && ms <= TIME_MAX_TIMESTAMP ? ms : undefined;
  } catch {
    return undefined;
  }
}

/** Formats a clock time (`HH:MM` or `h:mm a`) in `locale` and `timeZone`. */
export function formatClockTime(
  instant: number,
  locale: string,
  timeZone: string,
  hourCycle: HourCycle,
): string {
  return new Intl.DateTimeFormat(locale, {
    timeZone,
    hour: 'numeric',
    minute: '2-digit',
    hourCycle,
  }).format(instant);
}

/** Formats a calendar day heading in `locale` and `timeZone`. */
export function formatDayHeading(date: CalendarDate, locale: string, timeZone: string): string {
  return new Intl.DateTimeFormat(locale, {
    timeZone,
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    weekday: 'short',
  }).format(startOfDay(date, timeZone));
}

/** Formats the header label of a `day`, `week`, or `month` period. */
export function formatPeriodHeading(
  firstDay: CalendarDate,
  lastDay: CalendarDate,
  period: TimePeriod,
  locale: string,
  timeZone: string,
): string {
  const firstMs = startOfDay(firstDay, timeZone);
  if (period === 'day') return formatDayHeading(firstDay, locale, timeZone);
  if (period === 'month') {
    return new Intl.DateTimeFormat(locale, {
      timeZone,
      year: 'numeric',
      month: 'long',
    }).format(firstMs);
  }
  const lastMs = startOfDay(lastDay, timeZone);
  const crossYear = firstDay.slice(0, 4) !== lastDay.slice(0, 4);
  const startFmt = new Intl.DateTimeFormat(locale, {
    timeZone,
    ...(crossYear ? { year: 'numeric' } : {}),
    month: 'short',
    day: 'numeric',
  }).format(firstMs);
  const endFmt = new Intl.DateTimeFormat(locale, {
    timeZone,
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(lastMs);
  return `${startFmt} – ${endFmt}`;
}

/** Groups entries (already ordered newest first) by their local calendar day in `timeZone`. */
export function groupEntriesByDay<T extends { startedAt: number }>(
  entries: readonly T[],
  timeZone: string,
): { day: CalendarDate; entries: T[] }[] {
  const groups: { day: CalendarDate; entries: T[] }[] = [];
  for (const entry of entries) {
    const day = dateOfInstant(entry.startedAt, timeZone);
    const tail = groups.at(-1);
    if (tail?.day === day) tail.entries.push(entry);
    else groups.push({ day, entries: [entry] });
  }
  return groups;
}
