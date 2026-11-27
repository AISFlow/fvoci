/**
 * What the header writes into a view, and the changes to its sort: the
 * fields of a new view, of a change of grouping, the list of properties a
 * view can show.
 */
import { describe, expect, it } from 'vitest';

import {
  addSort,
  moveSort,
  removeSort,
  reverseSort,
  sortCandidates,
} from '$lib/features/view-header/sort-edit.ts';
import { asideTools, type AsideOffer } from '$lib/features/view-header/texts.ts';
import {
  dependenciesChoices,
  dependenciesName,
  dependenciesOffered,
  displayByChoices,
  displayByName,
  displayByOffered,
  groupableProperties,
  groupingFields,
  groupingOf,
  groupProperty,
  newViewFields,
  propertyToggles,
  sortOffered,
  withPropertyVisible,
} from '$lib/features/view-header/view-fields.ts';
import { optionalViewFields } from '$lib/logic/view-members.ts';
import { applyViewPatch, viewPatches } from '$lib/state/patches.ts';
import {
  createBoardView,
  TITLE_COLUMN_ID,
  type BoardView,
  type PropertyTemplate,
  type SortOption,
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
  { id: 'tags', name: 'Tags', type: 'multiSelect', options: [] },
  { id: 'owner', name: 'Owner', type: 'person', options: [] },
  { id: 'made', name: 'Made by', type: 'createdBy', options: [] },
];

function viewOf(fields: Partial<BoardView['fields']>): BoardView {
  const view = createBoardView();
  return { ...view, fields: { ...view.fields, ...fields } };
}

describe('a new view', () => {
  it('as a board is grouped by the first select property, every column showing', () => {
    const fields = newViewFields('board', TEMPLATES);
    expect(fields.viewType).toBe('board');
    expect(fields.groupBy).toBe('status');
    expect(fields.visibleGroups).toEqual(['', 'todo', 'done']);
    expect(fields.hiddenGroups).toEqual([]);
    expect(fields.shownProperties).toEqual([]);
  });

  it('as a board on a board without a select property is grouped by nothing yet', () => {
    const fields = newViewFields('board', TEMPLATES.slice(0, 1));
    expect(fields.groupBy).toBeUndefined();
    expect(fields.visibleGroups).toEqual([]);
  });

  it('as a table shows every property, ungrouped, and leaves the width of the title column to the table', () => {
    const fields = newViewFields('table', TEMPLATES);
    expect(fields.viewType).toBe('table');
    expect(fields.shownProperties).toEqual(['hours', 'status', 'tags', 'owner', 'made']);
    expect(fields.groupBy).toBe('');
    expect(fields.columnWidths).toEqual({});
  });

  it('as a gallery shows the titles', () => {
    const fields = newViewFields('gallery', TEMPLATES);
    expect(fields.viewType).toBe('gallery');
    expect(fields.shownProperties).toEqual([TITLE_COLUMN_ID]);
  });

  it('has every member a view has, none shared with another new view', () => {
    const first = newViewFields('table', TEMPLATES);
    const second = newViewFields('table', TEMPLATES);
    expect(Object.keys(first).sort()).toEqual(
      Object.keys({ ...createBoardView().fields, groupBy: '' }).sort(),
    );
    expect(first.filter).toEqual({ operation: 'and', filters: [] });
    expect(first.filter).not.toBe(second.filter);
    expect(first.sort).toEqual([]);
  });
});

describe('a calendar', () => {
  const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
  const made: PropertyTemplate = {
    id: 'created',
    name: 'Created',
    type: 'createdTime',
    options: [],
  };

  it('when new, is placed by the first date property and shows the titles', () => {
    const fields = newViewFields('calendar', [...TEMPLATES, made, due]);
    expect(fields.viewType).toBe('calendar');
    expect(fields.dateProperty).toBe('due');
    expect(fields.shownProperties).toEqual([TITLE_COLUMN_ID]);
    expect(newViewFields('calendar', [...TEMPLATES, made]).dateProperty).toBe('created');
    expect(newViewFields('calendar', TEMPLATES).dateProperty).toBeUndefined();
  });

  it('has "display by" and no grouping or sort', () => {
    expect(displayByOffered('calendar')).toBe(true);
    expect(displayByOffered('board')).toBe(false);
    expect(groupingOf('calendar').offered).toBe(false);
    expect(sortOffered('calendar')).toBe(false);
  });

  it('offers every kind of date, the one in use checked, and writes only a change', () => {
    const named = viewOf({ viewType: 'calendar', dateProperty: 'created' });
    expect(displayByChoices(named, [...TEMPLATES, made, due], 'Created time')).toEqual([
      { id: 'created', label: 'Created', checked: true, write: false },
      { id: 'due', label: 'Due', checked: false, write: true },
    ]);
    expect(displayByName(named, [...TEMPLATES, made, due], 'Created time')).toBe('Created');
    // A view that names nothing is placed by the first date; choosing it names it.
    const unnamed = viewOf({ viewType: 'calendar' });
    expect(displayByChoices(unnamed, [made, due], 'Created time')).toEqual([
      { id: 'created', label: 'Created', checked: false, write: true },
      { id: 'due', label: 'Due', checked: true, write: true },
    ]);
  });

  it('on a board without any date says it is placed by when cards were made', () => {
    const view = viewOf({ viewType: 'calendar' });
    expect(displayByName(view, TEMPLATES, 'Created time')).toBe('Created time');
    expect(displayByChoices(view, TEMPLATES, 'Created time')).toEqual([
      { id: '', label: 'Created time', checked: true, write: false },
    ]);
  });
});

