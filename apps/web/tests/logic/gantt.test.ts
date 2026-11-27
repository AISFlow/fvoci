/**
 * The arithmetic of the Gantt view (lib/logic/gantt.ts): the axis for each
 * zoom and first day of the week, across the ends of months and years, its
 * marks, the rows, and where a bar lies. The edges and the arrows are in
 * gantt-arrows.test.ts.
 *
 * The suite runs with the process in three time zones (tools/test-zones.ts);
 * nothing here asks the machine for its own, so every result is the same.
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import { spansOf, weekdayOf } from '$lib/logic/calendar.ts';
import { addDays, calendarDate, daysBetween, type CalendarDate } from '$lib/logic/dates.ts';
import type { Days } from '$lib/logic/dependencies.ts';
import {
  axisOf,
  barBox,
  barsOf,
  DAY_WIDTH,
  GANTT_ZOOMS,
  MAX_AXIS_DAYS,
  MIN_BAR_WIDTH,
  rowsOf,
  scaleOf,
  xOfDay,
  zoomOf,
  type Axis,
} from '$lib/logic/gantt.ts';
import type { Weekday } from '$lib/logic/regional.ts';
import type { GanttZoom } from '$lib/logic/view-members.ts';
import { createCard, setCardProperty, type PropertyTemplate } from '$lib/logic/wire.ts';

const day = (text: string): CalendarDate => text as CalendarDate;
const days = (first: string, last = first): Days => ({ first: day(first), last: day(last) });
const SUNDAY = 7;
const MONDAY = 1;
const SATURDAY = 6;
const TODAY = day('2026-11-10'); // a Tuesday

function axis(zoom: GanttZoom, spans: Days[] = [], today = TODAY, weekStart: Weekday = SUNDAY) {
  return axisOf({ zoom, weekStart, today, spans });
}

const sum = (widths: { width: number }[]): number =>
  widths.reduce((total, each) => total + each.width, 0);

describe('the zoom', () => {
  it('is the word stored, and `week` for none or one that is not known', () => {
    expect(zoomOf('day')).toBe('day');
    expect(zoomOf('month')).toBe('month');
    expect(zoomOf(undefined)).toBe('week');
    expect(zoomOf('quarter')).toBe('week');
    expect(zoomOf(3)).toBe('week');
  });

  it('makes a day narrower at each step', () => {
    const widths = GANTT_ZOOMS.map((zoom) => DAY_WIDTH[zoom]);
    expect(widths).toEqual([...widths].sort((a, b) => b - a));
  });
});

describe('the axis', () => {
  it('has today on it when there are no cards, with whole weeks around it', () => {
    const made = axis('day');
    // A week of margin either side of Tuesday the 10th, out to whole weeks from Sunday.
    expect(made.first).toBe('2026-11-01');
    expect(made.last).toBe('2026-11-21');
    expect(made.days).toBe(21);
    expect(made.width).toBe(21 * DAY_WIDTH.day);
  });

  it('begins on the first day of the reader`s week', () => {
    expect(axis('day', [], TODAY, MONDAY).first).toBe('2026-11-02');
    expect(axis('day', [], TODAY, SATURDAY).first).toBe('2026-10-31');
    expect(axis('week', [], TODAY, MONDAY).first).toBe('2026-10-26');
    for (const weekStart of [SUNDAY, MONDAY, SATURDAY] as const) {
      const made = axis('week', [], TODAY, weekStart);
      expect(weekdayOf(made.first)).toBe(weekStart);
      expect(made.days % 7).toBe(0);
    }
  });

  it('reaches from the first card to the last, across the end of a year', () => {
    const made = axis('week', [days('2026-12-28', '2027-01-05'), days('2026-10-01')]);
    // Two weeks of margin: 17 September back to Sunday the 13th, 19 January on to Saturday the 23rd.
    expect(made.first).toBe('2026-09-13');
    expect(made.last).toBe('2027-01-23');
  });

  it('is whole months at the zoom `month`, February of a leap year too', () => {
    const made = axis('month', [days('2028-02-10', '2028-02-20')], day('2028-02-15'));
    // 31 days of margin: 10 January and 22 March.
    expect(made.first).toBe('2028-01-01');
    expect(made.last).toBe('2028-03-31');
    expect(made.days).toBe(31 + 29 + 31);
  });

  it('is as long as the page has room for, out to a whole week and no longer', () => {
    const room = (minDays: number, spans: Days[] = []): Axis =>
      axisOf({ zoom: 'day', weekStart: SUNDAY, today: TODAY, spans, minDays });
    // 60 days from 1 November end on 30 December, a Wednesday: on to Saturday 2 January.
    expect(room(60)).toMatchObject({ first: '2026-11-01', last: '2027-01-02', days: 63 });
    // Room for less than there is changes nothing.
    expect(room(10)).toEqual(axis('day'));
    const long = [days('2026-11-03', '2027-03-01')];
    expect(room(60, long)).toEqual(axis('day', long));
    // Room for more than can be drawn is the most that can be drawn.
    expect(room(1_000_000).days).toBe(MAX_AXIS_DAYS.day);
  });

  it('takes the room before today where the years that can be written end', () => {
    const made = axisOf({
      zoom: 'day',
      weekStart: SUNDAY,
      today: day('9999-12-30'),
      spans: [],
      minDays: 60,
    });
    expect(made.last).toBe('9999-12-31');
    expect(made.days).toBeGreaterThanOrEqual(60);
    expect(made.days).toBeLessThan(67);
    expect(weekdayOf(made.first)).toBe(SUNDAY);
  });

  it('keeps a month of margin at the zoom `month`, to the day', () => {
    // 31 days after 30 November is 31 December: the axis ends with the year.
    expect(axis('month', [days('2026-11-30')]).last).toBe('2026-12-31');
    // One day more is in January.
    expect(axis('month', [days('2026-12-01')]).last).toBe('2027-01-31');
  });

  it('is a window only when the cards and the margins are more than can be drawn', () => {
    const margin = 7;
    const fits = MAX_AXIS_DAYS.day - 2 * margin - 1;
    const whole = axis('day', [{ first: TODAY, last: addDays(TODAY, fits) }]);
    expect(whole.last >= addDays(TODAY, fits + margin)).toBe(true);
    const cut = axis('day', [{ first: TODAY, last: addDays(TODAY, fits + 400) }]);
    expect(cut.last < addDays(TODAY, fits + 400)).toBe(true);
    expect(cut.days).toBeLessThanOrEqual(MAX_AXIS_DAYS.day + 12);
  });

  it('is a window around today when the cards are too far apart to draw', () => {
    for (const zoom of GANTT_ZOOMS) {
      const made = axis(zoom, [days('0001-01-01'), days('9999-12-31')]);
      expect(made.first <= TODAY && TODAY <= made.last).toBe(true);
      // No more than the limit and the rounding out to whole weeks or months.
      expect(made.days).toBeLessThanOrEqual(MAX_AXIS_DAYS[zoom] + 62);
      expect(made.days).toBeGreaterThanOrEqual(MAX_AXIS_DAYS[zoom]);
      // Today is a third of the way in.
      const before = daysBetween(made.first, TODAY) / made.days;
      expect(before).toBeGreaterThan(0.3);
      expect(before).toBeLessThan(0.37);
    }
  });

  it('gives the whole window to the side that has cards', () => {
    const past = axis('day', [days('0001-01-01')]);
    expect(past.last).toBe('2026-11-21');
    expect(past.days).toBeGreaterThanOrEqual(MAX_AXIS_DAYS.day);
    const future = axis('day', [days('9999-12-31')]);
    expect(future.first).toBe('2026-11-01');
    expect(future.days).toBeGreaterThanOrEqual(MAX_AXIS_DAYS.day);
  });

  it('does not leave the years that can be written', () => {
    const early = axis('week', [], day('0001-01-03'));
    expect(early.first).toBe('0001-01-01');
    const late = axis('week', [], day('9999-12-30'));
    expect(late.last).toBe('9999-12-31');
    expect(axis('month', [], day('9999-12-30')).last).toBe('9999-12-31');
    // The marks of an axis cut short at an end still add up to it.
    for (const made of [early, late]) {
      const scale = scaleOf(made, SUNDAY);
      expect(sum(scale.lower)).toBeCloseTo(made.width);
      expect(sum(scale.upper)).toBeCloseTo(made.width);
    }
  });

  it('knows where a day begins, off the axis too', () => {
    const made = axis('day');
    expect(xOfDay(made, day('2026-11-01'))).toBe(0);
    expect(xOfDay(made, TODAY)).toBe(9 * DAY_WIDTH.day);
    expect(xOfDay(made, day('2026-10-31'))).toBe(-DAY_WIDTH.day);
  });
});

describe('the marks of the axis', () => {
  it('are days under months at the zoom `day`, with a line at each week', () => {
    // 25 October to 12 December: the margin of a card from 3 November to 2 December.
    const made = axis('day', [days('2026-11-03', '2026-12-02')]);
    const scale = scaleOf(made, SUNDAY);
    expect(scale.lower).toHaveLength(made.days);
    expect(scale.lower[1]).toEqual({ date: '2026-10-26', x: DAY_WIDTH.day, width: DAY_WIDTH.day });
    expect(scale.upper.map((tick) => tick.date)).toEqual([
      '2026-10-25',
      '2026-11-01',
      '2026-12-01',
    ]);
    expect(scale.upper.map((tick) => tick.width / DAY_WIDTH.day)).toEqual([7, 30, 12]);
    expect(scale.lines).toHaveLength(made.days / 7 - 1);
    expect(scale.lines[0]).toBe(7 * DAY_WIDTH.day);
  });

  it('are weeks under months at the zoom `week`', () => {
    const made = axis('week', [], TODAY, MONDAY);
    const scale = scaleOf(made, MONDAY);
    expect(scale.lower.map((tick) => tick.date)).toEqual([
      '2026-10-26',
      '2026-11-02',
      '2026-11-09',
      '2026-11-16',
      '2026-11-23',
    ]);
    expect(scale.lower.every((tick) => tick.width === 7 * DAY_WIDTH.week)).toBe(true);
    // A month begins in the middle of a week: its mark begins there too.
    expect(scale.upper.map((tick) => [tick.date, tick.x])).toEqual([
      ['2026-10-26', 0],
      ['2026-11-01', 6 * DAY_WIDTH.week],
    ]);
    expect(scale.lines).toEqual(scale.lower.slice(1).map((tick) => tick.x));
  });

  it('are months under years at the zoom `month`, each as wide as its days', () => {
    const made = axis('month', [days('2027-12-20', '2028-03-05')], day('2028-01-15'));
    const scale = scaleOf(made, SUNDAY);
    expect(
      scale.lower.map((tick) => [tick.date, Math.round(tick.width / DAY_WIDTH.month)]),
    ).toEqual([
      ['2027-11-01', 30],
      ['2027-12-01', 31],
      ['2028-01-01', 31],
      ['2028-02-01', 29],
      ['2028-03-01', 31],
      ['2028-04-01', 30],
    ]);
    expect(scale.upper.map((tick) => tick.date)).toEqual(['2027-11-01', '2028-01-01']);
    expect(scale.upper[0]?.width).toBeCloseTo(61 * DAY_WIDTH.month);
  });

  it('add up to the axis and keep today on it, whatever the cards', () => {
    const date = fc
      .integer({ min: -4000, max: 4000 })
      .map((offset) => addDays(calendarDate(2026, 1, 1), offset));
    const span = fc.tuple(date, fc.nat(400)).map(([first, length]) => ({
      first,
      last: addDays(first, length),
    }));
    fc.assert(
      fc.property(
        fc.constantFrom(...GANTT_ZOOMS),
        fc.constantFrom<Weekday>(SUNDAY, MONDAY, SATURDAY),
        date,
        fc.array(span, { maxLength: 6 }),
        (zoom, weekStart, today, spans) => {
          const made = axisOf({ zoom, weekStart, today, spans });
          const scale = scaleOf(made, weekStart);
          expect(made.first <= today && today <= made.last).toBe(true);
          expect(made.days).toBe(daysBetween(made.first, made.last) + 1);
          expect(sum(scale.lower)).toBeCloseTo(made.width, 6);
          expect(sum(scale.upper)).toBeCloseTo(made.width, 6);
          if (zoom === 'month') expect(made.first.slice(8)).toBe('01');
          else expect(weekdayOf(made.first)).toBe(weekStart);
          for (const each of spans) {
            const box = barBox(made, each);
            expect(box.x).toBeGreaterThanOrEqual(0);
            expect(box.x + box.width).toBeLessThanOrEqual(made.width + 1e-9);
          }
        },
      ),
    );
  });
});

describe('rows', () => {
  it('are the cards in the order given where the view is not grouped', () => {
    expect(rowsOf([{ cardIds: ['a', 'b'] }])).toEqual([
      { kind: 'card', id: 'a', row: 0 },
      { kind: 'card', id: 'b', row: 1 },
    ]);
  });

  it('have a heading for each group, an empty one too', () => {
    expect(
      rowsOf([
        { groupId: 'todo', cardIds: ['a'] },
        { groupId: '', cardIds: [] },
        { groupId: 'done', cardIds: ['b', 'c'] },
      ]),
    ).toEqual([
      { kind: 'group', id: 'todo', row: 0, count: 1 },
      { kind: 'card', id: 'a', row: 1 },
      { kind: 'group', id: '', row: 2, count: 0 },
      { kind: 'group', id: 'done', row: 3, count: 2 },
      { kind: 'card', id: 'b', row: 4 },
      { kind: 'card', id: 'c', row: 5 },
    ]);
  });
});

describe('a bar', () => {
  const made: Axis = axis('day'); // 1 to 21 November

  it('of one day is one day wide, on its day', () => {
    expect(barBox(made, days('2026-11-10'))).toEqual({
      x: 9 * DAY_WIDTH.day,
      width: DAY_WIDTH.day,
      fromBefore: false,
      toAfter: false,
    });
  });

  it('of several days reaches from the start of the first to the end of the last', () => {
    const box = barBox(made, days('2026-11-03', '2026-11-05'));
    expect(box.x).toBe(2 * DAY_WIDTH.day);
    expect(box.width).toBe(3 * DAY_WIDTH.day);
  });

  it('is cut at the edge of the axis and says which end is cut', () => {
    expect(barBox(made, days('2026-10-20', '2026-11-02'))).toEqual({
      x: 0,
      width: 2 * DAY_WIDTH.day,
      fromBefore: true,
      toAfter: false,
    });
    const after = barBox(made, days('2026-11-20', '2027-01-01'));
    expect(after).toMatchObject({ x: 19 * DAY_WIDTH.day, fromBefore: false, toAfter: true });
    expect(after.x + after.width).toBe(made.width);
    expect(barBox(made, days('2026-01-01', '2027-01-01'))).toEqual({
      x: 0,
      width: made.width,
      fromBefore: true,
      toAfter: true,
    });
  });

  it('that begins on the first day of the axis or ends on its last is not cut', () => {
    expect(barBox(made, { first: made.first, last: made.first })).toEqual({
      x: 0,
      width: DAY_WIDTH.day,
      fromBefore: false,
      toAfter: false,
    });
    expect(barBox(made, { first: made.last, last: made.last })).toEqual({
      x: made.width - DAY_WIDTH.day,
      width: DAY_WIDTH.day,
      fromBefore: false,
      toAfter: false,
    });
    expect(barBox(made, { first: addDays(made.first, -1), last: made.first }).fromBefore).toBe(
      true,
    );
    expect(barBox(made, { first: made.last, last: addDays(made.last, 1) }).toAfter).toBe(true);
  });

  it('wholly off the axis keeps a stub at the edge it is beyond', () => {
    expect(barBox(made, days('2025-03-01', '2025-03-09'))).toEqual({
      x: 0,
      width: MIN_BAR_WIDTH,
      fromBefore: true,
      toAfter: false,
    });
    expect(barBox(made, days('2027-03-01'))).toEqual({
      x: made.width - MIN_BAR_WIDTH,
      width: MIN_BAR_WIDTH,
      fromBefore: false,
      toAfter: true,
    });
  });

  it('is never too narrow to see: a day at the zoom `month` is widened', () => {
    const months = axis('month');
    const box = barBox(months, days('2026-11-10'));
    expect(box.width).toBe(MIN_BAR_WIDTH);
    expect(box.x).toBeCloseTo(xOfDay(months, TODAY));
    // The last day of the axis: the widened bar ends with the axis.
    const last = barBox(months, { first: months.last, last: months.last });
    expect(last.x + last.width).toBeCloseTo(months.width);
  });

  it('lies on the day a moment falls on for the reader', () => {
    const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
    const card = createCard();
    // 01:00 on the 10th in Seoul, 16:00 on the 9th in UTC, 06:00 on the 10th in Kiritimati.
    setCardProperty(card, 'due', { from: Date.parse('2026-11-09T16:00:00Z') });
    const at = (timeZone: string): number => {
      const [span] = spansOf([card], due, timeZone).spans;
      if (!span) throw new Error('the card has no days');
      return barBox(made, span).x / DAY_WIDTH.day;
    };
    expect(at('Asia/Seoul')).toBe(9);
    expect(at('UTC')).toBe(8);
    expect(at('Pacific/Kiritimati')).toBe(9);
  });

  it('is in the row of its card, and a card without days has none', () => {
    const rows = rowsOf([{ groupId: 'g', cardIds: ['a', 'undated', 'b'] }]);
    const spans: Record<string, Days> = { a: days('2026-11-03'), b: days('2026-11-04') };
    const bars = barsOf(made, rows, (id) => spans[id]);
    expect([...bars.keys()]).toEqual(['a', 'b']);
    expect(bars.get('a')).toMatchObject({ id: 'a', row: 1, x: 2 * DAY_WIDTH.day });
    expect(bars.get('b')?.row).toBe(3);
  });
});
