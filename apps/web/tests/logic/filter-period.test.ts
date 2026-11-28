/**
 * The relative periods of a date condition (`isWithin`): the days each
 * covers for a reader, by their today and the day their week starts on, and
 * which cards a condition on one lets through; and the pseudo property
 * `done`, by the board's rule of done.
 */
import { describe, expect, it } from 'vitest';

import { calendarDate, type CalendarDate } from '$lib/logic/dates.ts';
import { isClauseMet, type FilterContext } from '$lib/logic/filter.ts';
import { DONE_FILTER_ID, doneRuleOf, isCardDone } from '$lib/logic/filter-done.ts';
import {
  daysOfPeriod,
  FILTER_PERIODS,
  isFilterPeriod,
  type FilterPeriod,
} from '$lib/logic/filter-period.ts';
import type { Weekday } from '$lib/logic/regional.ts';
import {
  createCard,
  setCardProperty,
  type Card,
  type DoneRule,
  type FilterClause,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

import { at } from '../support/zones.ts';

const day = (text: string): CalendarDate => text as CalendarDate;
const span = (first: string, last: string) => ({ first: day(first), last: day(last) });

/** Friday 27 November 2026. */
const FRIDAY = calendarDate(2026, 11, 27);
/** Sunday 29 November 2026: the last day of a week that starts on Monday, the first of one that starts on Sunday. */
const SUNDAY = calendarDate(2026, 11, 29);
const MONDAY: Weekday = 1;
const SATURDAY: Weekday = 6;
const SUNDAY_START: Weekday = 7;

describe('the days of a period', () => {
  it('are the closed list of eight, and nothing else is one', () => {
    expect(FILTER_PERIODS).toEqual([
      'thisWeek',
      'nextWeek',
      'lastWeek',
      'thisMonth',
      'next7Days',
      'next30Days',
      'last7Days',
      'last30Days',
    ]);
    expect(isFilterPeriod('thisWeek')).toBe(true);
    for (const other of ['today', '2026-11-27', 'ThisWeek', 'thisYear', '', undefined]) {
      expect(isFilterPeriod(other)).toBe(false);
    }
  });

  it('of the weeks follow the day the week starts on', () => {
    const cases: [FilterPeriod, Weekday, ReturnType<typeof span>][] = [
      ['thisWeek', MONDAY, span('2026-11-23', '2026-11-29')],
      ['nextWeek', MONDAY, span('2026-11-30', '2026-12-06')],
      ['lastWeek', MONDAY, span('2026-11-16', '2026-11-22')],
      ['thisWeek', SUNDAY_START, span('2026-11-22', '2026-11-28')],
      ['nextWeek', SUNDAY_START, span('2026-11-29', '2026-12-05')],
      ['lastWeek', SUNDAY_START, span('2026-11-15', '2026-11-21')],
      // A week that starts on the day today is: today is its first day.
      ['thisWeek', 5, span('2026-11-27', '2026-12-03')],
      ['thisWeek', SATURDAY, span('2026-11-21', '2026-11-27')],
    ];
    for (const [period, weekStart, expected] of cases) {
      expect(daysOfPeriod(period, FRIDAY, weekStart), `${period} from ${weekStart}`).toEqual(
        expected,
      );
    }
  });

  it('of a week on the edge of one: the last day of a week from Monday, the first of one from Sunday', () => {
    expect(daysOfPeriod('thisWeek', SUNDAY, MONDAY)).toEqual(span('2026-11-23', '2026-11-29'));
    expect(daysOfPeriod('thisWeek', SUNDAY, SUNDAY_START)).toEqual(
      span('2026-11-29', '2026-12-05'),
    );
    expect(daysOfPeriod('lastWeek', SUNDAY, SUNDAY_START)).toEqual(
      span('2026-11-22', '2026-11-28'),
    );
    expect(daysOfPeriod('nextWeek', SUNDAY, MONDAY)).toEqual(span('2026-11-30', '2026-12-06'));
  });

  it('of a month are those of today’s month, and of days counted include today', () => {
    expect(daysOfPeriod('thisMonth', FRIDAY, MONDAY)).toEqual(span('2026-11-01', '2026-11-30'));
    expect(daysOfPeriod('thisMonth', calendarDate(2028, 2, 10), MONDAY)).toEqual(
      span('2028-02-01', '2028-02-29'),
    );
    expect(daysOfPeriod('next7Days', FRIDAY, MONDAY)).toEqual(span('2026-11-27', '2026-12-03'));
    expect(daysOfPeriod('next30Days', FRIDAY, MONDAY)).toEqual(span('2026-11-27', '2026-12-26'));
    expect(daysOfPeriod('last7Days', FRIDAY, MONDAY)).toEqual(span('2026-11-21', '2026-11-27'));
    expect(daysOfPeriod('last30Days', FRIDAY, MONDAY)).toEqual(span('2026-10-29', '2026-11-27'));
    // Counted days do not care where the week starts.
    expect(daysOfPeriod('next7Days', FRIDAY, SUNDAY_START)).toEqual(
      daysOfPeriod('next7Days', FRIDAY, MONDAY),
    );
  });
});

const TEMPLATES: PropertyTemplate[] = [
  { id: 'due', name: 'Due', type: 'date', options: [] },
  { id: 'made', name: 'Made', type: 'createdTime', options: [] },
  { id: 'note', name: 'Note', type: 'text', options: [] },
  {
    id: 'status',
    name: 'Status',
    type: 'select',
    options: [
      { id: 'todo', label: 'To do', color: 'blue' },
      { id: 'done', label: 'Done', color: 'green' },
    ],
  },
  { id: 'tick', name: 'Tick', type: 'checkbox', options: [] },
];

function cardWith(properties: Record<string, PropertyValue>, createdAt = 0): Card {
  const card = createCard();
  for (const [id, value] of Object.entries(properties)) setCardProperty(card, id, value);
  card.createdAt = createdAt;
  return card;
}

const within = (propertyId: string, period: string): FilterClause => ({
  propertyId,
  condition: 'isWithin',
  values: [period],
});

/** A reader in Seoul at noon on Friday 27 November 2026, whose week starts on Monday. */
const seoul: FilterContext = {
  timeZone: 'Asia/Seoul',
  now: at('Asia/Seoul', 2026, 11, 27, 12),
  weekStart: MONDAY,
};

describe('a condition "is within" a period', () => {
  it('lets a date through that has a day of the period, a range by any of its days', () => {
    const met = (due: PropertyValue, period: FilterPeriod, context = seoul): boolean =>
      isClauseMet(within('due', period), TEMPLATES, cardWith({ due }), context);
    expect(met({ from: '2026-11-23' }, 'thisWeek')).toBe(true);
    expect(met({ from: '2026-11-29' }, 'thisWeek')).toBe(true);
    expect(met({ from: '2026-11-30' }, 'thisWeek')).toBe(false);
    expect(met({ from: '2026-11-22' }, 'thisWeek')).toBe(false);
    expect(met({ from: '2026-11-20', to: '2026-11-23' }, 'thisWeek')).toBe(true);
    expect(met({ from: '2026-11-30' }, 'nextWeek')).toBe(true);
    expect(met({ from: '2026-11-22' }, 'lastWeek')).toBe(true);
    expect(met({ from: '2026-11-01' }, 'thisMonth')).toBe(true);
    expect(met({ from: '2026-12-01' }, 'thisMonth')).toBe(false);
    expect(met({ from: '2026-12-03' }, 'next7Days')).toBe(true);
    expect(met({ from: '2026-12-04' }, 'next7Days')).toBe(false);
    expect(met({ from: '2026-11-21' }, 'last7Days')).toBe(true);
    expect(met({ from: '2026-11-20' }, 'last7Days')).toBe(false);
    expect(met({ from: '2026-10-29' }, 'last30Days')).toBe(true);
    expect(met({ from: '2026-12-26' }, 'next30Days')).toBe(true);
    expect(met({ from: '2026-12-27' }, 'next30Days')).toBe(false);
  });

  it('reads the week where the reader’s week starts: Sunday 22 is this week from Sunday, last week from Monday', () => {
    const card = cardWith({ due: { from: '2026-11-22' } });
    const fromSunday = { ...seoul, weekStart: SUNDAY_START };
    expect(isClauseMet(within('due', 'thisWeek'), TEMPLATES, card, fromSunday)).toBe(true);
    expect(isClauseMet(within('due', 'thisWeek'), TEMPLATES, card, seoul)).toBe(false);
    expect(isClauseMet(within('due', 'lastWeek'), TEMPLATES, card, seoul)).toBe(true);
    // Without a day the week starts on, it starts on Monday.
    const { weekStart: _, ...mondays } = seoul;
    expect(isClauseMet(within('due', 'lastWeek'), TEMPLATES, card, mondays)).toBe(true);
  });

  it('reads a moment on the day it is for the reader, and today as the reader’s today', () => {
    // 00:30 on Monday 30 November in Seoul is still Sunday 29 in London.
    const card = cardWith({}, at('Asia/Seoul', 2026, 11, 30, 0, 30));
    const london: FilterContext = { ...seoul, timeZone: 'Europe/London' };
    expect(isClauseMet(within('made', 'thisWeek'), TEMPLATES, card, seoul)).toBe(false);
    expect(isClauseMet(within('made', 'nextWeek'), TEMPLATES, card, seoul)).toBe(true);
    expect(isClauseMet(within('made', 'thisWeek'), TEMPLATES, card, london)).toBe(true);
    // At 23:30 on Sunday 29 November in Seoul, Monday 30 is still next week, and in the next seven days.
    const late = { ...seoul, now: at('Asia/Seoul', 2026, 11, 29, 23, 30) };
    const due = cardWith({ due: { from: '2026-11-30' } });
    expect(isClauseMet(within('due', 'thisWeek'), TEMPLATES, due, late)).toBe(false);
    expect(isClauseMet(within('due', 'next7Days'), TEMPLATES, due, late)).toBe(true);
  });

  it('lets every card through while it has no period, or a value that is not one', () => {
    const card = cardWith({ due: { from: '2026-01-01' } });
    for (const values of [[], ['2026-11-27'], ['today'], ['thisYear']]) {
      expect(isClauseMet({ ...within('due', ''), values }, TEMPLATES, card, seoul)).toBe(true);
    }
    // Not a date at all.
    expect(isClauseMet(within('note', 'thisWeek'), TEMPLATES, card, seoul)).toBe(true);
  });

  it('lets no card through that has no date', () => {
    expect(isClauseMet(within('due', 'thisWeek'), TEMPLATES, cardWith({}), seoul)).toBe(false);
  });

  it('is read by another condition as no value: a period under "is" lets every card through', () => {
    const card = cardWith({ due: { from: '2026-01-01' } });
    const is: FilterClause = { propertyId: 'due', condition: 'is', values: ['thisWeek'] };
    expect(isClauseMet(is, TEMPLATES, card, seoul)).toBe(true);
  });
});

describe('the pseudo property "done"', () => {
  const byOption: DoneRule = { kind: 'option', propertyId: 'status', optionIds: ['done'] };
  const byBox: DoneRule = { kind: 'checkbox', propertyId: 'tick' };
  const done = (condition: 'isSet' | 'isNotSet'): FilterClause => ({
    propertyId: DONE_FILTER_ID,
    condition,
    values: [],
  });

  it('is set on a card done by the board’s rule, and not set on another', () => {
    const finished = cardWith({ status: 'done', tick: 'true' });
    const open = cardWith({ status: 'todo' });
    for (const rule of [byOption, byBox]) {
      const context = { ...seoul, doneWhen: rule };
      expect(isClauseMet(done('isSet'), TEMPLATES, finished, context)).toBe(true);
      expect(isClauseMet(done('isSet'), TEMPLATES, open, context)).toBe(false);
      expect(isClauseMet(done('isNotSet'), TEMPLATES, open, context)).toBe(true);
      expect(isClauseMet(done('isNotSet'), TEMPLATES, finished, context)).toBe(false);
    }
  });

  it('reads the options of a multi-select, and a rule read without its kind as one of options', () => {
    const multi = cardWith({ status: ['todo', 'done'] });
    expect(isCardDone(byOption, multi)).toBe(true);
    const kindless = { propertyId: 'status', optionIds: ['done'] } as unknown as DoneRule;
    expect(isCardDone(kindless, cardWith({ status: 'done' }))).toBe(true);
  });

  it('lets every card through on a board without a rule, or with one on a property it lost', () => {
    const card = cardWith({ status: 'todo' });
    expect(isClauseMet(done('isSet'), TEMPLATES, card, seoul)).toBe(true);
    const lost: DoneRule = { kind: 'checkbox', propertyId: 'gone' };
    expect(doneRuleOf(lost, TEMPLATES)).toBeUndefined();
    expect(isClauseMet(done('isSet'), TEMPLATES, card, { ...seoul, doneWhen: lost })).toBe(true);
    expect(isClauseMet(done('isSet'), TEMPLATES, card, { ...seoul, doneWhen: null })).toBe(true);
  });
});
