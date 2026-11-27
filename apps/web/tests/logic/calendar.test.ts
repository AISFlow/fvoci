/**
 * The arithmetic of the calendar view (lib/logic/calendar.ts) and the names
 * it writes (lib/logic/format.ts): the weeks of a month for each first day
 * of the week, the edges of months, bars across several days and weeks, a
 * week whose cells have too few rows, and the day a moment is on for a
 * reader in a given time zone.
 *
 * The suite runs with the process in three time zones (tools/test-zones.ts);
 * nothing here asks the machine for its own, so every result is the same.
 */
import { describe, expect, it } from 'vitest';

import {
  cardDays,
  cardsByDay,
  compareSpans,
  displayableProperties,
  displayProperty,
  firstOf,
  foldWeek,
  inMonth,
  lastOf,
  layWeek,
  monthOf,
  shiftMonth,
  spansOf,
  weekdayOf,
  weekOf,
  weeksOfMonth,
  type Segment,
  type Span,
} from '$lib/logic/calendar.ts';
import { addDays, calendarDate, type CalendarDate } from '$lib/logic/dates.ts';
import {
  displayDay,
  displayDayLong,
  displayDayNumber,
  displayDayRange,
  displayMonth,
  displayTime,
  displayWeekday,
} from '$lib/logic/format.ts';
import { createCard, setCardProperty, type Card, type PropertyTemplate } from '$lib/logic/wire.ts';

import { at } from '../support/zones.ts';

const day = (text: string): CalendarDate => text as CalendarDate;
const span = (id: string, first: string, last = first): Span => ({
  id,
  first: day(first),
  last: day(last),
});
const SUNDAY = 7;
const MONDAY = 1;
const SATURDAY = 6;
const NOVEMBER_2026 = { year: 2026, month: 11 };

describe('months and weeks', () => {
  it('knows the weekday of a date, before 1970 too', () => {
    expect(weekdayOf(day('1970-01-01'))).toBe(4);
    expect(weekdayOf(day('2026-11-01'))).toBe(SUNDAY);
    expect(weekdayOf(day('2026-11-10'))).toBe(2);
    expect(weekdayOf(day('1969-12-28'))).toBe(SUNDAY);
    expect(weekdayOf(day('0001-01-01'))).toBe(MONDAY);
  });

  it('starts a week on Sunday, Monday or Saturday', () => {
    const tuesday = day('2026-11-10');
    expect(weekOf(tuesday, SUNDAY)[0]).toBe('2026-11-08');
    expect(weekOf(tuesday, MONDAY)[0]).toBe('2026-11-09');
    expect(weekOf(tuesday, SATURDAY)[0]).toBe('2026-11-07');
    expect(weekOf(tuesday, SUNDAY)).toHaveLength(7);
    // The first day of the week is a week of its own first day.
    expect(weekOf(day('2026-11-08'), SUNDAY)[0]).toBe('2026-11-08');
    expect(weekOf(day('2026-11-07'), SUNDAY)[6]).toBe('2026-11-07');
  });

  it('shows every week with a day of the month in it', () => {
    // November 2026 begins on a Sunday and ends on a Monday.
    const firsts = (weekStart: 1 | 6 | 7): string[] =>
      weeksOfMonth(NOVEMBER_2026, weekStart).map((week) => week[0] ?? '');
    expect(firsts(SUNDAY)).toEqual([
      '2026-11-01',
      '2026-11-08',
      '2026-11-15',
      '2026-11-22',
      '2026-11-29',
    ]);
    expect(firsts(MONDAY)).toHaveLength(6);
    expect(firsts(MONDAY)[0]).toBe('2026-10-26');
    expect(weeksOfMonth(NOVEMBER_2026, MONDAY).at(-1)?.[6]).toBe('2026-12-06');
    expect(firsts(SATURDAY)).toHaveLength(5);
    expect(firsts(SATURDAY)[0]).toBe('2026-10-31');
  });

  it('has four weeks for a February that fits, six for a month that spills', () => {
    // February 2026 begins on a Sunday and has 28 days.
    expect(weeksOfMonth({ year: 2026, month: 2 }, SUNDAY)).toHaveLength(4);
    expect(weeksOfMonth({ year: 2026, month: 2 }, MONDAY)).toHaveLength(5);
    // August 2026 begins on a Saturday and has 31 days.
    expect(weeksOfMonth({ year: 2026, month: 8 }, SUNDAY)).toHaveLength(6);
  });

  it('crosses the edges of months and years', () => {
    expect(shiftMonth({ year: 2026, month: 12 }, 1)).toEqual({ year: 2027, month: 1 });
    expect(shiftMonth({ year: 2026, month: 1 }, -1)).toEqual({ year: 2025, month: 12 });
    expect(shiftMonth({ year: 2026, month: 11 }, -23)).toEqual({ year: 2024, month: 12 });
    expect(lastOf({ year: 2024, month: 2 })).toBe('2024-02-29');
    expect(lastOf({ year: 2100, month: 2 })).toBe('2100-02-28');
    expect(lastOf({ year: 2026, month: 12 })).toBe('2026-12-31');
    expect(firstOf({ year: 99, month: 3 })).toBe('0099-03-01');
    expect(monthOf(day('2026-11-30'))).toEqual(NOVEMBER_2026);
    expect(inMonth(day('2026-11-30'), NOVEMBER_2026)).toBe(true);
    expect(inMonth(day('2026-12-01'), NOVEMBER_2026)).toBe(false);
    expect(inMonth(day('2026-10-31'), NOVEMBER_2026)).toBe(false);
  });
});

