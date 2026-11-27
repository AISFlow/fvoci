/**
 * The comparators of sort.ts are orderings, checked on generated cards: the
 * manual order (Q1 in docs/notes.md: no comparator answers "after" both ways
 * round) and the order by any combination of sort options.
 *
 * For every comparator `compare` and all cards a, b, c:
 *
 * - compare(a, a) is 0
 * - compare(a, b) and compare(b, a) have opposite signs
 * - if a comes before b and b before c, then a comes before c
 * - two different cards never compare as equal
 *
 * and therefore sorting gives the same result whatever order the cards
 * arrive in.
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import { calendarDate } from '$lib/logic/dates.ts';
import { cardComparator, manualComparator, sortCards, type SortContext } from '$lib/logic/sort.ts';
import {
  cardProperty,
  createCard,
  setCardProperty,
  TITLE_COLUMN_ID,
  type Card,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

import { storedDate } from './support/factory.ts';

const templates: PropertyTemplate[] = [
  { id: 'n', name: 'Number', type: 'number', options: [] },
  { id: 't', name: 'Text', type: 'text', options: [] },
  { id: 'd', name: 'Date', type: 'date', options: [] },
  {
    id: 's',
    name: 'Select',
    type: 'select',
    options: [
      { id: 'o1', label: 'beta', color: 'default' },
      { id: 'o2', label: 'alpha', color: 'default' },
    ],
  },
  { id: 'created', name: 'Created', type: 'createdTime', options: [] },
];

// Few distinct values on purpose: ties are where a comparator goes wrong.
const optional = <T>(arbitrary: fc.Arbitrary<T>): fc.Arbitrary<T | undefined> =>
  fc.option(arbitrary, { nil: undefined, freq: 3 });

const cardFields = fc.record({
  title: fc.constantFrom('', 'apple', 'Apple', 'banana', '가나', '다라'),
  createdAt: fc.integer({ min: 1, max: 4 }),
  n: optional(fc.constantFrom('1', '2', '10', '-3', 'abc', '')),
  t: optional(fc.constantFrom('x', 'y', 'Y', '')),
  d: optional(
    fc.oneof(
      fc
        .integer({ min: 1, max: 3 })
        .map((day) => storedDate({ from: { date: calendarDate(2026, 3, day) } })),
      fc
        .integer({ min: 0, max: 3 })
        .map((hours) => storedDate({ from: { instant: Date.UTC(2026, 2, 2, hours * 8) } })),
      fc.constant('not a date'),
    ),
  ),
  s: optional(fc.constantFrom('o1', 'o2', 'deleted')),
});

/** Cards with distinct identifiers. */
const cards = fc.array(cardFields, { minLength: 0, maxLength: 12 }).map((fields) =>
  fields.map((field, index): Card => {
    const card = createCard();
    card.id = `card-${index}`;
    card.title = field.title;
    card.createdAt = field.createdAt;
    for (const propertyId of ['n', 't', 'd', 's'] as const) {
      const value = field[propertyId];
      if (value !== undefined) setCardProperty(card, propertyId, value);
    }
    return card;
  }),
);

const sortArb = fc.array(
  fc.record({
    propertyId: fc.constantFrom(TITLE_COLUMN_ID, 'n', 't', 'd', 's', 'created', 'gone'),
    reversed: fc.boolean(),
  }),
  { minLength: 1, maxLength: 3 },
);

/** A manual order that names some cards, some twice, and some that do not exist. */
const cardOrder = fc
  .array(fc.integer({ min: 0, max: 14 }), { maxLength: 16 })
  .map((indexes) => indexes.map((index) => `card-${index}`));

function expectOrdering(compare: (first: Card, second: Card) => number, list: Card[]): void {
  for (const a of list) {
    expect(compare(a, a)).toBe(0);
    for (const b of list) {
      const ab = Math.sign(compare(a, b));
      expect(Math.sign(compare(b, a))).toBe(ab === 0 ? 0 : -ab);
      if (a !== b) expect(ab).not.toBe(0);
      for (const c of list) {
        if (ab < 0 && compare(b, c) < 0) expect(compare(a, c)).toBeLessThan(0);
      }
    }
  }
}

const ids = (list: readonly Card[]): string[] => list.map((card) => card.id);