describe('a Gantt chart', () => {
  const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };

  it('when new, is placed by the first date property, not grouped, zoomed to weeks', () => {
    const fields = newViewFields('gantt', [...TEMPLATES, due]);
    expect(fields.viewType).toBe('gantt');
    expect(fields.dateProperty).toBe('due');
    expect(fields.ganttZoom).toBe('week');
    expect(fields.groupBy).toBe('');
    expect(fields.shownProperties).toEqual([TITLE_COLUMN_ID]);
    expect(newViewFields('gantt', TEMPLATES).dateProperty).toBeUndefined();
  });

  it('when new, reads the first card relation of the board as its dependencies', () => {
    const after: PropertyTemplate = { id: 'after', name: 'After', type: 'relation', options: [] };
    const other: PropertyTemplate = { id: 'see', name: 'See also', type: 'relation', options: [] };
    expect(newViewFields('gantt', [due, after, other]).dependencyPropertyId).toBe('after');
    // Named only where there is one: a member with no value is not a missing member.
    expect('dependencyPropertyId' in newViewFields('gantt', [due])).toBe(false);
    expect(newViewFields('calendar', [due, after]).dependencyPropertyId).toBeUndefined();
  });

  it('offers "dependencies": none and every card relation, and writes only a change', () => {
    const after: PropertyTemplate = { id: 'after', name: 'After', type: 'relation', options: [] };
    const other: PropertyTemplate = { id: 'see', name: 'See also', type: 'relation', options: [] };
    const templates = [...TEMPLATES, after, other];
    expect(dependenciesOffered('gantt')).toBe(true);
    expect(dependenciesOffered('calendar')).toBe(false);
    const named = viewOf({ viewType: 'gantt', dependencyPropertyId: 'see' });
    expect(dependenciesName(named, templates, 'None')).toBe('See also');
    expect(dependenciesChoices(named, templates, 'None')).toEqual([
      { id: '', label: 'None', checked: false, write: true },
      { id: 'after', label: 'After', checked: false, write: true },
      { id: 'see', label: 'See also', checked: true, write: false },
    ]);
    const unnamed = viewOf({ viewType: 'gantt' });
    expect(dependenciesName(unnamed, templates, 'None')).toBe('None');
    expect(dependenciesChoices(unnamed, templates, 'None')[0]).toEqual({
      id: '',
      label: 'None',
      checked: true,
      write: false,
    });
  });

  it('has no dependencies by a property that is gone or no card relation, and can clear it', () => {
    for (const dependencyPropertyId of ['gone', 'status']) {
      const view = viewOf({ viewType: 'gantt', dependencyPropertyId });
      expect(dependenciesName(view, TEMPLATES, 'None')).toBe('None');
      expect(dependenciesChoices(view, TEMPLATES, 'None')).toEqual([
        { id: '', label: 'None', checked: true, write: true },
      ]);
    }
  });

  it('takes a first choice of "display by" or of a grouping back the same way', () => {
    const before = createBoardView({ ...createBoardView(), fields: { viewType: 'gantt' } });
    expect('dateProperty' in before.fields).toBe(false);
    expect('groupBy' in before.fields).toBe(false);
    const after = createBoardView({
      ...before,
      fields: { ...before.fields, dateProperty: 'start', groupBy: 'status' },
    });
    const [, backward] = viewPatches(after, before);
    expect([...(backward.deletedFields ?? [])].sort()).toEqual(['dateProperty', 'groupBy']);
    // A member that is there, the empty one too, is kept as it is.
    expect(createBoardView(viewOf({ groupBy: '' })).fields.groupBy).toBe('');
    expect(optionalViewFields({ groupBy: '', ganttZoom: undefined })).toStrictEqual({
      groupBy: '',
    });
    expect(optionalViewFields({})).toStrictEqual({});
  });

  it('takes its first choice of dependencies back by dropping the member, not by emptying it', () => {
    // A view that names no relation does not have the member at all.
    const before = createBoardView(viewOf({ viewType: 'gantt' }));
    expect('dependencyPropertyId' in before.fields).toBe(false);
    expect('ganttZoom' in before.fields).toBe(false);
    const after = { ...before, fields: { ...before.fields, dependencyPropertyId: 'after' } };
    const [forward, backward] = viewPatches(createBoardView(after), before);
    expect(forward.updatedFields).toEqual({ dependencyPropertyId: 'after' });
    // What the server is sent for the undo: a name to drop. An empty value would be sent as nothing.
    expect(backward.deletedFields).toEqual(['dependencyPropertyId']);
    expect(JSON.parse(JSON.stringify(backward))).toMatchObject({
      deletedFields: ['dependencyPropertyId'],
    });
    expect('dependencyPropertyId' in applyViewPatch(createBoardView(after), backward).fields).toBe(
      false,
    );
  });

  it('has "display by", a sort, and a grouping it can do without', () => {
    expect(displayByOffered('gantt')).toBe(true);
    expect(sortOffered('gantt')).toBe(true);
    expect(groupingOf('gantt')).toEqual({ offered: true, optional: true });
  });
});