describe('bars in a week', () => {
  const week = weekOf(day('2026-11-10'), SUNDAY); // 8 to 14 November

  it('cuts a card of several weeks into one bar a week, marked at the cut ends', () => {
    const long = span('long', '2026-11-05', '2026-11-20');
    expect(layWeek(week, [long])).toEqual([
      { id: 'long', lane: 0, column: 0, length: 7, fromBefore: true, toAfter: true },
    ]);
    const weeks = weeksOfMonth(NOVEMBER_2026, SUNDAY);
    const bars = weeks.flatMap((days) => layWeek(days, [long]));
    expect(bars.map((bar) => [bar.column, bar.length, bar.fromBefore, bar.toAfter])).toEqual([
      [4, 3, false, true],
      [0, 7, true, true],
      [0, 6, true, false],
    ]);
  });

  it('leaves out what is not in the week', () => {
    expect(layWeek(week, [span('before', '2026-11-07'), span('after', '2026-11-15')])).toEqual([]);
  });

  it('puts the earlier card on top, then the longer one, then the order given', () => {
    const bars = layWeek(week, [
      span('short', '2026-11-10'),
      span('later', '2026-11-11', '2026-11-12'),
      span('longer', '2026-11-10', '2026-11-13'),
      span('earliest', '2026-11-09'),
      span('same', '2026-11-10'),
    ]);
    const lanes = Object.fromEntries(bars.map((bar) => [bar.id, bar.lane]));
    expect(bars.map((bar) => bar.id)).toEqual(['earliest', 'longer', 'short', 'same', 'later']);
    // "earliest" (9th) leaves the 10th free; "longer" takes row 0 from the 10th to the 13th.
    expect(lanes).toEqual({ earliest: 0, longer: 0, short: 1, same: 2, later: 1 });
  });

  it('gives a card the highest row free on all its days, and the rows differ by week', () => {
    const dark = span('dark', '2026-11-12', '2026-11-24');
    const first = span('first', '2026-11-12');
    const second = span('second', '2026-11-11', '2026-11-12');
    const [one, two] = weeksOfMonth(NOVEMBER_2026, SUNDAY).slice(1, 3);
    const lane = (segments: Segment[], id: string): number | undefined =>
      segments.find((segment) => segment.id === id)?.lane;
    expect(lane(layWeek(one ?? [], [dark, first, second]), 'dark')).toBe(1);
    expect(lane(layWeek(two ?? [], [dark, first, second]), 'dark')).toBe(0);
  });

  it('keeps every row a bar of its own day when nothing overflows', () => {
    const segments = layWeek(week, [span('a', '2026-11-10'), span('b', '2026-11-10')]);
    expect(foldWeek(segments, 3)).toEqual({ shown: segments, more: [0, 0, 0, 0, 0, 0, 0] });
  });

  it('says "+N" in the last row of a day that has more cards than rows', () => {
    const segments = layWeek(week, [
      span('a', '2026-11-12'),
      span('b', '2026-11-12'),
      span('c', '2026-11-12'),
      span('d', '2026-11-13'),
    ]);
    const folded = foldWeek(segments, 2);
    // Thursday needs three rows and has two: one bar and "+2".
    expect(folded.shown.map((segment) => segment.id)).toEqual(['a', 'd']);
    expect(folded.more).toEqual([0, 0, 0, 0, 2, 0, 0]);
  });

  it('hides a bar in the whole week when one of its days is full', () => {
    const segments = layWeek(week, [
      span('crowd1', '2026-11-12'),
      span('crowd2', '2026-11-12'),
      span('range', '2026-11-12', '2026-11-14'),
    ]);
    // One row: the 12th needs it for "+N", so the range goes from the 13th and 14th too.
    const folded = foldWeek(segments, 1);
    expect(folded.shown).toEqual([]);
    expect(folded.more).toEqual([0, 0, 0, 0, 3, 1, 1]);
  });

  it('draws at least one row, however low the cell', () => {
    const segments = layWeek(week, [span('a', '2026-11-10')]);
    expect(foldWeek(segments, 0).shown).toHaveLength(1);
  });

  it('lists the cards of each day once per day, in the order given', () => {
    const days = weekOf(day('2026-11-10'), SUNDAY);
    const byDay = cardsByDay(
      [span('a', '2026-11-01', '2026-11-09'), span('b', '2026-11-09'), span('c', '2026-12-01')],
      days,
    );
    expect(byDay.get(day('2026-11-08'))).toEqual(['a']);
    expect(byDay.get(day('2026-11-09'))).toEqual(['a', 'b']);
    expect(byDay.get(day('2026-11-10'))).toEqual([]);
    expect([...byDay.values()].flat()).toHaveLength(3);
  });

  it('with six hundred cards, every card of every day is either a bar or counted in "+N"', () => {
    const many = Array.from({ length: 600 }, (_, index) =>
      span(
        `c${String(index)}`,
        addDays(day('2026-11-01'), index % 30),
        addDays(day('2026-11-01'), (index % 30) + (index % 4)),
      ),
    );
    const weeks = weeksOfMonth(NOVEMBER_2026, SUNDAY);
    const byDay = cardsByDay(many, weeks.flat());
    for (const week of weeks) {
      const { shown, more } = foldWeek(layWeek(week, many), 4);
      // Four rows: three bars at most and "+N" in the last.
      expect(Math.max(...shown.map((segment) => segment.lane))).toBeLessThan(4);
      week.forEach((date, column) => {
        const bars = shown.filter(
          (segment) => column >= segment.column && column < segment.column + segment.length,
        ).length;
        expect(bars + (more[column] ?? 0)).toBe(byDay.get(date)?.length);
      });
    }
  });

  it('puts a card without a time first on a day, then the earlier time, then the order given', () => {
    const at8 = { ...span('at8', '2026-11-12'), time: at('Asia/Seoul', 2026, 11, 12, 8) };
    const at15 = { ...span('at15', '2026-11-12'), time: at('Asia/Seoul', 2026, 11, 12, 15) };
    const allDay = span('allDay', '2026-11-12');
    const other = span('other', '2026-11-12');
    const bars = layWeek(week, [at15, allDay, at8, other]);
    expect(bars.map((bar) => bar.id)).toEqual(['allDay', 'other', 'at8', 'at15']);
    expect([at15, other, at8, allDay].sort(compareSpans).map((each) => each.id)).toEqual([
      'other',
      'allDay',
      'at8',
      'at15',
    ]);
  });
});

