/**
 * The quick filters of a board: which chips it offers its reader, which
 * cards each lets through, chips together, and the cards let through
 * whatever the chips are.
 */
import { describe, expect, it } from 'vitest';

import {
  applyQuickFilters,
  isQuickFilterMet,
  offeredQuickFilters,
  QUICK_FILTERS,
  type QuickFilterBoard,
  type QuickFilterContext,
} from '$lib/logic/quick-filter.ts';
import {
  createCard,
  setCardProperty,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

import { at } from '../support/zones.ts';

const BOARD: QuickFilterBoard = {
  cardProperties: [
    {
      id: 'status',
      name: 'Status',
      type: 'select',
      options: [
        { id: 'todo', label: 'To do', color: 'blue' },
        { id: 'finished', label: 'Done', color: 'green' },
      ],
    },
    { id: 'owner', name: 'Owner', type: 'person', options: [] },
    { id: 'helpers', name: 'Helpers', type: 'multiPerson', options: [] },
    { id: 'due', name: 'Due', type: 'date', options: [] },
    { id: 'author', name: 'Author', type: 'createdBy', options: [] },
  ],
  doneWhen: { kind: 'option', propertyId: 'status', optionIds: ['finished'] },
  dueDatePropertyId: 'due',
};

/** A reader in Seoul at noon on Friday 27 November 2026, whose week starts on Monday. */
const READER: QuickFilterContext = {
  meId: 'me',
  timeZone: 'Asia/Seoul',
  weekStart: 1,
  now: at('Asia/Seoul', 2026, 11, 27, 12),
};

function cardWith(id: string, properties: Record<string, PropertyValue> = {}): Card {
  const card = createCard();
  card.id = id;
  for (const [id, value] of Object.entries(properties)) setCardProperty(card, id, value);
  return card;
}

describe('the chips a board offers', () => {
  it('are all four, in their order, on a board with people, a rule of done and a due date', () => {
    expect(offeredQuickFilters(BOARD, 'me')).toEqual([...QUICK_FILTERS]);
    expect(QUICK_FILTERS).toEqual(['mine', 'incomplete', 'overdue', 'dueThisWeek']);
  });

  it('leave out "Mine" for a reader who is not signed in (a public link), or a board without people', () => {
    expect(offeredQuickFilters(BOARD, undefined)).toEqual(['incomplete', 'overdue', 'dueThisWeek']);
    const noPeople = {
      ...BOARD,
      cardProperties: BOARD.cardProperties.filter((p) => !p.type.toLowerCase().endsWith('person')),
    };
    expect(offeredQuickFilters(noPeople, 'me')).not.toContain('mine');
  });

  it('leave out "Not done" without a rule of done, and the due ones without a date property', () => {
    expect(offeredQuickFilters({ ...BOARD, doneWhen: null }, 'me')).toEqual([
      'mine',
      'overdue',
      'dueThisWeek',
    ]);
    const undated = {
      ...BOARD,
      cardProperties: BOARD.cardProperties.filter((property) => property.type !== 'date'),
    };
    expect(offeredQuickFilters(undated, 'me')).toEqual(['mine', 'incomplete']);
    // A rule on a property the board lost since; a due date it names that holds no dates.
    const lost = {
      ...undated,
      doneWhen: { kind: 'checkbox', propertyId: 'gone' },
      dueDatePropertyId: 'owner',
    } as const;
    expect(offeredQuickFilters(lost, 'me')).toEqual(['mine']);
    expect(offeredQuickFilters(undefined, 'me')).toEqual([]);
  });

  it('take the first date property for the due date when the board names none ("automatic")', () => {
    const later: PropertyTemplate = { id: 'later', name: 'Later', type: 'date', options: [] };
    const automatic = {
      ...BOARD,
      cardProperties: [...BOARD.cardProperties, later],
      dueDatePropertyId: null,
    };
    expect(offeredQuickFilters(automatic, 'me')).toEqual([...QUICK_FILTERS]);
    // The first date property of the board is "Due", not "Later".
    const card = cardWith('a', { due: { from: '2026-11-26' }, later: { from: '2026-12-26' } });
    expect(isQuickFilterMet('overdue', automatic, card, READER)).toBe(true);
    // A due date it names that is no date property is as none named.
    expect(
      isQuickFilterMet('overdue', { ...automatic, dueDatePropertyId: 'owner' }, card, READER),
    ).toBe(true);
    expect(
      isQuickFilterMet('overdue', { ...automatic, dueDatePropertyId: 'later' }, card, READER),
    ).toBe(false);
  });
});

describe('each chip', () => {
  const met = (id: (typeof QUICK_FILTERS)[number], card: Card, context = READER): boolean =>
    isQuickFilterMet(id, BOARD, card, context);

  it('"Mine" lets through a card one of whose person properties names the reader', () => {
    expect(met('mine', cardWith('a', { owner: 'me' }))).toBe(true);
    expect(met('mine', cardWith('b', { helpers: ['you', 'me'] }))).toBe(true);
    expect(met('mine', cardWith('c', { owner: 'you', helpers: ['them'] }))).toBe(false);
    // Who made a card is not whose it is.
    expect(met('mine', cardWith('d', { author: 'me' }))).toBe(false);
    expect(met('mine', cardWith('e'))).toBe(false);
  });

  it('"Not done" lets through a card the board’s rule does not call done', () => {
    expect(met('incomplete', cardWith('a', { status: 'todo' }))).toBe(true);
    expect(met('incomplete', cardWith('b'))).toBe(true);
    expect(met('incomplete', cardWith('c', { status: 'finished' }))).toBe(false);
  });

  it('"Overdue" lets through a card due before the reader’s today that is not done', () => {
    expect(met('overdue', cardWith('a', { due: { from: '2026-11-26' } }))).toBe(true);
    expect(met('overdue', cardWith('b', { due: { from: '2026-11-27' } }))).toBe(false);
    expect(met('overdue', cardWith('c', { due: { from: '2026-11-20' }, status: 'finished' }))).toBe(
      false,
    );
    // A range is due on its last day.
    expect(met('overdue', cardWith('d', { due: { from: '2026-11-20', to: '2026-11-27' } }))).toBe(
      false,
    );
    expect(met('overdue', cardWith('e', { due: { from: '2026-11-20', to: '2026-11-26' } }))).toBe(
      true,
    );
    expect(met('overdue', cardWith('f'))).toBe(false);
  });

  it('"Overdue" reads today in the reader’s time zone', () => {
    // 08:00 on the 27th in Seoul is still the 26th in New York.
    const morning = { ...READER, now: at('Asia/Seoul', 2026, 11, 27, 8) };
    const card = cardWith('a', { due: { from: '2026-11-26' } });
    expect(met('overdue', card, morning)).toBe(true);
    expect(met('overdue', card, { ...morning, timeZone: 'America/New_York' })).toBe(false);
  });

  it('"Due this week" lets through a card due in the reader’s week, done or not', () => {
    expect(met('dueThisWeek', cardWith('a', { due: { from: '2026-11-23' } }))).toBe(true);
    expect(met('dueThisWeek', cardWith('b', { due: { from: '2026-11-29' } }))).toBe(true);
    expect(
      met('dueThisWeek', cardWith('c', { due: { from: '2026-11-29' }, status: 'finished' })),
    ).toBe(true);
    expect(met('dueThisWeek', cardWith('d', { due: { from: '2026-11-30' } }))).toBe(false);
    expect(met('dueThisWeek', cardWith('e', { due: { from: '2026-11-22' } }))).toBe(false);
    expect(met('dueThisWeek', cardWith('f'))).toBe(false);
  });

  it('"Due this week" follows the day the reader’s week starts on', () => {
    const sunday = cardWith('a', { due: { from: '2026-11-22' } });
    const monday = cardWith('b', { due: { from: '2026-11-30' } });
    const fromSunday = { ...READER, weekStart: 7 } as const;
    expect(met('dueThisWeek', sunday, fromSunday)).toBe(true);
    expect(met('dueThisWeek', monday, fromSunday)).toBe(false);
    expect(met('dueThisWeek', sunday)).toBe(false);
    // On the Sunday that ends a week from Monday and begins one from Sunday.
    const onSunday = { ...READER, now: at('Asia/Seoul', 2026, 11, 29, 12) };
    expect(met('dueThisWeek', monday, onSunday)).toBe(false);
    expect(met('dueThisWeek', monday, { ...onSunday, weekStart: 7 })).toBe(true);
  });
});

describe('the chips together', () => {
  const cards = [
    cardWith('mine-late', { owner: 'me', due: { from: '2026-11-20' }, status: 'todo' }),
    cardWith('mine-done', { owner: 'me', due: { from: '2026-11-25' }, status: 'finished' }),
    cardWith('yours-late', { owner: 'you', due: { from: '2026-11-24' } }),
    cardWith('mine-later', { helpers: ['me'], due: { from: '2026-12-10' } }),
  ];
  const ids = (shown: readonly Card[]): string[] => shown.map((card) => card.id);

  it('all have to hold, and the cards keep their order', () => {
    expect(ids(applyQuickFilters(cards, ['mine'], BOARD, READER))).toEqual([
      'mine-late',
      'mine-done',
      'mine-later',
    ]);
    expect(ids(applyQuickFilters(cards, ['mine', 'overdue'], BOARD, READER))).toEqual([
      'mine-late',
    ]);
    expect(ids(applyQuickFilters(cards, ['incomplete', 'dueThisWeek'], BOARD, READER))).toEqual([
      'yours-late',
    ]);
    expect(ids(applyQuickFilters(cards, [], BOARD, READER))).toEqual(ids(cards));
  });

  it('leave out a chip the board does not offer: "Mine" on a public link lets every card through', () => {
    const publicLink = { ...READER, meId: undefined };
    expect(ids(applyQuickFilters(cards, ['mine'], BOARD, publicLink))).toEqual(ids(cards));
    const undated = {
      ...BOARD,
      cardProperties: BOARD.cardProperties.filter((property) => property.type !== 'date'),
    };
    expect(ids(applyQuickFilters(cards, ['overdue'], undated, READER))).toEqual(ids(cards));
  });

  it('let a card kept through whatever they are, without giving it values', () => {
    const fresh = cardWith('fresh');
    const before = structuredClone(fresh);
    const shown = applyQuickFilters(
      [...cards, fresh],
      ['mine', 'overdue'],
      BOARD,
      READER,
      (card) => card.id === 'fresh',
    );
    expect(ids(shown)).toEqual(['mine-late', 'fresh']);
    expect(fresh).toEqual(before);
  });
});
