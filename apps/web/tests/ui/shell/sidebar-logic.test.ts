/** What the sidebar works out: the search, the order of categories, and where a dropped board is filed. */
import { describe, expect, it } from 'vitest';

import {
  containerMoveOf,
  categoryOrderAfterStep,
  dragOrder,
  dragZones,
  filedIndex,
  filterSidebar,
  moveToEnd,
} from '$lib/features/shell/sidebar-logic.ts';
import { createBoard, type Container } from '$lib/logic/wire.ts';
import type { Category, SidebarCategory } from '$lib/state/index.ts';

function board(id: string, title: string): Container {
  return { ...createBoard(), id, title };
}

function entry(
  id: string,
  boards: Container[],
  overrides: Partial<Category> = {},
): SidebarCategory {
  return {
    category: {
      id,
      name: id,
      userId: 'reader',
      workspaceId: 'workspace',
      type: 'custom',
      collapsed: false,
      sortOrder: 0,
      containerIds: boards.map((one) => one.id),
      hiddenContainerIds: [],
      createdAt: 0,
      updatedAt: 0,
      ...overrides,
    },
    containers: boards,
  };
}

const plan = board('plan', 'Launch Plan');
const notes = board('notes', 'Meeting notes');
const goals = board('goals', 'Personal Goals');
const untitled = board('untitled', '');

const SIDEBAR: SidebarCategory[] = [
  entry('workspace', [plan, notes]),
  entry('personal', [goals], { collapsed: true }),
  entry('boards', [untitled], { type: 'system' }),
];

describe('the search of the sidebar', () => {
  it('an empty query, or one of spaces, changes nothing', () => {
    expect(filterSidebar(SIDEBAR, '')).toBe(SIDEBAR);
    expect(filterSidebar(SIDEBAR, '   ')).toBe(SIDEBAR);
  });

  it('keeps the boards whose title has the text, whatever the case', () => {
    const found = filterSidebar(SIDEBAR, 'PLAN');
    expect(found.map((one) => one.category.id)).toEqual(['workspace']);
    expect(found[0]?.containers).toEqual([plan]);
  });

  it('leaves out a category without a match, and finds a board of a folded one', () => {
    const found = filterSidebar(SIDEBAR, 'goal');
    expect(found.map((one) => one.category.id)).toEqual(['personal']);
  });

  it('matches full-width letters and ignores the spaces around the query', () => {
    expect(filterSidebar(SIDEBAR, '  ｍｅｅｔｉｎｇ ')[0]?.containers).toEqual([notes]);
  });

  it('a board without a title is found by nothing', () => {
    expect(filterSidebar(SIDEBAR, 'untitled')).toEqual([]);
  });

  it('does not change the sidebar it was given', () => {
    filterSidebar(SIDEBAR, 'plan');
    expect(SIDEBAR[0]?.containers).toEqual([plan, notes]);
  });
});

describe('moving a category a step', () => {
  const ids = ['a', 'b', 'c'];

  it('moves it one place up or down', () => {
    expect(categoryOrderAfterStep(ids, 'b', -1)).toEqual(['b', 'a', 'c']);
    expect(categoryOrderAfterStep(ids, 'b', 1)).toEqual(['a', 'c', 'b']);
  });

  it('has no answer at the ends, or for a category that is not there', () => {
    expect(categoryOrderAfterStep(ids, 'a', -1)).toBeUndefined();
    expect(categoryOrderAfterStep(ids, 'c', 1)).toBeUndefined();
    expect(categoryOrderAfterStep(ids, 'x', 1)).toBeUndefined();
  });

  it('names every category once, and leaves the given order alone', () => {
    const next = categoryOrderAfterStep(ids, 'a', 1);
    expect([...(next ?? [])].sort()).toEqual(['a', 'b', 'c']);
    expect(ids).toEqual(['a', 'b', 'c']);
  });
});

describe('what a board can be dragged over', () => {
  it('open categories are lists with their boards in order', () => {
    expect(dragOrder(SIDEBAR)).toEqual([
      { id: 'workspace', items: ['plan', 'notes'] },
      { id: 'boards', items: ['untitled'] },
    ]);
  });

  it('folded categories are targets without places', () => {
    expect(dragZones(SIDEBAR)).toEqual(['personal']);
  });
});

describe('where a dropped board is filed', () => {
  it('in a category that shows all it names, the place on screen is the place in its list', () => {
    const category = { containerIds: ['a', 'b', 'c'] };
    expect(filedIndex(category, ['a', 'b', 'c'], 'x', 0)).toBe(0);
    expect(filedIndex(category, ['a', 'b', 'c'], 'x', 2)).toBe(2);
    expect(filedIndex(category, ['a', 'b', 'c'], 'x', 3)).toBe(3);
  });

  it('both places are counted without the board itself', () => {
    const category = { containerIds: ['a', 'b', 'c'] };
    // "a" dropped after "b": among the others it is second, and so it is filed.
    expect(filedIndex(category, ['a', 'b', 'c'], 'a', 1)).toBe(1);
    expect(filedIndex(category, ['a', 'b', 'c'], 'a', 2)).toBe(2);
    expect(filedIndex(category, ['a', 'b', 'c'], 'c', 0)).toBe(0);
  });

  it('a board the category names but does not show keeps its place before what follows it', () => {
    // "gone" was deleted: it is in the list and not on screen.
    const category = { containerIds: ['a', 'gone', 'b'] };
    // Dropped before "b" on screen: filed right before "b", after the one that is not shown.
    expect(filedIndex(category, ['a', 'b'], 'x', 1)).toBe(2);
    expect(filedIndex(category, ['a', 'b'], 'x', 0)).toBe(0);
    expect(filedIndex(category, ['a', 'b'], 'x', 2)).toBe(3);
  });

  it('before a board the category shows without naming it, the board goes to the end of the list', () => {
    // The default category also shows boards nobody filed ("loose").
    const category = { containerIds: ['a'] };
    expect(filedIndex(category, ['a', 'loose'], 'x', 1)).toBe(1);
  });

  it('a drop at a place in an open category', () => {
    expect(
      containerMoveOf({ kind: 'index', listId: 'workspace', index: 1 }, 'goals', SIDEBAR),
    ).toEqual({
      containerId: 'goals',
      categoryId: 'workspace',
      index: 1,
    });
  });

  it('a drop on a folded category files the board last there', () => {
    expect(containerMoveOf({ kind: 'zone', zoneId: 'personal' }, 'plan', SIDEBAR)).toEqual({
      containerId: 'plan',
      categoryId: 'personal',
      index: 1,
    });
  });

  it('"Move to" files the board last, and does not count the board itself', () => {
    expect(moveToEnd('workspace', 'goals', SIDEBAR)).toEqual({
      containerId: 'goals',
      categoryId: 'workspace',
      index: 2,
    });
    expect(moveToEnd('workspace', 'plan', SIDEBAR)?.index).toBe(1);
  });

  it('has no answer for a category that is gone', () => {
    expect(containerMoveOf({ kind: 'index', listId: 'nowhere', index: 0 }, 'plan', SIDEBAR)).toBe(
      undefined,
    );
    expect(moveToEnd('nowhere', 'plan', SIDEBAR)).toBeUndefined();
  });
});
