/**
 * The changes the filter editor makes to a filter: which conditions each
 * kind of value offers, what a new condition starts as, what changing its
 * property resets, the groups it makes and takes apart by their paths, and
 * that a group deeper than it goes is never touched.
 */
import { describe, expect, it } from 'vitest';

import {
  addClause,
  addGroup,
  conditionCount,
  conditionsFor,
  filterEntries,
  filterPropertyIds,
  filterValueTypeOf,
  nodeAt,
  removeClause,
  removeGroup,
  setClauseCondition,
  setClauseProperty,
  setClauseValues,
  setOperation,
  takesValue,
  TITLE_FILTER_ID,
  toggleClauseValue,
  ungroup,
  type FilterEntry,
} from '$lib/features/view-header/filter-edit.ts';
import { DONE_FILTER_ID } from '$lib/logic/filter-done.ts';
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

/** The conditions directly in a group, as the editor's lines of them. */
const clausesOf = (group: FilterGroup): FilterEntry[] =>
  filterEntries(group).filter((entry) => entry.kind === 'clause');

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
    expect(conditionsFor('date')).toEqual([
      'is',
      'isBefore',
      'isAfter',
      'isWithin',
      'isSet',
      'isNotSet',
    ]);
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
    expect(takesValue('isWithin')).toBe(true);
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
    expect(
      clausesOf(group).map((entry) => entry.kind === 'clause' && entry.clause.propertyId),
    ).toEqual(['status', 'note', 'due', 'owner', 'checked']);
    expect((group.filters.at(-1) as FilterClause).condition).toBe('isSet');
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
    const next = setClauseProperty(group, [0], 'owner', TEMPLATES);
    expect(next.filters[0]).toEqual(clause('owner', 'isNotEmpty'));
    expect(next.filters[1]).toBe(group.filters[1]);
  });

  it('takes the default of a kind that does not have the condition', () => {
    expect(setClauseProperty(group, [0], 'note', TEMPLATES).filters[0]).toEqual(
      clause('note', 'is'),
    );
    expect(setClauseProperty(group, [0], 'checked', TEMPLATES).filters[0]).toEqual(
      clause('checked', 'isSet'),
    );
    expect(setClauseProperty(group, [1], 'status', TEMPLATES).filters[1]).toEqual(
      clause('status', 'includes'),
    );
    expect(setClauseProperty(group, [1], TITLE_FILTER_ID, TEMPLATES).filters[1]).toEqual(
      clause(TITLE_FILTER_ID, 'contains'),
    );
  });

  it('keeps the values when only the condition changes', () => {
    expect(setClauseCondition(group, [0], 'includes').filters[0]).toEqual(
      clause('status', 'includes', ['todo']),
    );
  });

  it('ticks and unticks a value', () => {
    const ticked = toggleClauseValue(group, [0], 'done');
    expect((ticked.filters[0] as FilterClause).values).toEqual(['todo', 'done']);
    const unticked = toggleClauseValue(ticked, [0], 'todo');
    expect((unticked.filters[0] as FilterClause).values).toEqual(['done']);
  });

  it('returns the filter it was given when there is nothing to change', () => {
    expect(setClauseProperty(group, [0], 'status', TEMPLATES)).toBe(group);
    expect(setClauseCondition(group, [0], 'isNotEmpty')).toBe(group);
    expect(setClauseValues(group, [0], ['todo'])).toBe(group);
    expect(setOperation(group, [], 'and')).toBe(group);
    expect(removeClause(group, [7])).toBe(group);
    expect(setClauseValues(group, [7], ['x'])).toBe(group);
  });

  it('does not change the filter it was given', () => {
    const before = structuredClone(group);
    setClauseProperty(group, [0], 'note', TEMPLATES);
    setClauseValues(group, [1], ['y']);
    toggleClauseValue(group, [0], 'done');
    removeClause(group, [0]);
    addClause(group, TEMPLATES);
    setOperation(group, [], 'or');
    expect(group).toEqual(before);
  });
});