describe('grouping', () => {
  it('is offered for boards and tables, and only a table can do without', () => {
    expect(groupingOf('board')).toEqual({ offered: true, optional: false });
    expect(groupingOf('table')).toEqual({ offered: true, optional: true });
    expect(groupingOf('gallery').offered).toBe(false);
    expect(groupingOf('calendar').offered).toBe(false);
  });

  it('can be by a select property, a person, or who made or changed the card', () => {
    expect(groupableProperties(TEMPLATES).map((template) => template.id)).toEqual([
      'status',
      'owner',
      'made',
    ]);
  });

  it('starts the columns over: no value first, then the options, nothing hidden or collapsed', () => {
    expect(groupingFields(TEMPLATES[1])).toEqual({
      groupBy: 'status',
      visibleGroups: ['', 'todo', 'done'],
      hiddenGroups: [],
      collapsedGroups: [],
    });
    // A person has no options: the columns are the people the cards have.
    expect(groupingFields(TEMPLATES[3]).visibleGroups).toEqual(['']);
  });

  it('by nothing is the empty identifier and no columns', () => {
    expect(groupingFields(undefined)).toEqual({
      groupBy: '',
      visibleGroups: [],
      hiddenGroups: [],
      collapsedGroups: [],
    });
  });

  it('of a view is the property it names; a board that names none uses the first select', () => {
    expect(groupProperty(viewOf({ viewType: 'board', groupBy: 'owner' }), TEMPLATES)?.id).toBe(
      'owner',
    );
    expect(groupProperty(viewOf({ viewType: 'board' }), TEMPLATES)?.id).toBe('status');
    expect(groupProperty(viewOf({ viewType: 'board', groupBy: 'gone' }), TEMPLATES)?.id).toBe(
      'status',
    );
    expect(groupProperty(viewOf({ viewType: 'table', groupBy: '' }), TEMPLATES)).toBeUndefined();
    expect(groupProperty(viewOf({ viewType: 'table' }), TEMPLATES)).toBeUndefined();
  });
});

describe('the properties a view shows', () => {
  it('can be every property of the board, and nothing a view does not read', () => {
    expect(propertyToggles(TEMPLATES, 'table').map((toggle) => toggle.id)).toEqual(
      TEMPLATES.map((template) => template.id),
    );
  });

  it('include a card relation in a table only: the face of a card does not show it', () => {
    const withRelation: PropertyTemplate[] = [
      ...TEMPLATES,
      { id: 'after', name: 'After', type: 'relation', options: [] },
    ];
    expect(propertyToggles(withRelation, 'table').map((toggle) => toggle.id)).toContain('after');
    for (const viewType of ['board', 'gallery', 'calendar'] as const) {
      expect(propertyToggles(withRelation, viewType).map((toggle) => toggle.id)).toEqual(
        TEMPLATES.map((template) => template.id),
      );
    }
  });

  it('get a newly shown one at the end, and lose a hidden one where it was', () => {
    expect(withPropertyVisible(['a', 'b'], 'c', true)).toEqual(['a', 'b', 'c']);
    expect(withPropertyVisible(['a', 'b', 'c'], 'b', false)).toEqual(['a', 'c']);
    expect(withPropertyVisible(['a', 'b'], 'a', true)).toEqual(['a', 'b']);
    expect(withPropertyVisible(['a', 'b'], 'z', false)).toEqual(['a', 'b']);
  });
});

