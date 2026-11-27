/**
 * The reader's time zone is an argument, never the time zone of the machine.
 *
 * - A moment (a card's creation, a date with a time) is on the day the
 *   reader's clock shows. Days are not all 24 hours long: where clocks
 *   change, one day a year has 23 hours and one has 25.
 * - Near the date line the same moment is on three different dates.
 * - A date without a time is a calendar date: the same for every reader.
 *
 * Every case names its time zone. The whole suite also runs with the
 * process in UTC, Asia/Seoul and Pacific/Kiritimati (tools/test-zones.ts);
 * since nothing here asks the machine for its time zone, each result must be
 * the same in all three.
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import { CALCULATIONS } from '$lib/logic/calculations.ts';
import {
  addDays,
  calendarDate,
  dateOfInstant,
  daysOf,
  endOfDay,
  startOfDay,
  type CalendarDate,
} from '$lib/logic/dates.ts';
import { isClauseMet } from '$lib/logic/filter.ts';
import { displayCalendarDate, displayDate, displayDateTime, inputDate } from '$lib/logic/format.ts';
import { PROPERTY_BEHAVIORS } from '$lib/logic/properties.ts';
import { sortCards } from '$lib/logic/sort.ts';
import {
  createCard,
  createFilterClause,
  formatFilterDate,
  setCardProperty,
  type Card,
  type FilterCondition,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

import { storedDate } from './support/factory.ts';

import { at, ZONES } from '../support/zones.ts';

const HOUR = 3_600_000;

const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
const created: PropertyTemplate = {
  id: 'created',
  name: 'Created',
  type: 'createdTime',
  options: [],
};
const templates = [due, created];

function cardCreatedAt(instant: number, id = 'card'): Card {
  const card = createCard();
  card.id = id;
  card.createdAt = instant;
  return card;
}

function cardDue(date: CalendarDate, id = 'card'): Card {
  const card = createCard();
  card.id = id;
  setCardProperty(card, 'due', storedDate({ from: { date } }));
  return card;
}

const filters = (
  propertyId: string,
  condition: FilterCondition,
  day: CalendarDate,
  card: Card,
  timeZone: string,
): boolean =>
  isClauseMet(
    createFilterClause({ propertyId, condition, values: [formatFilterDate(day)] }),
    templates,
    card,
    { timeZone },
  );

describe('a day where the clocks change', () => {
  // New York, 2026: clocks go forward on 8 March and back on 1 November.
  const newYork = 'America/New_York';
  const short = calendarDate(2026, 3, 8);
  const long = calendarDate(2026, 11, 1);

  it('has 23 hours when the clocks go forward', () => {
    expect(startOfDay(short, newYork)).toBe(Date.UTC(2026, 2, 8, 5)); // 00:00 EST
    expect(endOfDay(short, newYork)).toBe(Date.UTC(2026, 2, 9, 4)); // 00:00 EDT
    expect(endOfDay(short, newYork) - startOfDay(short, newYork)).toBe(23 * HOUR);
  });

  it('has 25 hours when the clocks go back', () => {
    expect(startOfDay(long, newYork)).toBe(Date.UTC(2026, 10, 1, 4)); // 00:00 EDT
    expect(endOfDay(long, newYork)).toBe(Date.UTC(2026, 10, 2, 5)); // 00:00 EST
    expect(endOfDay(long, newYork) - startOfDay(long, newYork)).toBe(25 * HOUR);
  });

  it('keeps every moment of the 25 hours on that day, and no other', () => {
    const start = startOfDay(long, newYork);
    for (const instant of [start, start + 24 * HOUR, start + 25 * HOUR - 1]) {
      expect(dateOfInstant(instant, newYork)).toBe(long);
      expect(filters('created', 'is', long, cardCreatedAt(instant), newYork)).toBe(true);
    }
    expect(dateOfInstant(start - 1, newYork)).toBe('2026-10-31');
    expect(dateOfInstant(start + 25 * HOUR, newYork)).toBe('2026-11-02');
    expect(filters('created', 'is', long, cardCreatedAt(start + 25 * HOUR), newYork)).toBe(false);
    expect(filters('created', 'isAfter', long, cardCreatedAt(start + 25 * HOUR), newYork)).toBe(
      true,
    );
    // The hour that happens twice (01:30, first as EDT and then as EST) is one day.
    expect(dateOfInstant(Date.UTC(2026, 10, 1, 5, 30), newYork)).toBe(long);
    expect(dateOfInstant(Date.UTC(2026, 10, 1, 6, 30), newYork)).toBe(long);
  });

  it('ends the 23 hours an hour early', () => {
    const start = startOfDay(short, newYork);
    expect(dateOfInstant(start + 23 * HOUR - 1, newYork)).toBe(short);
    expect(dateOfInstant(start + 23 * HOUR, newYork)).toBe('2026-03-09');
    expect(filters('created', 'is', short, cardCreatedAt(start + 23 * HOUR - 1), newYork)).toBe(
      true,
    );
    expect(filters('created', 'is', short, cardCreatedAt(start + 23 * HOUR), newYork)).toBe(false);
    expect(
      filters('created', 'isBefore', '2026-03-09' as CalendarDate, cardCreatedAt(start), newYork),
    ).toBe(true);
  });

  it('begins at the first moment there is when midnight itself is skipped', () => {
    // Havana, 8 March 2026: at 00:00 the clocks jump to 01:00.
    const havana = 'America/Havana';
    const start = startOfDay(short, havana);
    expect(start).toBe(Date.UTC(2026, 2, 8, 5));
    expect(dateOfInstant(start, havana)).toBe(short);
    expect(dateOfInstant(start - 1, havana)).toBe('2026-03-07');
    expect(endOfDay(short, havana) - start).toBe(23 * HOUR);
  });

  it('changes by half an hour on Lord Howe Island', () => {
    // Clocks go forward 30 minutes on 4 October 2026.
    const lordHowe = 'Australia/Lord_Howe';
    const day = calendarDate(2026, 10, 4);
    expect(endOfDay(day, lordHowe) - startOfDay(day, lordHowe)).toBe(23.5 * HOUR);
  });

  it('counts the days of a date property by the calendar, not by 24 hours', () => {
    // 23:30 on the 7th and 00:30 on the 9th in New York are 24 hours apart
    // and on three calendar days: the 7th, the short 8th and the 9th.
    const from = at(newYork, 2026, 3, 7, 23, 30);
    const to = at(newYork, 2026, 3, 9, 0, 30);
    expect(to - from).toBe(24 * HOUR);
    const timed = (instant: number): Card => {
      const card = createCard();
      setCardProperty(card, 'due', storedDate({ from: { instant } }));
      return card;
    };
    const reader = { locale: 'en', timeZone: newYork };
    expect(CALCULATIONS.dateRange([timed(from), timed(to)], due, reader)).toBe('3 days');
    // For a reader in Seoul the same two moments are on the 8th and the 9th.
    expect(
      CALCULATIONS.dateRange([timed(from), timed(to)], due, { ...reader, timeZone: 'Asia/Seoul' }),
    ).toBe('2 days');
    // Between the moments two cards were created the range is the time that passed.
    expect(
      CALCULATIONS.dateRange([cardCreatedAt(from, 'a'), cardCreatedAt(to, 'b')], created, reader),
    ).toBe('1 day');
  });

  it('holds for any moment in any zone: a moment lies within its own day', () => {
    fc.assert(
      fc.property(
        fc.constantFrom(...ZONES),
        // 2000 to 2040: covers the dates on which each of these zones changed its clocks.
        fc.integer({ min: Date.UTC(2000, 0, 1), max: Date.UTC(2040, 0, 1) }),
        (timeZone, instant) => {
          const day = dateOfInstant(instant, timeZone);
          expect(startOfDay(day, timeZone)).toBeLessThanOrEqual(instant);
          expect(endOfDay(day, timeZone)).toBeGreaterThan(instant);
          // The moment before the day begins is the day before.
          expect(dateOfInstant(startOfDay(day, timeZone) - 1, timeZone)).toBe(addDays(day, -1));
          expect(dateOfInstant(endOfDay(day, timeZone), timeZone)).toBe(addDays(day, 1));
          const length = endOfDay(day, timeZone) - startOfDay(day, timeZone);
          expect([23, 23.5, 24, 24.5, 25]).toContain(length / HOUR);
        },
      ),
      { numRuns: 500 },
    );
  });
});

describe('near the date line', () => {
  const kiritimati = 'Pacific/Kiritimati'; // UTC+14
  const pagoPago = 'Pacific/Pago_Pago'; // UTC-11
  const bakerIsland = 'Etc/GMT+12'; // UTC-12

  it('puts one moment on three dates', () => {
    const moment = Date.UTC(2026, 2, 5, 10, 30);
    expect(dateOfInstant(moment, kiritimati)).toBe('2026-03-06'); // 00:30
    expect(dateOfInstant(moment, 'UTC')).toBe('2026-03-05');
    expect(dateOfInstant(moment, pagoPago)).toBe('2026-03-04'); // 23:30
    expect(dateOfInstant(moment, bakerIsland)).toBe('2026-03-04'); // 22:30
  });

  it('starts the same date 26 hours apart on the two sides', () => {
    const fifth = calendarDate(2026, 3, 5);
    expect(startOfDay(fifth, kiritimati)).toBe(Date.UTC(2026, 2, 4, 10));
    expect(startOfDay(fifth, bakerIsland)).toBe(Date.UTC(2026, 2, 5, 12));
    expect(startOfDay(fifth, bakerIsland) - startOfDay(fifth, kiritimati)).toBe(26 * HOUR);
    // The 5th is over in Kiritimati before it has begun on Baker Island.
    expect(endOfDay(fifth, kiritimati)).toBeLessThan(startOfDay(fifth, bakerIsland));
  });

  it('filters a card by the date its reader sees', () => {
    const card = cardCreatedAt(Date.UTC(2026, 2, 5, 10, 30));
    const sixth = calendarDate(2026, 3, 6);
    const fourth = calendarDate(2026, 3, 4);
    expect(filters('created', 'is', sixth, card, kiritimati)).toBe(true);
    expect(filters('created', 'is', sixth, card, pagoPago)).toBe(false);
    expect(filters('created', 'is', fourth, card, pagoPago)).toBe(true);
    expect(filters('created', 'isBefore', sixth, card, pagoPago)).toBe(true);
    expect(filters('created', 'isBefore', sixth, card, kiritimati)).toBe(false);
    expect(filters('created', 'isAfter', fourth, card, kiritimati)).toBe(true);
    expect(filters('created', 'isAfter', fourth, card, bakerIsland)).toBe(false);
  });

  it("shows the moment on each reader's clock", () => {
    const moment = Date.UTC(2026, 2, 5, 10, 30);
    const now = Date.UTC(2026, 5, 1);
    const show = (timeZone: string): string =>
      displayDateTime(moment, { locale: 'en', timeZone, now });
    expect(show(kiritimati)).toBe('March 06 at 12:30 AM');
    expect(show('Asia/Seoul')).toBe('March 05 at 7:30 PM');
    expect(show(pagoPago)).toBe('March 04 at 11:30 PM');
    expect(displayDate(moment, { locale: 'en', timeZone: kiritimati, now })).toBe('March 06');
    expect(inputDate(moment, { locale: 'en-US', timeZone: pagoPago })).toBe('03/04/2026');
  });

  it('decides "this year" on the reader\'s calendar', () => {
    // 31 December 2025, 12:00 UTC: already 2026 in Kiritimati, still 2025 in Pago Pago.
    const now = Date.UTC(2025, 11, 31, 12);
    const newYearsDay = calendarDate(2026, 1, 1);
    expect(displayCalendarDate(newYearsDay, { locale: 'en', timeZone: kiritimati, now })).toBe(
      'January 01',
    );
    expect(displayCalendarDate(newYearsDay, { locale: 'en', timeZone: pagoPago, now })).toBe(
      'January 01, 2026',
    );
    // Crossing into 2026 in the same zone invalidates any cached year bounds.
    const nextYear = Date.UTC(2026, 5, 1);
    expect(
      displayCalendarDate(newYearsDay, { locale: 'en', timeZone: pagoPago, now: nextYear }),
    ).toBe('January 01');
    expect(
      displayDate(Date.UTC(2025, 6, 9, 12), { locale: 'en', timeZone: 'UTC', now: nextYear }),
    ).toBe('July 09, 2025');
    expect(
      displayDateTime(Date.UTC(2026, 6, 9, 15, 20), {
        locale: 'en',
        timeZone: 'UTC',
        hourCycle: 'h23',
        now: nextYear,
      }),
    ).toBe('July 09 at 15:20');
  });

  it('knows a date that a place skipped has no moments', () => {
    // Samoa moved across the date line at the end of 29 December 2011: there was no 30th.
    const apia = 'Pacific/Apia';
    const skipped = calendarDate(2011, 12, 30);
    expect(endOfDay(skipped, apia)).toBe(startOfDay(skipped, apia));
    const lastOf29th = startOfDay(skipped, apia) - 1;
    expect(dateOfInstant(lastOf29th, apia)).toBe('2011-12-29');
    expect(dateOfInstant(lastOf29th + 1, apia)).toBe('2011-12-31');
  });
});

describe('at the ends of the years a date can have (1 to 9999)', () => {
  // The last minute of 9999 in UTC is in the year 10000 in Seoul.
  const lastMinute = Date.UTC(9999, 11, 31, 23, 59);
  const firstMoment = new Date(0).setUTCFullYear(1, 0, 1);

  it('a moment late on the last day of 9999 is on that day east of UTC, not in 10000', () => {
    expect(dateOfInstant(lastMinute, 'Asia/Seoul')).toBe('9999-12-31');
    expect(dateOfInstant(lastMinute, 'Pacific/Kiritimati')).toBe('9999-12-31');
    // Days compare as text: a range that ends then still ends after it starts.
    const days = daysOf(
      { from: { date: '9999-12-30' as CalendarDate }, to: { instant: lastMinute } },
      'Asia/Seoul',
    );
    expect(days).toEqual({ first: '9999-12-30', last: '9999-12-31' });
  });

  it('a moment early on the first day of the year 1 is on that day west of UTC, not a year on', () => {
    // Before it, for a reader in Los Angeles, is the year 1 before the era: read as "1".
    expect(dateOfInstant(firstMoment, 'America/Los_Angeles')).toBe('0001-01-01');
    expect(dateOfInstant(firstMoment, 'UTC')).toBe('0001-01-01');
    expect(dateOfInstant(firstMoment + 86_400_000, 'America/Los_Angeles')).toBe('0001-01-01');
  });
});

describe('a date without a time is the same date for every reader', () => {
  const fifth = calendarDate(2026, 3, 5);
  const now = Date.UTC(2026, 5, 1);

  it('covers the same day in every time zone', () => {
    const stored = storedDate({ from: { date: fifth } });
    for (const timeZone of ZONES) {
      expect(daysOf({ from: { date: fifth } }, timeZone)).toEqual({ first: fifth, last: fifth });
      expect(PROPERTY_BEHAVIORS.date.days?.(stored, createCard(), timeZone)).toEqual({
        first: fifth,
        last: fifth,
      });
    }
  });

  it('is shown as the same day in every time zone', () => {
    const stored = storedDate({ from: { date: fifth } });
    for (const timeZone of ZONES) {
      const context = { locale: 'en', timeZone, now };
      expect(displayCalendarDate(fifth, context)).toBe('March 05');
      expect(PROPERTY_BEHAVIORS.date.displayValue(stored, createCard(), due, context)).toBe(
        'March 05',
      );
      expect(CALCULATIONS.earliest([cardDue(fifth)], due, context)).toBe('March 05');
    }
  });

  it('passes the same filters in every time zone', () => {
    const card = cardDue(fifth);
    for (const timeZone of ZONES) {
      expect(filters('due', 'is', fifth, card, timeZone)).toBe(true);
      expect(filters('due', 'is', addDays(fifth, 1), card, timeZone)).toBe(false);
      expect(filters('due', 'is', addDays(fifth, -1), card, timeZone)).toBe(false);
      expect(filters('due', 'isBefore', addDays(fifth, 1), card, timeZone)).toBe(true);
      expect(filters('due', 'isBefore', fifth, card, timeZone)).toBe(false);
      expect(filters('due', 'isAfter', addDays(fifth, -1), card, timeZone)).toBe(true);
      expect(filters('due', 'isAfter', fifth, card, timeZone)).toBe(false);
    }
  });

  it('sorts the same in every time zone', () => {
    const cards = [
      cardDue(calendarDate(2026, 3, 6), 'c'),
      cardDue(fifth, 'b'),
      cardDue(calendarDate(2026, 3, 4), 'a'),
    ];
    for (const timeZone of ZONES) {
      const sorted = sortCards(cards, {
        templates,
        sort: [{ propertyId: 'due', reversed: false }],
        cardOrder: [],
        locale: 'en',
        timeZone,
      });
      expect(sorted.map((card) => card.id)).toEqual(['a', 'b', 'c']);
    }
  });

  it('holds for any date, any filter and any two readers', () => {
    const anyDay = fc
      .integer({ min: 0, max: 20_000 })
      .map((days) => addDays(calendarDate(2000, 1, 1), days));
    fc.assert(
      fc.property(
        anyDay,
        fc.integer({ min: -2, max: 2 }),
        fc.constantFrom<FilterCondition>('is', 'isBefore', 'isAfter'),
        fc.constantFrom(...ZONES),
        fc.constantFrom(...ZONES),
        (date, offset, condition, first, second) => {
          const card = cardDue(date);
          const asked = addDays(date, offset);
          expect(filters('due', condition, asked, card, first)).toBe(
            filters('due', condition, asked, card, second),
          );
          expect(daysOf({ from: { date } }, first)).toEqual(daysOf({ from: { date } }, second));
          const context = { locale: 'en', now };
          expect(displayCalendarDate(date, { ...context, timeZone: first })).toBe(
            displayCalendarDate(date, { ...context, timeZone: second }),
          );
        },
      ),
      { numRuns: 300 },
    );
  });

  it('but a date with a time is a moment, and does depend on the reader', () => {
    const timed = storedDate({ from: { instant: Date.UTC(2026, 2, 5, 20, 0) } });
    const days = PROPERTY_BEHAVIORS.date.days;
    expect(days?.(timed, createCard(), 'UTC')?.first).toBe('2026-03-05');
    expect(days?.(timed, createCard(), 'Asia/Seoul')?.first).toBe('2026-03-06');
  });
});
