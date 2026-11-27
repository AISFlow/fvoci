/**
 * The days written in a row (`logic/diary-streak.ts`): the runs written days
 * make, through the ends of months and years and 29 February, and the count
 * up to today, or up to yesterday while today is not written yet.
 */
import { describe, expect, it } from 'vitest';

import { addDays, type CalendarDate } from '$lib/logic/dates.ts';
import { runsOf, streakOf } from '$lib/logic/diary-streak.ts';

const days = (...texts: string[]): CalendarDate[] => texts as CalendarDate[];
const on = (text: string): CalendarDate => text as CalendarDate;

describe('the runs of written days', () => {
  it('go through the ends of months and years, and 29 February only where there is one', () => {
    expect(runsOf([])).toEqual([]);
    expect(
      runsOf(
        days(
          '2024-03-03',
          '2023-12-31',
          '2024-02-29',
          '2024-01-01',
          '2024-02-28',
          '2024-03-01',
          '2025-02-28',
          '2025-03-01',
        ),
      ),
    ).toEqual([
      { from: '2023-12-31', to: '2024-01-01', days: 2 },
      { from: '2024-02-28', to: '2024-03-01', days: 3 },
      { from: '2024-03-03', to: '2024-03-03', days: 1 },
      { from: '2025-02-28', to: '2025-03-01', days: 2 },
    ]);
  });
});

describe('the days written in a row', () => {
  const runs = runsOf(days('2026-10-01', '2026-10-02', '2026-11-18', '2026-11-19', '2026-11-20'));

  it('count up to today where today is written', () => {
    const withToday = runsOf(
      days('2026-10-01', '2026-10-02', '2026-11-18', '2026-11-19', '2026-11-20', '2026-11-21'),
    );
    expect(streakOf(withToday, on('2026-11-21'))).toEqual({ current: 4, longest: 4 });
  });

  it('count up to yesterday while today is not written yet', () => {
    expect(streakOf(runs, on('2026-11-21'))).toEqual({ current: 3, longest: 3 });
  });

  it('are none once a whole day went by unwritten, and the longest stays', () => {
    expect(streakOf(runs, on('2026-11-22'))).toEqual({ current: 0, longest: 3 });
  });

  it('leave out days written after today, on a device where they have come', () => {
    const ahead = runsOf(days('2026-11-20', '2026-11-21', '2026-11-22'));
    expect(streakOf(ahead, on('2026-11-21'))).toEqual({ current: 2, longest: 2 });
    expect(streakOf(runsOf(days('2026-11-22')), on('2026-11-20'))).toEqual({
      current: 0,
      longest: 0,
    });
    // The longest run of the past stays the longest, whatever is written ahead.
    const before = runsOf(
      days('2026-10-01', '2026-10-02', '2026-10-03', '2026-11-22', '2026-11-23'),
    );
    expect(streakOf(before, on('2026-11-21'))).toEqual({ current: 0, longest: 3 });
  });

  it('go on through the end of the year and 29 February', () => {
    const year = runsOf(days('2023-12-30', '2023-12-31', '2024-01-01'));
    expect(streakOf(year, on('2024-01-02')).current).toBe(3);
    const leap = runsOf(days('2024-02-28', '2024-02-29'));
    expect(streakOf(leap, on('2024-03-01')).current).toBe(2);
    expect(streakOf(runsOf(days('2025-02-28')), on('2025-03-01')).current).toBe(1);
  });

  it('count a run as long as the diary', () => {
    const long = Array.from({ length: 400 }, (_, index) => addDays(on('2025-10-18'), index));
    expect(streakOf(runsOf(long), long.at(-1) ?? on('2026-11-21'))).toEqual({
      current: 400,
      longest: 400,
    });
  });
});