describe('the manual order is an ordering', () => {
  it('for any cards and any stored order', () => {
    fc.assert(
      fc.property(cards, cardOrder, (list, order) => {
        expectOrdering(manualComparator(order), list);
      }),
      { numRuns: 300 },
    );
  });

  it('puts every placed card before every card without a position', () => {
    fc.assert(
      fc.property(cards, cardOrder, (list, order) => {
        const sorted = ids(
          sortCards(list, {
            templates,
            sort: [],
            cardOrder: order,
            locale: 'en',
            timeZone: 'UTC',
          }),
        );
        const placed = sorted.map((id) => order.includes(id));
        expect(placed).toEqual([...placed].sort((a, b) => Number(b) - Number(a)));
        // The placed ones are in the stored order.
        const positions = sorted.filter((id) => order.includes(id)).map((id) => order.indexOf(id));
        expect(positions).toEqual([...positions].sort((a, b) => a - b));
      }),
      { numRuns: 300 },
    );
  });

  it('sorts to the same result whatever order the cards arrive in', () => {
    fc.assert(
      fc.property(
        cards,
        cardOrder,
        fc.array(fc.nat(), { minLength: 12, maxLength: 12 }),
        (list, order, keys) => {
          const context: SortContext = {
            templates,
            sort: [],
            cardOrder: order,
            locale: 'en',
            timeZone: 'UTC',
          };
          const shuffled = list
            .map((card, index) => ({ card, key: keys[index] ?? 0 }))
            .sort((a, b) => a.key - b.key)
            .map(({ card }) => card);
          expect(ids(sortCards(shuffled, context))).toEqual(ids(sortCards(list, context)));
        },
      ),
      { numRuns: 300 },
    );
  });
});

describe('the order by sort options is an ordering', () => {
  it('for any cards and any options', () => {
    fc.assert(
      fc.property(cards, sortArb, (list, options) => {
        const context: SortContext = {
          templates,
          sort: options,
          cardOrder: [],
          locale: 'en',
          timeZone: 'UTC',
        };
        expectOrdering(cardComparator(context), list);
      }),
      { numRuns: 300 },
    );
  });

  it('sorts to the same result whatever order the cards arrive in', () => {
    fc.assert(
      fc.property(cards, sortArb, (list, options) => {
        const context: SortContext = {
          templates,
          sort: options,
          cardOrder: [],
          locale: 'en',
          timeZone: 'UTC',
        };
        expect(ids(sortCards([...list].reverse(), context))).toEqual(ids(sortCards(list, context)));
      }),
      { numRuns: 300 },
    );
  });

  it('keeps cards without a value after those with one, whichever way an option runs', () => {
    const hasValue: Record<string, (card: Card) => boolean> = {
      n: (card) => {
        const stored = cardProperty(card, 'n');
        return typeof stored === 'string' && stored.trim() !== '' && !Number.isNaN(Number(stored));
      },
      t: (card) => Boolean(cardProperty(card, 't')),
      s: (card) => {
        const stored = cardProperty(card, 's');
        return typeof stored === 'string' && ['o1', 'o2'].includes(stored);
      },
    };
    fc.assert(
      fc.property(
        cards,
        fc.constantFrom('n', 't', 's'),
        fc.boolean(),
        (list, propertyId, reversed) => {
          const sorted = sortCards(list, {
            templates,
            sort: [{ propertyId, reversed }],
            cardOrder: [],
            locale: 'en',
            timeZone: 'UTC',
          });
          const has = sorted.map((card) => hasValue[propertyId]?.(card) ?? false);
          // Once a card without a value has come, no card with one follows.
          expect(has).toEqual([...has].sort((a, b) => Number(b) - Number(a)));
        },
      ),
      { numRuns: 300 },
    );
  });
});

describe('the check itself', () => {
  /** A manual order that breaks Q1: a card without a position is "after" whichever way round it is asked. */
  const brokenManualOrder =
    (order: readonly string[]) =>
    (first: Card, second: Card): number => {
      const a = order.indexOf(first.id);
      const b = order.indexOf(second.id);
      if (a < 0 && b < 0)
        return first.title.localeCompare(second.title) || first.createdAt - second.createdAt;
      if (a < 0 && b >= 0) return 1;
      return a - b;
    };

  it('rejects a comparator that answers "after" both ways round', () => {
    const result = fc.check(
      fc.property(cards, cardOrder, (list, order) => {
        expectOrdering(brokenManualOrder(order), list);
      }),
      { numRuns: 300, seed: 1 },
    );
    expect(result.failed).toBe(true);
  });
});
