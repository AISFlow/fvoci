import { describe, expect, it } from 'vitest';

import type { CalendarDate } from '$lib/logic/dates.ts';
import {
  durationParts,
  formatTimerClock,
  fromDateTimeInput,
  groupEntriesByDay,
  liveSummary,
  periodBounds,
  shiftPeriod,
  toDateTimeInput,
} from '$lib/logic/time.ts';

describe('time logic: duration formatting', () => {
  it('formats HH:MM:SS and clamps negatives to 00:00:00', () => {
    expect(formatTimerClock(-5000)).toBe('00:00:00');
    expect(formatTimerClock(0)).toBe('00:00:00');
    expect(formatTimerClock(5_999)).toBe('00:00:05');
    expect(formatTimerClock(3_661_000)).toBe('01:01:01');
    expect(formatTimerClock(100 * 3_600_000)).toBe('100:00:00');
  });

  it('splits hours, minutes, and seconds cleanly', () => {
    expect(durationParts(7_385_000)).toEqual({ hours: 2, minutes: 3, seconds: 5 });
  });
});

describe('time logic: inclusive period boundaries', () => {
  it('computes exact inclusive day bounds [startOfDay, startOfNextDay - 1] in Asia/Seoul', () => {
    const anchor = '2026-11-24' as CalendarDate;
    const bounds = periodBounds(anchor, 'day', 'Asia/Seoul', 1);
    expect(bounds.firstDay).toBe('2026-11-24');
    expect(bounds.lastDay).toBe('2026-11-24');
    // 2026-11-24T00:00:00+09:00 == 2026-11-23T15:00:00Z
    expect(bounds.from).toBe(Date.UTC(2026, 10, 23, 15, 0, 0, 0));
    expect(bounds.to).toBe(Date.UTC(2026, 10, 24, 15, 0, 0, 0) - 1);
  });

  it('respects DST 23-hour spring-forward and 25-hour fall-back days', () => {
    // America/New_York spring forward: 2026-03-08 (23h)
    const spring = periodBounds('2026-03-08' as CalendarDate, 'day', 'America/New_York', 7);
    expect(spring.to - spring.from + 1).toBe(23 * 3_600_000);

    // America/New_York fall back: 2026-11-01 (25h)
    const fall = periodBounds('2026-11-01' as CalendarDate, 'day', 'America/New_York', 7);
    expect(fall.to - fall.from + 1).toBe(25 * 3_600_000);
  });

  it('computes week bounds for Monday and Sunday week starts', () => {
    // 2026-11-24 is Tuesday
    const mon = periodBounds('2026-11-24' as CalendarDate, 'week', 'UTC', 1);
    expect(mon.firstDay).toBe('2026-11-23');
    expect(mon.lastDay).toBe('2026-11-29');
    expect(mon.from).toBe(Date.UTC(2026, 10, 23, 0, 0, 0, 0));
    expect(mon.to).toBe(Date.UTC(2026, 10, 30, 0, 0, 0, 0) - 1);

    const sun = periodBounds('2026-11-24' as CalendarDate, 'week', 'UTC', 7);
    expect(sun.firstDay).toBe('2026-11-22');
    expect(sun.lastDay).toBe('2026-11-28');
  });

  it('computes inclusive month bounds across leap and non-leap Februaries', () => {
    const leap = periodBounds('2024-02-15' as CalendarDate, 'month', 'UTC', 1);
    expect(leap.firstDay).toBe('2024-02-01');
    expect(leap.lastDay).toBe('2024-02-29');
    expect(leap.to).toBe(Date.UTC(2024, 2, 1, 0, 0, 0, 0) - 1);

    const nonLeap = periodBounds('2026-02-15' as CalendarDate, 'month', 'UTC', 1);
    expect(nonLeap.lastDay).toBe('2026-02-28');
  });

  it('shifts day, week, and month anchors clamping month ends', () => {
    expect(shiftPeriod('2026-01-31' as CalendarDate, 'month', 1)).toBe('2026-02-28');
    expect(shiftPeriod('2024-01-31' as CalendarDate, 'month', 1)).toBe('2024-02-29');
    expect(shiftPeriod('2026-03-01' as CalendarDate, 'day', -1)).toBe('2026-02-28');
    expect(shiftPeriod('2026-11-24' as CalendarDate, 'week', 1)).toBe('2026-12-01');
  });
});

describe('time logic: date-time input conversion and live summary', () => {
  it('round-trips datetime-local strings in an explicit IANA timezone', () => {
    const instant = Date.UTC(2026, 10, 24, 1, 30, 0); // 10:30 in Asia/Seoul
    const formatted = toDateTimeInput(instant, 'Asia/Seoul');
    expect(formatted).toBe('2026-11-24T10:30');
    expect(fromDateTimeInput(formatted, 'Asia/Seoul')).toBe(instant);
    expect(fromDateTimeInput('2026-02-30T10:00', 'UTC')).toBeUndefined();
    expect(fromDateTimeInput('not-a-date', 'UTC')).toBeUndefined();
  });

  it('adds live active elapsed ms to summary only when inside range and filter', () => {
    const base = {
      totalMs: 600_000,
      byKind: { lecture: 600_000, study: 0, work: 0, other: 0 },
      count: 2,
    };
    const active = { kind: 'study' as const, startedAt: 1_000_000, endedAt: null };
    const range = { from: 900_000, to: 2_000_000 };

    const updated = liveSummary(base, active, 1_050_000, range, 'all');
    expect(updated.totalMs).toBe(650_000);
    expect(updated.byKind.study).toBe(50_000);
    expect(updated.count).toBe(2);

    // Filtered out by kind
    expect(liveSummary(base, active, 1_050_000, range, 'lecture')).toEqual(base);

    // Outside period range
    expect(liveSummary(base, active, 1_050_000, { from: 0, to: 500_000 }, 'all')).toEqual(base);
  });

  it('groups entries by local day in the given timezone', () => {
    const entries = [
      { id: '1', startedAt: Date.UTC(2026, 10, 24, 16, 0, 0) }, // 2026-11-25 01:00 KST
      { id: '2', startedAt: Date.UTC(2026, 10, 24, 14, 0, 0) }, // 2026-11-24 23:00 KST
    ];
    const groups = groupEntriesByDay(entries, 'Asia/Seoul');
    expect(groups.map((g) => g.day)).toEqual(['2026-11-25', '2026-11-24']);
  });
});
