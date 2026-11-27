/**
 * Changes to the definition of a property, worked out without a store: what
 * a change of kind does to the values of the cards, and what a view loses
 * with a property.
 */
import { describe, expect, it } from 'vitest';

import {
  freePropertyName,
  newOption,
  newProperty,
  planTypeChange,
  viewWithoutProperty,
} from '$lib/features/card/property-change.ts';
import {
  createBoardView,
  createCard,
  type BoardView,
  type Card,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue,
} from '$lib/logic/wire.ts';

const ID = 'property';

function template(type: PropertyType, options: string[] = []): PropertyTemplate {
  return {
    id: ID,
    name: 'Stage',
    type,
    options: options.map((label) => ({ id: `id-${label}`, label, color: 'blue' })),
  };
}

function cardWith(id: string, value?: PropertyValue): Card {
  const card = createCard();
  card.id = id;
  if (value !== undefined) card.fields.properties[ID] = value;
  return card;
}

describe('a name for a new property', () => {
  it('is the name of its kind when that is free', () => {
    expect(freePropertyName('Text', [template('select')])).toBe('Text');
  });

  it('gets the lowest free number when the name is taken', () => {
    const taken = [
      { ...template('text'), name: 'Text' },
      { ...template('text'), name: 'Text 2' },
      { ...template('text'), name: 'Text 4' },
    ];
    expect(freePropertyName('Text', taken)).toBe('Text 3');
  });

  it('comes with an identifier of its own and no options', () => {
    const first = newProperty('select', 'Select', []);
    const second = newProperty('select', 'Select', [first]);
    expect(first).toMatchObject({ name: 'Select', type: 'select', options: [] });
    expect(second.name).toBe('Select 2');
    expect(second.id).not.toBe(first.id);
  });

  it('gives a new option the default colour', () => {
    expect(newOption('Later')).toMatchObject({ label: 'Later', color: 'default' });
  });
});

