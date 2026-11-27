/**
 * Calendar dates, formatting and text helpers, past what utils.test.ts
 * covers of them.
 */
import { describe, expect, it } from 'vitest';

import { arrayMove, arraysEqual } from '$lib/logic/arrays.ts';
import {
  addDays,
  calendarDate,
  comparePoints,
  dateOfInstant,
  dateOfPoint,
  daysBetween,
  daysOf,
  endOfDay,
  parseCalendarDate,
  startOfDay,
} from '$lib/logic/dates.ts';
import {
  displayDate,
  displayDateTime,
  humanFileSize,
  humanizeDuration,
  inputDate,
  relativeTime,
  roundTo,
} from '$lib/logic/format.ts';
import {
  countCheckboxesInMarkdown,
  foldCase,
  hashCode,
  htmlDecode,
  htmlEncode,
  sanitizeFilename,
} from '$lib/logic/text.ts';
import { fillPath, normalizeBasePath } from '$lib/logic/urls.ts';
import { readDisplayName, userDisplayName } from '$lib/logic/users.ts';
describe('calendar dates', () => {
  it('are written year-month-day and compare as text', () => {
    expect(calendarDate(2026, 3, 5)).toBe('2026-03-05');
    expect(calendarDate(2026, 3, 5) < calendarDate(2026, 11, 1)).toBe(true);
    expect(calendarDate(2026, 13, 1)).toBe('2027-01-01');
  });

  it('are read from text only when the text is a real day', () => {
    expect(parseCalendarDate('2026-03-05')).toBe('2026-03-05');
    expect(parseCalendarDate('2026-02-30')).toBeUndefined();
    expect(parseCalendarDate('2026-3-5')).toBeUndefined();
    expect(parseCalendarDate('tomorrow')).toBeUndefined();
  });

  it('move by days across months, years and leap days', () => {
    expect(addDays(calendarDate(2026, 12, 31), 1)).toBe('2027-01-01');
    expect(addDays(calendarDate(2028, 3, 1), -1)).toBe('2028-02-29');
    expect(daysBetween(calendarDate(2026, 1, 1), calendarDate(2027, 1, 1))).toBe(365);
    expect(daysBetween(calendarDate(2026, 3, 5), calendarDate(2026, 3, 1))).toBe(-4);
  });

  it('give the day a moment falls on for a reader in a time zone', () => {
    const moment = Date.UTC(2023, 11, 6, 18, 0);
    expect(dateOfInstant(moment, 'UTC')).toBe('2023-12-06');
    expect(dateOfInstant(moment, 'Asia/Seoul')).toBe('2023-12-07');
    expect(dateOfInstant(moment, 'America/Los_Angeles')).toBe('2023-12-06');
    expect(dateOfInstant(Date.UTC(2023, 11, 6, 10, 30), 'Pacific/Kiritimati')).toBe('2023-12-07');
    expect(dateOfInstant(Date.UTC(2023, 11, 8, 10, 30), 'Pacific/Pago_Pago')).toBe('2023-12-07');
  });

  it("begin and end at midnight on the reader's clock", () => {
    const fifth = calendarDate(2026, 3, 5);
    expect(startOfDay(fifth, 'UTC')).toBe(Date.UTC(2026, 2, 5));
    expect(startOfDay(fifth, 'Asia/Seoul')).toBe(Date.UTC(2026, 2, 4, 15));
    expect(endOfDay(fifth, 'Asia/Seoul')).toBe(Date.UTC(2026, 2, 5, 15));
    expect(startOfDay(fifth, 'Asia/Kathmandu')).toBe(Date.UTC(2026, 2, 4, 18, 15));
    expect(dateOfInstant(startOfDay(fifth, 'Asia/Seoul'), 'Asia/Seoul')).toBe(fifth);
    expect(dateOfInstant(endOfDay(fifth, 'Asia/Seoul') - 1, 'Asia/Seoul')).toBe(fifth);
  });
});