describe('whether a card is done', () => {
  const rule = { kind: 'checkbox', propertyId: 'checked' } as const;

  it('is offered after the title where the board has a rule of done on a property it has', () => {
    expect(filterPropertyIds(TEMPLATES, rule).slice(0, 3)).toEqual([
      TITLE_FILTER_ID,
      DONE_FILTER_ID,
      'status',
    ]);
    expect(filterPropertyIds(TEMPLATES, undefined)).not.toContain(DONE_FILTER_ID);
    expect(filterPropertyIds(TEMPLATES, { kind: 'checkbox', propertyId: 'gone' })).not.toContain(
      DONE_FILTER_ID,
    );
  });

  it('is a yes or a no: a condition put on it is "is set", and its values go', () => {
    expect(filterValueTypeOf(DONE_FILTER_ID, TEMPLATES)).toBe('boolean');
    const group: FilterGroup = {
      operation: 'and',
      filters: [clause('status', 'includes', ['todo'])],
    };
    expect(setClauseProperty(group, [0], DONE_FILTER_ID, TEMPLATES).filters[0]).toEqual(
      clause(DONE_FILTER_ID, 'isSet'),
    );
  });
});

describe('a group in the filter', () => {
  const inner = NESTED.filters[1] as FilterGroup;

  it('is a part of the editor of its own, its conditions with their paths', () => {
    expect(filterEntries(NESTED)).toEqual([
      { kind: 'clause', path: [0], clause: NESTED.filters[0] },
      {
        kind: 'group',
        path: [1],
        group: inner,
        entries: [
          { kind: 'clause', path: [1, 0], clause: inner.filters[0] },
          { kind: 'clause', path: [1, 1], clause: inner.filters[1] },
        ],
      },
    ]);
    expect(nodeAt(NESTED, [1, 1])).toBe(inner.filters[1]);
    expect(nodeAt(NESTED, [])).toBe(NESTED);
    expect(nodeAt(NESTED, [0, 0])).toBeUndefined();
    expect(nodeAt(NESTED, [5])).toBeUndefined();
    expect(conditionCount(NESTED)).toBe(3);
    expect(conditionCount(EMPTY)).toBe(0);
  });

  it('has its conditions changed by their paths, the rest of the filter the very objects they were', () => {
    const edited = setClauseValues(NESTED, [1, 0], ['done']);
    expect(edited.filters[0]).toBe(NESTED.filters[0]);
    const group = edited.filters[1] as FilterGroup;
    expect(group.filters[0]).toEqual(clause('status', 'includes', ['done']));
    expect(group.filters[1]).toBe(inner.filters[1]);
    expect(setClauseCondition(NESTED, [1, 1], 'isNotSet').filters[0]).toBe(NESTED.filters[0]);
    expect(toggleClauseValue(NESTED, [1, 0], 'todo')).toEqual({
      ...NESTED,
      filters: [
        NESTED.filters[0],
        { ...inner, filters: [clause('status', 'includes'), inner.filters[1]] },
      ],
    });
  });

  it('gets a condition at its end, on a property it has none on yet', () => {
    const added = addClause(NESTED, TEMPLATES, [1]);
    expect((added.filters[1] as FilterGroup).filters[2]).toEqual(clause('note', 'is'));
    expect(added.filters[0]).toBe(NESTED.filters[0]);
  });

  it('is added with one condition, asking the other of what the filter asks', () => {
    const one = addGroup(EMPTY, TEMPLATES);
    expect(one.filters).toEqual([{ operation: 'or', filters: [clause('status', 'includes')] }]);
    const any: FilterGroup = { operation: 'or', filters: [clause('note', 'is', ['x'])] };
    expect(addGroup(any, TEMPLATES).filters[1]).toEqual({
      operation: 'and',
      filters: [clause('status', 'includes')],
    });
  });

  it('asks all of its conditions or any one, apart from the filter', () => {
    const changed = setOperation(NESTED, [1], 'or');
    expect(changed.operation).toBe('or');
    expect((changed.filters[1] as FilterGroup).operation).toBe('or');
    expect(setOperation(NESTED, [1], 'and')).toBe(NESTED);
    expect(setOperation(NESTED, [0], 'and')).toBe(NESTED);
  });

  it('is taken apart: its conditions take its place, in their order', () => {
    const group: FilterGroup = {
      operation: 'and',
      filters: [clause('note', 'is', ['a']), inner, clause('due', 'isNotSet')],
    };
    expect(ungroup(group, [1]).filters).toEqual([
      group.filters[0],
      inner.filters[0],
      inner.filters[1],
      group.filters[2],
    ]);
    expect(ungroup(group, [0])).toBe(group);
    expect(ungroup(group, [])).toBe(group);
  });

  it('is deleted with its conditions, and goes when its last condition does', () => {
    expect(removeGroup(NESTED, [1]).filters).toEqual([NESTED.filters[0]]);
    expect(removeGroup(NESTED, [0])).toBe(NESTED);
    const single: FilterGroup = {
      operation: 'or',
      filters: [
        clause('note', 'is', ['a']),
        { operation: 'and', filters: [clause('due', 'isSet')] },
      ],
    };
    expect(removeClause(single, [1, 0]).filters).toEqual([single.filters[0]]);
    // The filter itself stays when its last condition goes.
    expect(
      removeClause({ operation: 'and', filters: [clause('due', 'isSet')] }, [0]).filters,
    ).toEqual([]);
  });

  it('is not changed or removed by an edit of a condition aimed at its path', () => {
    expect(removeClause(NESTED, [1])).toBe(NESTED);
    expect(setClauseValues(NESTED, [1], ['x'])).toBe(NESTED);
    expect(setClauseCondition(NESTED, [1], 'is')).toBe(NESTED);
    expect(setClauseProperty(NESTED, [1], 'note', TEMPLATES)).toBe(NESTED);
    expect(toggleClauseValue(NESTED, [1], 'x')).toBe(NESTED);
    expect(setClauseValues(NESTED, [1, 7], ['x'])).toBe(NESTED);
  });

  it('does not change the filter it was given', () => {
    const before = structuredClone(NESTED);
    addGroup(NESTED, TEMPLATES);
    addClause(NESTED, TEMPLATES, [1]);
    setOperation(NESTED, [1], 'or');
    ungroup(NESTED, [1]);
    removeGroup(NESTED, [1]);
    removeClause(NESTED, [1, 0]);
    expect(NESTED).toEqual(before);
  });
});