describe('changing the kind of a property', () => {
  it('leaves texts alone between the kinds that hold a text', () => {
    const cards = [cardWith('a', '12'), cardWith('b', 'twelve'), cardWith('c')];
    const plan = planTypeChange(template('text'), 'number', cards);
    expect(plan.template.type).toBe('number');
    expect(plan.values.size).toBe(0);
    expect(plan).toMatchObject({ kept: 2, lost: 0 });
  });

  it('turns texts into options of a select, one option for the same text', () => {
    const cards = [cardWith('a', 'Now'), cardWith('b', 'Later'), cardWith('c', ' Now ')];
    const plan = planTypeChange(template('text'), 'select', cards);
    expect(plan.template.options.map((option) => option.label)).toEqual(['Now', 'Later']);
    const [now, later] = plan.template.options;
    expect(plan.values.get('a')).toBe(now?.id);
    expect(plan.values.get('b')).toBe(later?.id);
    expect(plan.values.get('c')).toBe(now?.id);
    expect(plan).toMatchObject({ kept: 3, lost: 0 });
  });

  it('turns a text into a list of one option for a multi-select', () => {
    const plan = planTypeChange(template('text'), 'multiSelect', [cardWith('a', 'Now')]);
    expect(plan.values.get('a')).toEqual([plan.template.options[0]?.id]);
  });

  it('keeps the options between select and multi-select; a select keeps the first', () => {
    const many = template('multiSelect', ['one', 'two']);
    const cards = [cardWith('a', ['id-two', 'id-one']), cardWith('b', ['id-one'])];
    const toOne = planTypeChange(many, 'select', cards);
    expect(toOne.template.options).toEqual(many.options);
    expect(toOne.values.get('a')).toBe('id-two');
    expect(toOne.values.get('b')).toBe('id-one');
    expect(toOne).toMatchObject({ kept: 2, lost: 0 });

    const toMany = planTypeChange(template('select', ['one']), 'multiSelect', [
      cardWith('a', 'id-one'),
    ]);
    expect(toMany.values.get('a')).toEqual(['id-one']);
  });

  it('removes a value that names an option that is gone', () => {
    const plan = planTypeChange(template('select', ['one']), 'multiSelect', [
      cardWith('a', 'id-deleted'),
    ]);
    expect(plan.values.has('a')).toBe(true);
    expect(plan.values.get('a')).toBeUndefined();
    expect(plan).toMatchObject({ kept: 0, lost: 1 });
  });

  it('turns options into their names for a text kind', () => {
    const one = cardWith('a', 'id-one');
    const cards = [one, cardWith('b', ['id-one', 'id-two'])];
    const single = planTypeChange(template('select', ['one', 'two']), 'text', [one]);
    expect(single.values.get('a')).toBe('one');
    expect(single.template.options).toEqual([]);
    const many = planTypeChange(template('multiSelect', ['one', 'two']), 'text', cards);
    expect(many.values.get('b')).toBe('one, two');
    expect(many).toMatchObject({ kept: 2, lost: 0 });
  });

  it('keeps people between person and multi-person; a person keeps the first', () => {
    const toOne = planTypeChange(template('multiPerson'), 'person', [cardWith('a', ['u1', 'u2'])]);
    expect(toOne.values.get('a')).toBe('u1');
    const toMany = planTypeChange(template('person'), 'multiPerson', [cardWith('a', 'u1')]);
    expect(toMany.values.get('a')).toEqual(['u1']);
  });

  it('keeps a checked box as the text it is stored as, and only that text as a checked box', () => {
    const toText = planTypeChange(template('checkbox'), 'text', [cardWith('a', 'true')]);
    expect(toText.values.size).toBe(0);
    expect(toText).toMatchObject({ kept: 1, lost: 0 });
    const toBox = planTypeChange(template('text'), 'checkbox', [
      cardWith('a', 'true'),
      cardWith('b', 'yes'),
    ]);
    expect(toBox.values.has('a')).toBe(false);
    expect(toBox.values.get('b')).toBeUndefined();
    expect(toBox).toMatchObject({ kept: 1, lost: 1 });
  });

  it.each<[PropertyType, PropertyValue, PropertyType]>([
    ['text', 'someone', 'person'],
    ['person', 'u1', 'text'],
    ['person', 'u1', 'select'],
    ['select', 'id-one', 'person'],
    ['select', 'id-one', 'date'],
    ['text', '2026-11-10', 'date'],
    ['date', { from: '2026-11-10' }, 'text'],
    ['date', { from: '2026-11-10' }, 'checkbox'],
    ['checkbox', 'true', 'select'],
    ['text', 'kept nowhere', 'createdTime'],
  ])('removes the value going from %s (%j) to %s', (from, value, to) => {
    const plan = planTypeChange(template(from, ['one']), to, [cardWith('a', value)]);
    expect(plan.values.has('a')).toBe(true);
    expect(plan.values.get('a')).toBeUndefined();
    expect(plan).toMatchObject({ kept: 0, lost: 1 });
    expect(plan.template.options).toEqual([]);
  });

  it('counts only the cards that have a value', () => {
    const cards = [cardWith('a'), cardWith('b', ''), cardWith('c', []), cardWith('d', 'x')];
    expect(planTypeChange(template('text'), 'date', cards)).toMatchObject({ kept: 0, lost: 1 });
  });

  it('has nothing to carry over from a kind that stores nothing', () => {
    const plan = planTypeChange(template('createdTime'), 'text', [cardWith('a', 'left behind')]);
    expect(plan.values.size).toBe(0);
    expect(plan).toMatchObject({ kept: 0, lost: 0 });
  });

  it('does not touch the definition it was given', () => {
    const before = template('select', ['one']);
    const copy = structuredClone(before);
    planTypeChange(before, 'text', [cardWith('a', 'id-one')]);
    expect(before).toEqual(copy);
  });
});