describe('date values', () => {
  const fifth = { date: calendarDate(2026, 3, 5) };
  const ninth = { date: calendarDate(2026, 3, 9) };

  it('cover one day, or every day of a range', () => {
    expect(daysOf({ from: fifth }, 'UTC')).toEqual({ first: '2026-03-05', last: '2026-03-05' });
    expect(daysOf({ from: fifth, to: ninth }, 'UTC')).toEqual({
      first: '2026-03-05',
      last: '2026-03-09',
    });
    expect(daysOf({}, 'UTC')).toBeUndefined();
  });

  it('cover the same days when a range was stored backwards', () => {
    expect(daysOf({ from: ninth, to: fifth }, 'UTC')).toEqual({
      first: '2026-03-05',
      last: '2026-03-09',
    });
  });

  it("fall on the reader's day when they carry a time", () => {
    const moment = { instant: Date.UTC(2026, 2, 5, 20, 0) };
    expect(daysOf({ from: moment }, 'UTC')?.first).toBe('2026-03-05');
    expect(daysOf({ from: moment }, 'Asia/Seoul')?.first).toBe('2026-03-06');
    expect(dateOfPoint(fifth, 'Asia/Seoul')).toBe('2026-03-05');
  });

  it('order by day, a bare day before a moment of that day, moments by time', () => {
    const morning = { instant: Date.UTC(2026, 2, 5, 8, 0) };
    const evening = { instant: Date.UTC(2026, 2, 5, 20, 0) };
    expect(comparePoints(fifth, ninth, 'UTC')).toBe(-1);
    expect(comparePoints(ninth, fifth, 'UTC')).toBe(1);
    expect(comparePoints(fifth, fifth, 'UTC')).toBe(0);
    expect(comparePoints(fifth, morning, 'UTC')).toBe(-1);
    expect(comparePoints(morning, fifth, 'UTC')).toBe(1);
    expect(comparePoints(morning, evening, 'UTC')).toBe(-1);
    // In Seoul the evening moment is already the 6th.
    expect(comparePoints(evening, { date: calendarDate(2026, 3, 6) }, 'Asia/Seoul')).toBe(1);
  });
});

describe('durations and sizes', () => {
  const now = Date.UTC(2026, 5, 15, 12, 0, 0);
  const ago = (milliseconds: number): number => now - milliseconds;
  const MINUTE = 60_000;
  const HOUR = 60 * MINUTE;
  const DAY = 24 * HOUR;

  it('says how long ago something was, in the unit people would use', () => {
    expect(relativeTime(ago(10_000), 'en', now)).toBe('now');
    expect(relativeTime(ago(5 * MINUTE), 'en', now)).toBe('5 minutes ago');
    expect(relativeTime(ago(50 * MINUTE), 'en', now)).toBe('1 hour ago');
    expect(relativeTime(ago(3 * HOUR), 'en', now)).toBe('3 hours ago');
    expect(relativeTime(ago(30 * HOUR), 'en', now)).toBe('yesterday');
    expect(relativeTime(ago(40 * DAY), 'en', now)).toBe('last month');
    expect(relativeTime(ago(400 * DAY), 'en', now)).toBe('last year');
    expect(relativeTime(now + 2 * DAY, 'en', now)).toBe('in 2 days');
    expect(relativeTime(ago(3 * HOUR), 'ko', now)).toBe('3시간 전');
  });

  it('rounds each unit before choosing it', () => {
    // 44.7 seconds round to 45: no longer "a few seconds".
    expect(humanizeDuration(44_700, 'en')).toBe('1 minute');
    expect(humanizeDuration(44 * MINUTE + 40_000, 'en')).toBe('1 hour');
    expect(humanizeDuration(21 * HOUR + 40 * MINUTE, 'en')).toBe('1 day');
    expect(humanizeDuration(44 * MINUTE + 20_000, 'en')).toBe('44 minutes');
    expect(humanizeDuration(45.6 * DAY, 'en')).toBe('1 month');
    expect(humanizeDuration(400 * DAY, 'en')).toBe('1 year');
    expect(humanizeDuration(800 * DAY, 'en')).toBe('2 years');
  });

  it('shows nothing for a date that is not one', () => {
    const reader = { locale: 'en', timeZone: 'UTC' };
    expect(displayDate(Number.NaN, reader)).toBe('');
    expect(displayDateTime(Number.NaN, reader)).toBe('');
    expect(inputDate(Number.NaN, reader)).toBe('');
  });

  it('names a length of time', () => {
    expect(humanizeDuration(20_000, 'en')).toBe('20 seconds');
    expect(humanizeDuration(DAY, 'en')).toBe('1 day');
    expect(humanizeDuration(3 * DAY, 'en')).toBe('3 days');
    expect(humanizeDuration(70 * DAY, 'en')).toBe('2 months');
    expect(humanizeDuration(3 * DAY, 'ko')).toBe('3일');
  });

  it('rounds to decimal places', () => {
    expect(roundTo(56.666, 2)).toBe(56.67);
    expect(roundTo(-0.004, 2)).toBe(-0);
  });

  it('writes file sizes in binary or decimal units', () => {
    expect(humanFileSize(500)).toBe('500 B');
    expect(humanFileSize(1536)).toBe('1.5 KiB');
    expect(humanFileSize(1_500_000, true)).toBe('1.5 MB');
    expect(humanFileSize(1024 * 1024 * 1024, false, 2)).toBe('1.00 GiB');
  });
});

