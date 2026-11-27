/**
 * The changes the filter editor makes to a filter: which conditions each
 * kind of value offers, what a new condition starts as, what changing its
 * property resets, and that a group inside the filter is never touched.
 */
import { describe, expect, it } from 'vitest';

import {
  addClause,
  clauseEntries,
  conditionCount,
  conditionsFor,
  filterValueTypeOf,
  nestedGroupCount,
  removeClause,
  setClauseCondition,
  setClauseProperty,
  setClauseValues,
  setOperation,
  takesValue,
  TITLE_FILTER_ID,
  toggleClauseValue,
} from '$lib/features/view-header/filter-edit.ts';
import { isClauseMet } from '$lib/logic/filter.ts';
import { filterConditionValidOrDefault } from '$lib/logic/filter-conditions.ts';
import type { FilterValueType } from '$lib/logic/properties.ts';
import {
  createCard,
  type FilterClause,
  type FilterGroup,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

const TEMPLATES: PropertyTemplate[] = [
  { id: 'hours', name: 'Hours', type: 'number', options: [] },
  {
    id: 'status',
    name: 'Status',
    type: 'select',
    options: [
      { id: 'todo', label: 'To do', color: 'blue' },
      { id: 'done', label: 'Done', color: 'green' },
    ],
  },
  { id: 'note', name: 'Note', type: 'text', options: [] },
  { id: 'due', name: 'Due', type: 'date', options: [] },
  { id: 'owner', name: 'Owner', type: 'person', options: [] },
  { id: 'checked', name: 'Checked', type: 'checkbox', options: [] },
];

const clause = (propertyId: string, condition: FilterClause['condition'], values: string[] = []) =>
  ({ propertyId, condition, values }) satisfies FilterClause;

/** A group as an archive or another client can have written it: nested two deep. */
const NESTED: FilterGroup = {
  operation: 'or',
  filters: [
    clause('note', 'contains', ['a']),
    { operation: 'and', filters: [clause('status', 'includes', ['todo']), clause('due', 'isSet')] },
  ],
};

const EMPTY: FilterGroup = { operation: 'and', filters: [] };

describe('the conditions of a kind of value', () => {
  it('are the ones the logic keeps for that kind, its default first', () => {
    expect(conditionsFor('options')).toEqual(['includes', 'notIncludes', 'isEmpty', 'isNotEmpty']);
    expect(conditionsFor('person')).toEqual(conditionsFor('options'));
    expect(conditionsFor('boolean')).toEqual(['isSet', 'isNotSet']);
    expect(conditionsFor('text')).toEqual([
      'is',
      'contains',
      'notContains',
      'startsWith',
      'notStartsWith',
      'endsWith',
      'notEndsWith',
    ]);
    expect(conditionsFor('date')).toEqual(['is', 'isBefore', 'isAfter', 'isSet', 'isNotSet']);
    expect(conditionsFor('none')).toEqual([]);
  });

  it('start with what a condition of another kind turns into', () => {
    const kinds: FilterValueType[] = ['options', 'person', 'boolean', 'text', 'date'];
    for (const kind of kinds) {
      const offered = conditionsFor(kind);
      const foreign = kind === 'text' || kind === 'date' ? 'includes' : 'contains';
      expect(offered[0]).toBe(filterConditionValidOrDefault(kind, foreign));
      for (const condition of offered) {
        expect(filterConditionValidOrDefault(kind, condition)).toBe(condition);
      }
    }
  });

  it('take a value, except those that ask whether there is one', () => {
    expect(
      ['isEmpty', 'isNotEmpty', 'isSet', 'isNotSet'].map((c) => takesValue(c as never)),
    ).toEqual([false, false, false, false]);
    expect(takesValue('includes')).toBe(true);
    expect(takesValue('isBefore')).toBe(true);
    expect(takesValue('notEndsWith')).toBe(true);
  });

  it('are those of text for the title, and none for a property that cannot be filtered or is gone', () => {
    expect(filterValueTypeOf(TITLE_FILTER_ID, TEMPLATES)).toBe('text');
    expect(filterValueTypeOf('status', TEMPLATES)).toBe('options');
    expect(filterValueTypeOf('owner', TEMPLATES)).toBe('person');
    expect(filterValueTypeOf('checked', TEMPLATES)).toBe('boolean');
    expect(filterValueTypeOf('hours', TEMPLATES)).toBe('none');
    expect(filterValueTypeOf('deleted', TEMPLATES)).toBe('none');
  });
});

describe('a new condition', () => {
  it('is on the first property that can be filtered and has none yet, with its default condition', () => {
    const one = addClause(EMPTY, TEMPLATES);
    expect(one.filters).toEqual([clause('status', 'includes')]);
    const two = addClause(one, TEMPLATES);
    expect(two.filters[1]).toEqual(clause('note', 'is'));
    const three = addClause(two, TEMPLATES);
    expect(three.filters[2]).toEqual(clause('due', 'is'));
  });

  it('is on the title once every property has one, and again after that', () => {
    let group = EMPTY;
    for (let count = 0; count < 5; count += 1) group = addClause(group, TEMPLATES);
    expect(clauseEntries(group).map((entry) => entry.clause.propertyId)).toEqual([
      'status',
      'note',
      'due',
      'owner',
      'checked',
    ]);
    expect(clauseEntries(group).at(-1)?.clause.condition).toBe('isSet');
    group = addClause(group, TEMPLATES);
    expect(group.filters.at(-1)).toEqual(clause(TITLE_FILTER_ID, 'is'));
    group = addClause(group, TEMPLATES);
    expect(group.filters.at(-1)).toEqual(clause(TITLE_FILTER_ID, 'is'));
  });

  it('is on the title on a board without a property to filter by', () => {
    expect(addClause(EMPTY, []).filters).toEqual([clause(TITLE_FILTER_ID, 'is')]);
  });

  it('lets every card through until it has a value', () => {
    const context = { timeZone: 'UTC' };
    const card = createCard();
    for (let count = 1; count <= 6; count += 1) {
      let group = EMPTY;
      for (let added = 0; added < count; added += 1) group = addClause(group, TEMPLATES);
      const last = group.filters.at(-1) as FilterClause;
      // "Is set" of a checkbox has no value to wait for: it filters at once.
      if (last.condition === 'isSet') continue;
      expect(isClauseMet(last, TEMPLATES, card, context)).toBe(true);
    }
  });
});

describe('changing a condition', () => {
  const group: FilterGroup = {
    operation: 'and',
    filters: [clause('status', 'isNotEmpty', ['todo']), clause('note', 'contains', ['x'])],
  };

  it('keeps the condition for a property whose kind has it, and drops the values', () => {
    const next = setClauseProperty(group, 0, 'owner', TEMPLATES);
    expect(next.filters[0]).toEqual(clause('owner', 'isNotEmpty'));
    expect(next.filters[1]).toBe(group.filters[1]);
  });

  it('takes the default of a kind that does not have the condition', () => {
    expect(setClauseProperty(group, 0, 'note', TEMPLATES).filters[0]).toEqual(clause('note', 'is'));
    expect(setClauseProperty(group, 0, 'checked', TEMPLATES).filters[0]).toEqual(
      clause('checked', 'isSet'),
    );
    expect(setClauseProperty(group, 1, 'status', TEMPLATES).filters[1]).toEqual(
      clause('status', 'includes'),
    );
    expect(setClauseProperty(group, 1, TITLE_FILTER_ID, TEMPLATES).filters[1]).toEqual(
      clause(TITLE_FILTER_ID, 'contains'),
    );
  });

  it('keeps the values when only the condition changes', () => {
    expect(setClauseCondition(group, 0, 'includes').filters[0]).toEqual(
      clause('status', 'includes', ['todo']),
    );
  });

  it('ticks and unticks a value', () => {
    const ticked = toggleClauseValue(group, 0, 'done');
    expect((ticked.filters[0] as FilterClause).values).toEqual(['todo', 'done']);
    const unticked = toggleClauseValue(ticked, 0, 'todo');
    expect((unticked.filters[0] as FilterClause).values).toEqual(['done']);
  });

  it('returns the filter it was given when there is nothing to change', () => {
    expect(setClauseProperty(group, 0, 'status', TEMPLATES)).toBe(group);
    expect(setClauseCondition(group, 0, 'isNotEmpty')).toBe(group);
    expect(setClauseValues(group, 0, ['todo'])).toBe(group);
    expect(setOperation(group, 'and')).toBe(group);
    expect(removeClause(group, 7)).toBe(group);
    expect(setClauseValues(group, 7, ['x'])).toBe(group);
  });

  it('does not change the filter it was given', () => {
    const before = structuredClone(group);
    setClauseProperty(group, 0, 'note', TEMPLATES);
    setClauseValues(group, 1, ['y']);
    toggleClauseValue(group, 0, 'done');
    removeClause(group, 0);
    addClause(group, TEMPLATES);
    setOperation(group, 'or');
    expect(group).toEqual(before);
  });
});

describe('a group inside the filter', () => {
  const inner = NESTED.filters[1] as FilterGroup;

  it('is not a line of the editor, and its conditions are counted', () => {
    expect(clauseEntries(NESTED)).toEqual([{ index: 0, clause: NESTED.filters[0] }]);
    expect(nestedGroupCount(NESTED)).toBe(1);
    expect(conditionCount(NESTED)).toBe(3);
    expect(conditionCount(EMPTY)).toBe(0);
  });

  it('is the same object after every kind of edit', () => {
    const edits: FilterGroup[] = [
      addClause(NESTED, TEMPLATES),
      setClauseProperty(NESTED, 0, 'status', TEMPLATES),
      setClauseCondition(NESTED, 0, 'is'),
      setClauseValues(NESTED, 0, ['b']),
      toggleClauseValue(NESTED, 0, 'b'),
      removeClause(NESTED, 0),
      setOperation(NESTED, 'and'),
    ];
    for (const edited of edits) {
      expect(edited).not.toBe(NESTED);
      expect(edited.filters.filter((filter) => filter === inner)).toHaveLength(1);
      expect(nestedGroupCount(edited)).toBe(1);
    }
    // And its content is what it was.
    expect(inner).toEqual({
      operation: 'and',
      filters: [clause('status', 'includes', ['todo']), clause('due', 'isSet')],
    });
  });

  it('keeps its place: a line is found by its place among everything, groups included', () => {
    const group: FilterGroup = { operation: 'and', filters: [inner, clause('note', 'is', ['x'])] };
    expect(clauseEntries(group)).toEqual([{ index: 1, clause: group.filters[1] }]);
    const edited = setClauseValues(group, 1, ['y']);
    expect(edited.filters[0]).toBe(inner);
    expect(edited.filters[1]).toEqual(clause('note', 'is', ['y']));
    const added = addClause(edited, TEMPLATES);
    expect(added.filters.map((filter) => filter === inner)).toEqual([true, false, false]);
  });

  it('is not changed or removed by an edit aimed at its place', () => {
    const group: FilterGroup = { operation: 'and', filters: [inner, clause('note', 'is')] };
    expect(removeClause(group, 0)).toBe(group);
    expect(setClauseValues(group, 0, ['x'])).toBe(group);
    expect(setClauseCondition(group, 0, 'is')).toBe(group);
    expect(setClauseProperty(group, 0, 'note', TEMPLATES)).toBe(group);
    expect(toggleClauseValue(group, 0, 'x')).toBe(group);
  });
});
