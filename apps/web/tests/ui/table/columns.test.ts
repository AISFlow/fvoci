/** The columns of a table view: which, in what order, how wide. */
import { describe, expect, it } from 'vitest';

import {
  clampWidth,
  gridTemplate,
  reorderedIds,
  sortDirection,
  steppedIds,
  storedTitleWidth,
  storedWidth,
  tableColumns,
  TABLE_SETTINGS,
} from '$lib/features/views/table/columns.ts';
import {
  createBoardView,
  TITLE_COLUMN_ID,
  type BoardView,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

const TEMPLATES: PropertyTemplate[] = [
  { id: 'status', name: 'Status', type: 'select', options: [] },
  { id: 'owner', name: 'Owner', type: 'person', options: [] },
  { id: 'hours', name: 'Hours', type: 'number', options: [] },
];

function viewOf(fields: Partial<BoardView['fields']>): BoardView {
  const view = createBoardView();
  view.fields = { ...view.fields, viewType: 'table', ...fields };
  return view;
}

describe('the columns of a view', () => {
  it('are its visible properties, in its order, at their stored widths', () => {
    const view = viewOf({
      shownProperties: ['hours', 'status'],
      columnWidths: { hours: 120, status: 240 },
    });
    expect(tableColumns(view, TEMPLATES).map(({ id, width }) => [id, width])).toEqual([
      ['hours', 120],
      ['status', 240],
    ]);
  });

  it('leave out a property the board no longer has, and one named twice', () => {
    const view = viewOf({ shownProperties: ['gone', 'status', 'status', 'owner'] });
    expect(tableColumns(view, TEMPLATES).map((column) => column.id)).toEqual(['status', 'owner']);
  });

  it('get the default width where the view stores none', () => {
    const view = viewOf({ shownProperties: ['status'], columnWidths: { status: 0 } });
    expect(tableColumns(view, TEMPLATES)[0]?.width).toBe(TABLE_SETTINGS.defaultColumnWidth);
    expect(storedWidth({}, 'status')).toBe(TABLE_SETTINGS.defaultColumnWidth);
  });

  it('show a width that is being dragged in place of the stored one', () => {
    const view = viewOf({
      shownProperties: ['status', 'owner'],
      columnWidths: { status: 200, owner: 200 },
    });
    const columns = tableColumns(view, TEMPLATES, { owner: 317 });
    expect(columns.map((column) => column.width)).toEqual([200, 317]);
  });

  it('never show a stored width outside the limits', () => {
    expect(storedWidth({ a: 20 }, 'a')).toBe(TABLE_SETTINGS.minColumnWidth);
    expect(storedWidth({ a: 5000 }, 'a')).toBe(TABLE_SETTINGS.maxColumnWidth);
  });
});

describe('a width', () => {
  it('is a whole number of pixels between the least and the most', () => {
    expect(clampWidth(150.4)).toBe(150);
    expect(clampWidth(3)).toBe(TABLE_SETTINGS.minColumnWidth);
    expect(clampWidth(99999)).toBe(TABLE_SETTINGS.maxColumnWidth);
  });

  it('stops at a narrower limit when one is given: half of the pane, for the title column', () => {
    expect(clampWidth(400, 206)).toBe(206);
    expect(clampWidth(180, 206)).toBe(180);
  });

  it('does not go under the least width even when the limit is lower', () => {
    expect(clampWidth(400, 60)).toBe(TABLE_SETTINGS.minColumnWidth);
  });
});

describe('the width of the title column', () => {
  it('is what the reader stored', () => {
    expect(storedTitleWidth({ [TITLE_COLUMN_ID]: 280 })).toBe(280);
  });

  it('is left to the pane when nothing is stored', () => {
    expect(storedTitleWidth({})).toBeUndefined();
    expect(storedTitleWidth({ [TITLE_COLUMN_ID]: 0 })).toBeUndefined();
  });
});

describe('the tracks of a row', () => {
  it('start with the title column, capped at half of the pane, and end with what is left', () => {
    expect(gridTemplate([120, 240])).toBe(
      'min(var(--table-title), 50cqw) 120px 240px minmax(0, 1fr)',
    );
  });

  it('are the title column and the rest when there are no properties', () => {
    expect(gridTemplate([])).toBe('min(var(--table-title), 50cqw) minmax(0, 1fr)');
  });
});

describe('moving a column', () => {
  const visible = ['gone', 'status', 'owner', 'hours'];
  const shown = ['status', 'owner', 'hours'];

  it('a step to the right swaps it with its neighbour', () => {
    expect(steppedIds(visible, shown, 'status', 1)).toEqual(['owner', 'status', 'hours', 'gone']);
  });

  it('a step to the left does the same the other way', () => {
    expect(steppedIds(visible, shown, 'hours', -1)).toEqual(['status', 'hours', 'owner', 'gone']);
  });

  it('is not possible past either end', () => {
    expect(steppedIds(visible, shown, 'status', -1)).toBeUndefined();
    expect(steppedIds(visible, shown, 'hours', 1)).toBeUndefined();
    expect(steppedIds(visible, shown, 'nowhere', 1)).toBeUndefined();
  });

  it('keeps the identifiers that have no column, after the shown ones', () => {
    expect(reorderedIds(visible, ['hours', 'status', 'owner'])).toEqual([
      'hours',
      'status',
      'owner',
      'gone',
    ]);
  });
});

describe('how a column sorts the view', () => {
  it('is told from the first sort of the view only', () => {
    const sorts = [
      { propertyId: 'status', reversed: true },
      { propertyId: 'hours', reversed: false },
    ];
    expect(sortDirection(sorts, 'status')).toBe('descending');
    expect(sortDirection(sorts, 'hours')).toBeUndefined();
    expect(sortDirection([{ propertyId: TITLE_COLUMN_ID, reversed: false }], TITLE_COLUMN_ID)).toBe(
      'ascending',
    );
    expect(sortDirection([], 'status')).toBeUndefined();
  });
});