describe('text', () => {
  it('removes every character a file name may not have (Q7)', () => {
    expect(sanitizeFilename('a/b:c')).toBe('abc');
    expect(sanitizeFilename('v1.2.3 notes')).toBe('v123 notes');
    expect(sanitizeFilename('a/b\\c/d?e*f|g"h<i>j')).toBe('abcdefghij');
    expect(sanitizeFilename('보드 제목')).toBe('보드 제목');
  });

  it('folds case the same way for every reader', () => {
    expect(foldCase('HeLLo')).toBe('hello');
    expect(foldCase('ＡＢＣ')).toBe('abc');
    expect(foldCase('\u1112\u1161\u11ab')).toBe('한');
    expect(foldCase('TITLE')).toBe('title');
  });

  it('encodes and decodes the characters that matter in HTML', () => {
    expect(htmlEncode('<a href="x">&</a>')).toBe('&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;');
    expect(htmlDecode(htmlEncode('<a href="x">&</a>'))).toBe('<a href="x">&</a>');
  });

  it('hashes a text to a stable 32-bit number', () => {
    expect(hashCode('')).toBe(0);
    expect(hashCode('abc')).toBe(96354);
    expect(hashCode('a longer text that overflows')).toBe(hashCode('a longer text that overflows'));
  });

  it('counts task list items of every list marker, but not inside code fences', () => {
    const text = [
      '* [x] star',
      '1. [ ] numbered',
      '  - [X] nested',
      '- [] not a task',
      '```',
      '- [x] code',
      '```',
    ].join('\n');
    expect(countCheckboxesInMarkdown(text)).toEqual({ total: 3, checked: 2 });
    expect(countCheckboxesInMarkdown('')).toEqual({ total: 0, checked: 0 });
  });
});

describe('small helpers', () => {
  it('compares lists item by item', () => {
    expect(arraysEqual(['a', 'b'], ['a', 'b'])).toBe(true);
    expect(arraysEqual(['a', 'b'], ['b', 'a'])).toBe(false);
    expect(arraysEqual(['a'], ['a', 'b'])).toBe(false);
    expect(arraysEqual(undefined, [])).toBe(false);
    expect(arraysEqual(null, null)).toBe(true);
  });

  it('moves an item within a list', () => {
    const list = ['a', 'b', 'c', 'd'];
    arrayMove(list, 0, 2);
    expect(list).toEqual(['b', 'c', 'a', 'd']);
  });

  it('fills route parameters and drops missing optional ones', () => {
    expect(fillPath('/board/:boardId/:viewId?/:cardId?', { boardId: 'b', viewId: 'v' })).toBe(
      '/board/b/v',
    );
    expect(() => fillPath('/board/:boardId', {})).toThrow('boardId');
  });

  it('strips the slashes around a base path', () => {
    expect(['/boards/', 'boards', '', undefined, '/a/b//'].map(normalizeBasePath)).toEqual([
      'boards',
      'boards',
      '',
      '',
      'a/b',
    ]);
  });

  it('shows a user by display name and login name, or the login name alone', () => {
    expect(userDisplayName({ username: 'ada', displayName: 'Ada Lovelace' })).toBe(
      'Ada Lovelace (@ada)',
    );
    expect(userDisplayName({ username: 'ada', displayName: '' })).toBe('ada');
  });

  it('reads a display name as the server does', () => {
    expect(readDisplayName('  Ada \u3000')).toEqual({ name: 'Ada' });
    expect(readDisplayName(' \n ')).toEqual({ name: '' });
    expect(readDisplayName('가'.repeat(64))).toEqual({ name: '가'.repeat(64) });
    // 64 UTF-16 code units: an emoji is two.
    expect(readDisplayName('😀'.repeat(33))).toEqual({ refused: 'too_long' });
    for (const refused of ['a\u0000b', 'a\u200bb', 'a\u202eb', 'a\u2028b', '\u00ad', 'a\u0085b']) {
      expect(readDisplayName(refused)).toEqual({ refused: 'invalid_characters' });
    }
    // The server trims White_Space, which has U+0085 and not U+FEFF.
    expect(readDisplayName('\u0085Ada')).toEqual({ name: 'Ada' });
    expect(readDisplayName('\ufeffAda')).toEqual({ refused: 'invalid_characters' });
  });
});