describe('the sort of a view', () => {
  const options: readonly SortOption[] = [
    { propertyId: 'status', reversed: false },
    { propertyId: TITLE_COLUMN_ID, reversed: true },
  ];

  it('is not offered for a calendar', () => {
    expect(sortOffered('calendar')).toBe(false);
    expect(['board', 'table', 'gallery'].map((type) => sortOffered(type as never))).toEqual([
      true,
      true,
      true,
    ]);
  });

  it('can be added on the title and on every property, each once', () => {
    expect(sortCandidates([], TEMPLATES)).toEqual([
      TITLE_COLUMN_ID,
      'hours',
      'status',
      'tags',
      'owner',
      'made',
    ]);
    expect(sortCandidates(options, TEMPLATES)).toEqual(['hours', 'tags', 'owner', 'made']);
    expect(addSort(options, 'status')).toBe(options);
  });

  it('gets a new line at the end, ascending', () => {
    expect(addSort(options, 'hours')).toEqual([
      ...options,
      { propertyId: 'hours', reversed: false },
    ]);
  });

  it('reverses, removes and moves a line', () => {
    expect(reverseSort(options, 1)).toEqual([
      options[0],
      { propertyId: TITLE_COLUMN_ID, reversed: false },
    ]);
    expect(removeSort(options, 0)).toEqual([options[1]]);
    expect(moveSort(options, 1, -1)).toEqual([options[1], options[0]]);
    expect(moveSort(options, 0, 1)).toEqual([options[1], options[0]]);
  });

  it('returns the list it was given when there is nothing to do', () => {
    expect(moveSort(options, 0, -1)).toBe(options);
    expect(moveSort(options, 1, 1)).toBe(options);
    expect(reverseSort(options, 5)).toBe(options);
    expect(removeSort(options, 5)).toBe(options);
  });
});

describe('the lines of "…" for the tools without a button', () => {
  const open = {
    sort: () => undefined,
    'display-by': () => undefined,
    dependencies: () => undefined,
    group: () => undefined,
    properties: () => undefined,
  };
  const gantt: AsideOffer = {
    row: false,
    sort: 2,
    displayBy: 'Due',
    dependencies: 'After',
    group: 'None',
  };
  const ids = (offer: AsideOffer): string[] => asideTools(offer, open).map((line) => line.id);

  it('are every tool the view offers in the folded row, each with its choice', () => {
    expect(ids(gantt)).toEqual(['sort', 'display-by', 'dependencies', 'group', 'properties']);
    const lines = asideTools(gantt, open);
    expect(lines.map((line) => line.hint)).toEqual(['2', 'Due', 'After', 'None', undefined]);
    // A view without a sort set says no number; a kind of view without a tool has no line.
    expect(asideTools({ ...gantt, sort: 0 }, open)[0]?.hint).toBeUndefined();
    expect(
      ids({
        row: false,
        sort: undefined,
        displayBy: 'Due',
        dependencies: undefined,
        group: undefined,
      }),
    ).toEqual(['display-by', 'properties']);
  });

  it('are "dependencies" alone in the whole row, where the other tools have buttons', () => {
    expect(ids({ ...gantt, row: true })).toEqual(['dependencies']);
    expect(ids({ ...gantt, row: true, dependencies: undefined })).toEqual([]);
  });

  it('cut a long name short: the menu does not, and the line has to say what it is', () => {
    const long = 'The cards that have to be done before this one';
    const [line] = asideTools({ ...gantt, row: true, dependencies: long }, open);
    expect(line?.hint).toBe('The cards that have…');
    expect(asideTools({ ...gantt, row: true, dependencies: '선행 카드' }, open)[0]?.hint).toBe(
      '선행 카드',
    );
  });

  it('open the tool they name', () => {
    const opened: string[] = [];
    const lines = asideTools(gantt, {
      ...open,
      dependencies: () => void opened.push('dependencies'),
    });
    lines.find((line) => line.id === 'dependencies')?.run?.();
    expect(opened).toEqual(['dependencies']);
  });
});
