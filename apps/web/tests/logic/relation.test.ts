/**
 * The card relation kind outside of its editor: how its value is read and
 * written, and what sorting, filtering, searching, grouping, calculations
 * and a change of kind do with it. A card that is not there (deleted) counts
 * as not named, but its identifier stays in the stored value.
 */
import { describe, expect, it } from 'vitest';

import { planTypeChange } from '$lib/features/card/property-change.ts';
import { isClauseMet } from '$lib/logic/filter.ts';
import { propertyBehavior, SELECTABLE_PROPERTY_TYPES } from '$lib/logic/properties.ts';
import { searchFilterCards } from '$lib/logic/search.ts';
import { sortCards } from '$lib/logic/sort.ts';
import {
  createCard,
  parseRelationLink,
  parseRelationValue,
  RELATION_LAG_MAX,
  RELATION_LAG_MIN,
  relationCardIds,
  relationLink,
  setCardProperty,
  type Card,
  type FilterCondition,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

const RELATION: PropertyTemplate = { id: 'after', name: 'After', type: 'relation', options: [] };

function card(id: string, title: string, after?: PropertyValue): Card {
  const made = createCard();
  made.id = id;
  made.title = title;
  made.createdAt = 1;
  if (after !== undefined) setCardProperty(made, 'after', after);
  return made;
}

const TITLES = new Map([
  ['a', 'Alpha'],
  ['b', 'Bravo'],
  ['c', 'Charlie'],
]);
const cardTitle = (id: string): string | undefined => TITLES.get(id);

describe('the stored value', () => {
  it('reads a list of relation links, normalized, each card once, in order', () => {
    const raw = [
      { card: 'b', kind: 'ss', lagDays: -2 },
      { card: 'a' } as unknown as ReturnType<typeof relationLink>,
      { card: 'b', kind: 'ff', lagDays: 4 },
      { card: '' } as unknown as ReturnType<typeof relationLink>,
    ];
    expect(parseRelationValue(raw)).toEqual([
      relationLink('b', 'ss', -2),
      relationLink('a', 'fs', 0),
    ]);
    expect(relationCardIds(raw)).toEqual(['b', 'a']);
    for (const [kind, lag] of [
      ['fs', RELATION_LAG_MIN],
      ['ss', -1],
      ['ff', 0],
      ['sf', RELATION_LAG_MAX],
    ] as const) {
      expect(parseRelationLink({ card: 'a', kind, lagDays: lag })).toEqual({
        card: 'a',
        kind,
        lagDays: lag,
      });
    }
  });

  it('rejects legacy string arrays, invalid kinds, out-of-range lags, and unknown fields', () => {
    expect(parseRelationValue(['a', 'b'])).toEqual([]);
    expect(parseRelationValue('a')).toEqual([]);
    expect(parseRelationValue({ from: '2026-01-01' })).toEqual([]);
    expect(parseRelationValue(undefined)).toEqual([]);
    for (const bad of [
      'a',
      {},
      { card: '' },
      { card: 'a', kind: 'FS' },
      { card: 'a', kind: 'bad' },
      { card: 'a', kind: null },
      { card: 'a', lagDays: RELATION_LAG_MIN - 1 },
      { card: 'a', lagDays: RELATION_LAG_MAX + 1 },
      { card: 'a', lagDays: 1.5 },
      { card: 'a', lagDays: '0' },
      { card: 'a', lagDays: null },
      { card: 'a', extra: 1 },
    ]) {
      expect(parseRelationLink(bad)).toBeUndefined();
    }
    expect(parseRelationLink({ card: 'a', lagDays: -0 })).toEqual({
      card: 'a',
      kind: 'fs',
      lagDays: 0,
    });
  });
});

describe('what the kind can do', () => {
  const behavior = propertyBehavior('relation');

  it('is offered when a property is added, and is no grouping and no calculation', () => {
    expect(SELECTABLE_PROPERTY_TYPES).toContain('relation');
    expect(behavior.canGroup).toBe(false);
    expect(behavior.calculationOptions).toEqual(['none']);
  });

  it('is filtered by whether it names a card that is there', () => {
    expect(behavior.filterValueType).toBe('presence');
    const met = (condition: FilterCondition, value?: PropertyValue): boolean =>
      isClauseMet(
        { propertyId: 'after', condition, values: [] },
        [RELATION],
        card('x', 'X', value),
        { timeZone: 'UTC', cardTitle },
      );
    expect(met('isNotEmpty', [relationLink('a')])).toBe(true);
    expect(met('isEmpty', [relationLink('a')])).toBe(false);
    expect(met('isEmpty')).toBe(true);
    // Only a deleted card, or the card itself: as good as empty.
    expect(met('isEmpty', [relationLink('gone')])).toBe(true);
    expect(met('isEmpty', [relationLink('x')])).toBe(true);
    expect(met('isNotEmpty', [relationLink('gone')])).toBe(false);
  });

  it('shows the titles of the cards that are there, for export', () => {
    const shown = behavior.displayValue(
      [relationLink('c', 'ss', 1), relationLink('gone'), relationLink('x'), relationLink('a')],
      card('x', 'X'),
      RELATION,
      {
        locale: 'en',
        timeZone: 'UTC',
        cardTitle,
      },
    );
    expect(shown).toEqual(['Charlie', 'Alpha']);
  });

  it('sorts by the titles of the cards named; a card naming none, or only deleted ones, goes last', () => {
    const cards = [
      card('1', 'one', [relationLink('c')]),
      card('2', 'two', [relationLink('gone')]),
      card('3', 'three', [relationLink('a'), relationLink('c')]),
      card('4', 'four'),
      card('5', 'five', [relationLink('b')]),
    ];
    const sorted = (reversed: boolean): string[] =>
      sortCards(cards, {
        templates: [RELATION],
        sort: [{ propertyId: 'after', reversed }],
        cardOrder: [],
        locale: 'en',
        timeZone: 'UTC',
        cardTitle,
      }).map((each) => each.title);
    expect(sorted(false)).toEqual(['three', 'five', 'one', 'four', 'two']);
    expect(sorted(true)).toEqual(['one', 'five', 'three', 'four', 'two']);
  });

  it('is not searched: its value is identifiers', () => {
    const cards = [card('1', 'one', [relationLink('bravo')])];
    expect(searchFilterCards(cards, [RELATION], 'bravo')).toEqual([]);
  });
});

describe('a change of kind', () => {
  it('to a relation removes the values of another kind', () => {
    const text = { ...RELATION, type: 'text' as const };
    const plan = planTypeChange(text, 'relation', [card('1', 'one', 'b'), card('2', 'two')]);
    expect(plan.lost).toBe(1);
    expect(plan.kept).toBe(0);
    expect([...plan.values]).toEqual([['1', undefined]]);
  });

  it('from a relation removes its values, whatever the new kind', () => {
    for (const type of ['text', 'multiSelect', 'multiPerson', 'date'] as const) {
      const plan = planTypeChange(RELATION, type, [
        card('1', 'one', [relationLink('a'), relationLink('b')]),
      ]);
      expect(plan.lost).toBe(1);
      expect([...plan.values]).toEqual([['1', undefined]]);
    }
  });

  it('from a list of people removes them: a person is no card', () => {
    const people = { ...RELATION, type: 'multiPerson' as const };
    expect(planTypeChange(people, 'relation', [card('1', 'one', ['user-1'])]).lost).toBe(1);
  });
});