describe('which property places the cards, and where a card goes', () => {
  const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
  const start: PropertyTemplate = { id: 'start', name: 'Start', type: 'date', options: [] };
  const made: PropertyTemplate = { id: 'made', name: 'Made', type: 'createdTime', options: [] };
  const text: PropertyTemplate = { id: 'note', name: 'Note', type: 'text', options: [] };

  it('takes the property the view names, while it is a kind of date', () => {
    expect(displayProperty('start', [text, due, start])).toBe(start);
    expect(displayProperty('made', [text, due, made])).toBe(made);
  });

  it('falls back to the first date, then to any kind of date, then to nothing', () => {
    expect(displayProperty(undefined, [text, made, due, start])).toBe(due);
    expect(displayProperty('note', [text, due])).toBe(due);
    expect(displayProperty('gone', [text, made])).toBe(made);
    expect(displayProperty(undefined, [text])).toBeUndefined();
    expect(displayableProperties([text, made, due])).toEqual([made, due]);
  });

  function cardWith(value: unknown, createdAt = 0): Card {
    const card = createCard();
    card.id = 'card';
    card.createdAt = createdAt;
    if (value !== undefined) setCardProperty(card, 'due', value as string);
    return card;
  }

  it('puts a day without a time on that day for every reader', () => {
    const card = cardWith({ from: '2026-11-11' });
    for (const zone of ['Asia/Seoul', 'America/Los_Angeles', 'Pacific/Kiritimati']) {
      expect(cardDays(card, due, zone)).toEqual({ first: '2026-11-11', last: '2026-11-11' });
    }
  });

  it("puts a moment on the reader's day, and a range of moments may cover two days or one", () => {
    const meeting = cardWith({ from: at('Asia/Seoul', 2026, 11, 10, 8) });
    expect(cardDays(meeting, due, 'Asia/Seoul')?.first).toBe('2026-11-10');
    expect(cardDays(meeting, due, 'America/Los_Angeles')?.first).toBe('2026-11-09');
    const release = cardWith({
      from: at('Asia/Seoul', 2026, 11, 20, 22),
      to: at('Asia/Seoul', 2026, 11, 21, 6),
    });
    expect(cardDays(release, due, 'Asia/Seoul')).toEqual({
      first: '2026-11-20',
      last: '2026-11-21',
    });
    expect(cardDays(release, due, 'America/Los_Angeles')).toEqual({
      first: '2026-11-20',
      last: '2026-11-20',
    });
  });

  it('places a card by when it was made where the board has no date at all', () => {
    const card = cardWith(undefined, at('Asia/Seoul', 2026, 11, 3, 0, 30));
    expect(cardDays(card, undefined, 'Asia/Seoul')?.first).toBe('2026-11-03');
    expect(cardDays(card, undefined, 'UTC')?.first).toBe('2026-11-02');
    expect(cardDays(card, made, 'Asia/Seoul')?.first).toBe('2026-11-03');
  });

  it('keeps cards without a value apart, and the order of the rest', () => {
    const a = { ...cardWith({ from: '2026-11-02' }), id: 'a' };
    const b = { ...cardWith(undefined), id: 'b' };
    const c = { ...cardWith({ from: '2026-11-01', to: '2026-11-03' }), id: 'c' };
    const d = { ...cardWith('not a date'), id: 'd' };
    expect(spansOf([a, b, c, d], due, 'UTC')).toEqual({
      spans: [span('a', '2026-11-02'), span('c', '2026-11-01', '2026-11-03')],
      undated: ['b', 'd'],
    });
    // A value with a time carries the moment it starts; a time of creation does not.
    const timed = { ...cardWith({ from: at('UTC', 2026, 11, 2, 9) }), id: 't' };
    expect(spansOf([timed], due, 'UTC').spans[0]?.time).toBe(at('UTC', 2026, 11, 2, 9));
    expect(spansOf([timed], made, 'UTC').spans[0]?.time).toBeUndefined();
  });
});