describe('a group deeper than the editor goes', () => {
  /** Three levels: the filter, a group in it, and a group in that, as another client can write it. */
  const deepest: FilterGroup = {
    operation: 'or',
    filters: [clause('due', 'isSet'), clause('note', 'is', ['z'])],
  };
  const middle: FilterGroup = {
    operation: 'and',
    filters: [clause('status', 'includes', ['todo']), deepest],
  };
  const DEEP: FilterGroup = {
    operation: 'or',
    filters: [clause('note', 'contains', ['a']), middle],
  };

  it('is said to be there among the parts of its group, not as lines', () => {
    const [, group] = filterEntries(DEEP);
    expect(group?.kind === 'group' && group.entries).toEqual([
      { kind: 'clause', path: [1, 0], clause: middle.filters[0] },
      { kind: 'kept', path: [1, 1], group: deepest },
    ]);
    expect(conditionCount(DEEP)).toBe(4);
  });

  it('is the same object after every kind of edit around it', () => {
    const edits: FilterGroup[] = [
      addClause(DEEP, TEMPLATES),
      addClause(DEEP, TEMPLATES, [1]),
      addGroup(DEEP, TEMPLATES),
      setClauseProperty(DEEP, [1, 0], 'note', TEMPLATES),
      setClauseCondition(DEEP, [1, 0], 'isEmpty'),
      setClauseValues(DEEP, [1, 0], ['done']),
      toggleClauseValue(DEEP, [0], 'b'),
      removeClause(DEEP, [1, 0]),
      removeClause(DEEP, [0]),
      setOperation(DEEP, [1], 'or'),
      setOperation(DEEP, [], 'and'),
    ];
    for (const edited of edits) {
      expect(edited).not.toBe(DEEP);
      const found = edited.filters.flatMap((part) =>
        'filters' in part ? part.filters.filter((inside) => inside === deepest) : [],
      );
      expect(found).toHaveLength(1);
    }
    expect(deepest).toEqual({
      operation: 'or',
      filters: [clause('due', 'isSet'), clause('note', 'is', ['z'])],
    });
  });

  it('is not changed by an edit of a condition aimed at its path, and comes up a level when its group is taken apart', () => {
    expect(setClauseValues(DEEP, [1, 1], ['x'])).toBe(DEEP);
    expect(removeClause(DEEP, [1, 1])).toBe(DEEP);
    const apart = ungroup(DEEP, [1]);
    expect(apart.filters[2]).toBe(deepest);
    expect(filterEntries(apart)[2]?.kind).toBe('group');
  });
});