describe('a view without a property', () => {
  function view(fields: Partial<BoardView['fields']>): BoardView {
    const made = createBoardView();
    made.fields = { ...made.fields, ...fields };
    return made;
  }

  const using = (): BoardView =>
    view({
      viewType: 'board',
      groupBy: ID,
      dateProperty: ID,
      shownProperties: ['other', ID],
      sort: [
        { propertyId: ID, reversed: false },
        { propertyId: 'other', reversed: true },
      ],
      visibleGroups: ['id-one', ''],
      hiddenGroups: ['id-two'],
      collapsedGroups: ['id-one'],
      filter: {
        operation: 'and',
        filters: [
          { propertyId: ID, condition: 'includes', values: ['id-one'] },
          { propertyId: 'other', condition: 'isNotEmpty', values: [] },
          { operation: 'or', filters: [{ propertyId: ID, condition: 'isEmpty', values: [] }] },
        ],
      },
      columnWidths: { [ID]: 120, other: 80 },
      columnCalculations: { [ID]: 'count', other: 'sum' },
      groupCalculations: {
        'id-one': { calculation: 'sum', propertyId: ID },
        'id-two': { calculation: 'count', propertyId: 'other' },
      },
    });

  it('loses every mention of a deleted property', () => {
    const { fields } = viewWithoutProperty(using(), ID);
    expect(fields.shownProperties).toEqual(['other']);
    expect(fields.sort).toEqual([{ propertyId: 'other', reversed: true }]);
    // The nested group was only about the property: it goes with its clause.
    expect(fields.filter).toEqual({
      operation: 'and',
      filters: [{ propertyId: 'other', condition: 'isNotEmpty', values: [] }],
    });
    expect('groupBy' in fields).toBe(false);
    expect('dateProperty' in fields).toBe(false);
    expect(fields.visibleGroups).toEqual([]);
    expect(fields.hiddenGroups).toEqual([]);
    expect(fields.collapsedGroups).toEqual([]);
    expect(fields.columnWidths).toEqual({ other: 80 });
    expect(fields.columnCalculations).toEqual({ other: 'sum' });
    expect(fields.groupCalculations).toEqual({
      'id-two': { calculation: 'count', propertyId: 'other' },
    });
  });

  it('is the view it was given when the property is not used in it', () => {
    const plain = view({ shownProperties: ['other'] });
    expect(viewWithoutProperty(plain, ID)).toBe(plain);
    expect(viewWithoutProperty(plain, ID, 'text')).toBe(plain);
  });

  it('does not touch the view it was given', () => {
    const before = using();
    const copy = structuredClone(before);
    viewWithoutProperty(before, ID);
    viewWithoutProperty(before, ID, 'text');
    expect(before).toEqual(copy);
  });

  it('keeps where a property is shown and sorted by when only its kind changes', () => {
    const { fields } = viewWithoutProperty(using(), ID, 'text');
    expect(fields.shownProperties).toEqual(['other', ID]);
    expect(fields.sort).toHaveLength(2);
    expect(fields.columnWidths).toEqual({ [ID]: 120, other: 80 });
  });

  it('drops what was written for the old kind: filter clauses, calculations, columns', () => {
    const { fields } = viewWithoutProperty(using(), ID, 'text');
    expect(fields.filter.filters).toEqual([
      { propertyId: 'other', condition: 'isNotEmpty', values: [] },
    ]);
    expect(fields.columnCalculations).toEqual({ other: 'sum' });
    expect(Object.keys(fields.groupCalculations)).toEqual(['id-two']);
    expect(fields.visibleGroups).toEqual([]);
    // A text cannot group cards or place them on a calendar.
    expect('groupBy' in fields).toBe(false);
    expect('dateProperty' in fields).toBe(false);
  });

  it('keeps the grouping for a kind that can group, with its columns emptied', () => {
    const { fields } = viewWithoutProperty(using(), ID, 'person');
    expect(fields.groupBy).toBe(ID);
    expect(fields.visibleGroups).toEqual([]);
    expect('dateProperty' in fields).toBe(false);
  });

  it('keeps the calendar date for a kind that has days', () => {
    const { fields } = viewWithoutProperty(using(), ID, 'date');
    expect(fields.dateProperty).toBe(ID);
    expect('groupBy' in fields).toBe(false);
  });
});

describe('a Gantt view, when the relation it draws arrows by goes', () => {
  const gantt = (): ReturnType<typeof createBoardView> => {
    const made = createBoardView();
    made.fields.viewType = 'gantt';
    made.fields.dependencyPropertyId = 'after';
    return made;
  };

  it('forgets it when it is deleted', () => {
    const { fields } = viewWithoutProperty(gantt(), 'after');
    expect('dependencyPropertyId' in fields).toBe(false);
  });

  it('forgets it when it becomes another kind', () => {
    const { fields } = viewWithoutProperty(gantt(), 'after', 'text');
    expect('dependencyPropertyId' in fields).toBe(false);
  });

  it('keeps it while it stays a card relation, and keeps another', () => {
    expect(viewWithoutProperty(gantt(), 'after', 'relation').fields.dependencyPropertyId).toBe(
      'after',
    );
    expect(viewWithoutProperty(gantt(), 'other').fields.dependencyPropertyId).toBe('after');
  });
});

describe('a view, when a property becomes a card relation', () => {
  it('takes a view off grouping and calculating by it', () => {
    const view = createBoardView();
    view.fields.groupBy = 'after';
    view.fields.columnCalculations = { after: 'count' };
    const next = viewWithoutProperty(view, 'after', 'relation');
    expect(next.fields.groupBy).toBeUndefined();
    expect(next.fields.columnCalculations).toEqual({});
  });
});
