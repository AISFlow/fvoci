/**
 * What a calendar view is showing (features/views/calendar/period.svelte.ts):
 * a month or a week, remembered on the device for each view, and what the
 * arrows, "Today" and a picked day do in each.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import { periodTitle } from '$lib/features/views/calendar/period-title.ts';
import { Period, scales } from '$lib/features/views/calendar/period.svelte.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';

const day = (text: string): CalendarDate => text as CalendarDate;
const TODAY = day('2026-11-10');

beforeEach(() => {
  localStorage.clear();
});

describe('month or week', () => {
  it('is a month until a week is chosen, and is remembered for the view', () => {
    const period = new Period('view-a', TODAY);
    expect(period.scale).toBe('month');
    period.scale = 'week';
    expect(new Period('view-a', TODAY).scale).toBe('week');
    expect(new Period('view-b', TODAY).scale).toBe('month');
    expect(localStorage.getItem('fvoci.calendar.scale.view-a')).toBe('week');
  });

  it('is read from the device when it was chosen in an earlier visit', () => {
    localStorage.setItem('fvoci.calendar.scale.view-c', 'week');
    expect(scales.get('view-c')).toBe('week');
    localStorage.setItem('fvoci.calendar.scale.view-d', 'year');
    expect(scales.get('view-d')).toBe('month');
  });
});

describe('a month', () => {
  it('shows the weeks of the month of today, and the arrows go a month', () => {
    const period = new Period('view-month', TODAY);
    expect(period.weeks(7)).toHaveLength(5);
    expect(period.weeks(7)[0]?.[0]).toBe('2026-11-01');
    period.step(1, TODAY);
    expect(period.month).toEqual({ year: 2026, month: 12 });
    expect(period.selected).toBe('2026-12-01');
    period.step(-1, TODAY);
    expect(period.selected).toBe(TODAY);
  });
});

describe('a week', () => {
  it('shows the week of the picked day, for the reader’s first day of the week', () => {
    const period = new Period('view-week', TODAY);
    period.scale = 'week';
    expect(period.weeks(7)).toEqual([
      [
        '2026-11-08',
        '2026-11-09',
        '2026-11-10',
        '2026-11-11',
        '2026-11-12',
        '2026-11-13',
        '2026-11-14',
      ],
    ]);
    expect(period.weeks(1)[0]?.[0]).toBe('2026-11-09');
  });

  it('goes a week with the arrows, into the next month and year', () => {
    const period = new Period('view-step', day('2026-12-29'));
    period.scale = 'week';
    period.step(1, TODAY);
    expect(period.selected).toBe('2027-01-05');
    expect(period.month).toEqual({ year: 2027, month: 1 });
    expect(period.weeks(7)[0]?.[0]).toBe('2027-01-03');
  });

  it('turned back into a month shows the month of the day picked in the week', () => {
    const period = new Period('view-back', TODAY);
    period.scale = 'week';
    period.step(3, TODAY);
    period.scale = 'month';
    expect(period.month).toEqual({ year: 2026, month: 12 });
    expect(period.selected).toBe('2026-12-01');
  });

  it('"Today" brings back today, the week of it, and the month of it', () => {
    const period = new Period('view-today', TODAY);
    period.scale = 'week';
    period.step(-6, TODAY);
    period.goTo(TODAY);
    expect(period.selected).toBe(TODAY);
    expect(period.weeks(7)[0]?.[0]).toBe('2026-11-08');
  });
});

/** The text with the narrow spaces `Intl` puts around a dash read as plain ones. */
const plain = (text: string): string => text.replace(/\s/gu, ' ');

describe('what the row calls it', () => {
  it('is the month, or the days of the week, with the year only out of today’s, in the reader’s language', () => {
    const period = new Period('view-title', TODAY);
    expect(periodTitle('month', period.weeks(7), period.month, 'en-US', TODAY)).toBe(
      'November 2026',
    );
    period.scale = 'week';
    expect(plain(periodTitle('week', period.weeks(7), period.month, 'en-US', TODAY))).toBe(
      'Nov 8 – 14',
    );
    expect(plain(periodTitle('week', period.weeks(7), period.month, 'ko-KR', TODAY))).toBe(
      '11월 8일~14일',
    );
    // Another year than today's says which.
    expect(
      plain(periodTitle('week', period.weeks(7), period.month, 'en-US', day('2027-01-04'))),
    ).toBe('Nov 8 – 14, 2026');
  });

  it('names both months and years of a week across the new year', () => {
    const period = new Period('view-title-2', day('2026-12-30'));
    period.scale = 'week';
    expect(plain(periodTitle('week', period.weeks(1), period.month, 'en-US', TODAY))).toBe(
      'Dec 28, 2026 – Jan 3, 2027',
    );
  });
});
