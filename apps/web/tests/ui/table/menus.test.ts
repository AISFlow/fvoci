/** The lines of the menu of a column's heading and of a row. */
import { describe, expect, it, vi } from 'vitest';

import { TABLE_SETTINGS } from '$lib/features/views/table/columns.ts';
import {
  headerMenu,
  rowMenu,
  type HeaderMenu,
  type RowMenu,
} from '$lib/features/views/table/menus.ts';

function header(overrides: Partial<HeaderMenu> = {}): HeaderMenu {
  return {
    sorted: undefined,
    place: { at: 1, count: 3 },
    width: 150,
    onSort: vi.fn(),
    onHide: vi.fn(),
    onMove: vi.fn(),
    onResize: vi.fn(),
    ...overrides,
  };
}

function row(overrides: Partial<RowMenu> = {}): RowMenu {
  return {
    canEdit: true,
    onOpen: vi.fn(),
    onDuplicate: vi.fn(),
    onCopyLink: vi.fn(),
    onDelete: vi.fn(),
    ...overrides,
  };
}

const ids = (actions: { id: string }[]): string[] => actions.map((action) => action.id);
const run = (actions: { id: string; run?: (() => void) | undefined }[], id: string): void => {
  actions.find((action) => action.id === id)?.run?.();
};

describe('the menu of a heading', () => {
  it('sorts, hides, moves a step and changes the width', () => {
    expect(ids(headerMenu(header()))).toEqual([
      'sortAscending',
      'sortDescending',
      'hide',
      'moveLeft',
      'moveRight',
      'wider',
      'narrower',
    ]);
  });

  it('of the title column neither hides nor moves', () => {
    expect(ids(headerMenu(header({ place: undefined })))).toEqual([
      'sortAscending',
      'sortDescending',
      'wider',
      'narrower',
    ]);
  });

  it('marks the direction the view is sorted in', () => {
    const actions = headerMenu(header({ sorted: 'descending' }));
    expect(actions.find((action) => action.id === 'sortDescending')?.checked).toBe(true);
    expect(actions.find((action) => action.id === 'sortAscending')?.checked).toBe(false);
  });

  it('does not offer a step past either end', () => {
    const first = headerMenu(header({ place: { at: 0, count: 3 } }));
    expect(first.find((action) => action.id === 'moveLeft')?.disabled).toBe(true);
    expect(first.find((action) => action.id === 'moveRight')?.disabled).toBe(false);
    const last = headerMenu(header({ place: { at: 2, count: 3 } }));
    expect(last.find((action) => action.id === 'moveRight')?.disabled).toBe(true);
    const only = headerMenu(header({ place: { at: 0, count: 1 } }));
    expect(only.find((action) => action.id === 'moveLeft')?.disabled).toBe(true);
    expect(only.find((action) => action.id === 'moveRight')?.disabled).toBe(true);
  });

  it('does not offer a width beyond the limits', () => {
    const narrow = headerMenu(header({ width: TABLE_SETTINGS.minColumnWidth }));
    expect(narrow.find((action) => action.id === 'narrower')?.disabled).toBe(true);
    const wide = headerMenu(header({ width: TABLE_SETTINGS.maxColumnWidth }));
    expect(wide.find((action) => action.id === 'wider')?.disabled).toBe(true);
  });

  it('tells the caller what was chosen', () => {
    const menu = header();
    const actions = headerMenu(menu);
    run(actions, 'sortDescending');
    expect(menu.onSort).toHaveBeenCalledExactlyOnceWith(true);
    run(actions, 'moveLeft');
    expect(menu.onMove).toHaveBeenCalledExactlyOnceWith(-1);
    run(actions, 'narrower');
    expect(menu.onResize).toHaveBeenCalledExactlyOnceWith(-TABLE_SETTINGS.menuStep);
    run(actions, 'hide');
    expect(menu.onHide).toHaveBeenCalledOnce();
  });
});

describe('the menu of a row', () => {
  it('opens, copies, gives the link and deletes, in that order, with deleting set apart', () => {
    const actions = rowMenu(row());
    expect(ids(actions)).toEqual(['open', 'duplicate', 'copyLink', 'delete']);
    const remove = actions.at(-1);
    expect(remove?.destructive).toBe(true);
    expect(remove?.separated).toBe(true);
  });

  it('offers a reader who may only look the card and its link', () => {
    expect(ids(rowMenu(row({ canEdit: false })))).toEqual(['open', 'copyLink']);
  });

  it('tells the caller what was chosen', () => {
    const menu = row();
    const actions = rowMenu(menu);
    run(actions, 'open');
    run(actions, 'delete');
    expect(menu.onOpen).toHaveBeenCalledOnce();
    expect(menu.onDelete).toHaveBeenCalledOnce();
    expect(menu.onDuplicate).not.toHaveBeenCalled();
  });
});