describe('the names a calendar writes', () => {
  const november12 = calendarDate(2026, 11, 12);

  it("comes from the reader's language, and a date never moves with a zone", () => {
    expect(displayMonth(november12, 'en')).toBe('November 2026');
    expect(displayMonth(november12, 'ko')).toBe('2026년 11월');
    expect(displayDay(november12, 'en')).toBe('November 12');
    expect(displayDay(november12, 'ko')).toBe('11월 12일');
    expect(displayDayLong(november12, 'en')).toBe('Thursday, November 12');
    expect(displayDayLong(november12, 'ko')).toBe('11월 12일 목요일');
    expect(displayWeekday(november12, 'en')).toBe('Thu');
    expect(displayWeekday(november12, 'ko')).toBe('목');
  });

  it('writes the number of a day without the word for day', () => {
    expect(displayDayNumber(november12, 'ko')).toBe('12');
    expect(displayDayNumber(day('2026-11-01'), 'en')).toBe('1');
    expect(displayDayNumber(day('0099-03-01'), 'en')).toBe('1');
  });

  it('writes a range of days with what they share once', () => {
    expect(displayDayRange(day('2026-11-08'), day('2026-11-14'), 'en')).toMatch(
      /^November 8\s*–\s*14$/,
    );
    expect(displayDayRange(day('2026-11-30'), day('2026-12-04'), 'ko')).toMatch(
      /^11월 30일\s*~\s*12월 4일$/,
    );
  });

  it("writes a time on the reader's clock", () => {
    const eight = at('Asia/Seoul', 2026, 11, 10, 8);
    expect(displayTime(eight, { locale: 'en', timeZone: 'Asia/Seoul', hourCycle: 'h12' })).toBe(
      '8:00 AM',
    );
    expect(displayTime(eight, { locale: 'en', timeZone: 'Asia/Seoul', hourCycle: 'h23' })).toBe(
      '08:00',
    );
    expect(
      displayTime(eight, { locale: 'en', timeZone: 'America/Los_Angeles', hourCycle: 'h12' }),
    ).toBe('3:00 PM');
    expect(displayTime(Number.NaN, { locale: 'en', timeZone: 'UTC' })).toBe('');
  });
});
