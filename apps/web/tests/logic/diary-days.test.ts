/**
 * The days of a diary's page (`logic/diary-days.ts`): a day of the year
 * without its year, read and stepped through `02-29`, and the years a page
 * shows of it, the latest first.
 */
import { describe, expect, it } from 'vitest';

import type { CalendarDate } from '$lib/logic/dates.ts';
import {
  dateIn,
  monthDayOf,
  pageDays,
  parseMonthDay,
  stepMonthDay,
  writable,
  yearOf,
  type MonthDay,
} from '$lib/logic/diary-days.ts';

const day = (text: string): MonthDay => {
  const parsed = parseMonthDay(text);
  if (!parsed) throw new Error(`not a day of the year: ${text}`);
  return parsed;
};
const date = (text: string): CalendarDate => text as CalendarDate;

describe('a day of the year', () => {
  it('is read as MM-DD, 02-29 included, and nothing else', () => {
    expect(parseMonthDay('11-21')).toBe('11-21');
    expect(parseMonthDay('02-29')).toBe('02-29');
    for (const text of ['02-30', '13-01', '00-10', '1-21', '11-21x', '', '2026-11-21']) {
      expect(parseMonthDay(text)).toBeUndefined();
    }
  });

  it('is of a date, and the year is its own', () => {
    expect(monthDayOf(date('2026-11-21'))).toBe('11-21');
    expect(yearOf(date('2026-11-21'))).toBe(2026);
  });

  it('steps through 02-29 and round the end of the year', () => {
    expect(stepMonthDay(day('02-28'), 1)).toBe('02-29');
    expect(stepMonthDay(day('02-29'), 1)).toBe('03-01');
    expect(stepMonthDay(day('03-01'), -1)).toBe('02-29');
    expect(stepMonthDay(day('12-31'), 1)).toBe('01-01');
    expect(stepMonthDay(day('01-01'), -1)).toBe('12-31');
  });

  it('is in a year that has it, of the years 1 to 9999', () => {
    expect(dateIn(2025, day('11-21'))).toBe('2025-11-21');
    expect(dateIn(2024, day('02-29'))).toBe('2024-02-29');
    expect(dateIn(2025, day('02-29'))).toBeUndefined();
    expect(dateIn(1900, day('02-29'))).toBeUndefined();
    expect(dateIn(1, day('01-01'))).toBe('0001-01-01');
    expect(dateIn(0, day('01-01'))).toBeUndefined();
    expect(dateIn(10_000, day('01-01'))).toBeUndefined();
  });
});

describe('the days of a page', () => {
  const today = date('2026-11-21');

  it('go from this year down to the first asked, the latest first', () => {
    expect(pageDays(day('11-21'), today, 2024, [])).toEqual([
      '2026-11-21',
      '2025-11-21',
      '2024-11-21',
    ]);
  });

  it('are this year’s alone for a diary made this year with nothing earlier written', () => {
    expect(pageDays(day('11-21'), today, 2026, [])).toEqual(['2026-11-21']);
  });

  it('reach every year written, before the first asked', () => {
    expect(pageDays(day('11-21'), today, 2026, [date('2023-11-21')])).toEqual([
      '2026-11-21',
      '2025-11-21',
      '2024-11-21',
      '2023-11-21',
    ]);
  });

  it('hold this year’s day before it has come, and it is not written in', () => {
    expect(pageDays(day('12-25'), today, 2025, [])).toEqual(['2026-12-25', '2025-12-25']);
    expect(writable(date('2026-12-25'), today)).toBe(false);
    expect(writable(today, today)).toBe(true);
    expect(writable(date('2025-12-25'), today)).toBe(true);
  });

  it('of 02-29 are the leap years alone', () => {
    expect(pageDays(day('02-29'), today, 2016, [])).toEqual([
      '2024-02-29',
      '2020-02-29',
      '2016-02-29',
    ]);
  });

  it('hold a later year written on, on a device in a time zone ahead', () => {
    expect(pageDays(day('01-01'), date('2026-12-31'), 2026, [date('2027-01-01')])).toEqual([
      '2027-01-01',
      '2026-01-01',
    ]);
  });

  it('stop at the year 1', () => {
    expect(pageDays(day('01-01'), date('0002-03-01'), -5, [])).toEqual([
      '0002-01-01',
      '0001-01-01',
    ]);
  });
});
