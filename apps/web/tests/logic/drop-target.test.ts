/**
 * Where a dragged item lands: from a pointer and rectangles, and from steps
 * of the keyboard.
 *
 * The layout used throughout is three columns side by side, 100 wide with a
 * gap of 20 between them, each showing 300 of height from y = 50. Items are
 * 40 high with a gap of 10.
 */
import { describe, expect, test } from 'vitest';

import {
  clampToRect,
  dropTarget,
  edgeScrollFactor,
  insertionIndex,
  keyboardTarget,
  locateItem,
  moveItem,
  nearestList,
  sameTarget,
  type ListBox,
  type ListOrder,
  type Rect,
} from '$lib/logic/drop-target.ts';

const COLUMN_TOP = 50;
const COLUMN_HEIGHT = 300;

/** `count` items of height 40, 10 apart, in the column at `left`; `scrolled` moves them up. */
function stack(left: number, count: number, scrolled = 0): Rect[] {
  return Array.from({ length: count }, (_, index) => ({
    left,
    top: COLUMN_TOP + index * 50 - scrolled,
    width: 100,
    height: 40,
  }));
}

function column(id: string, left: number, count: number, scrolled = 0): ListBox {
  return {
    id,
    rect: { left, top: COLUMN_TOP, width: 100, height: COLUMN_HEIGHT },
    items: stack(left, count, scrolled),
  };
}

const board: ListBox[] = [column('a', 0, 3), column('b', 120, 5), column('c', 240, 0)];

describe('clampToRect', () => {
  const rect = { left: 10, top: 20, width: 100, height: 50 };

  test('a point inside stays where it is', () => {
    expect(clampToRect({ x: 30, y: 40 }, rect)).toEqual({ x: 30, y: 40 });
  });

  test('a point outside moves to the nearest edge or corner', () => {
    expect(clampToRect({ x: 0, y: 40 }, rect)).toEqual({ x: 10, y: 40 });
    expect(clampToRect({ x: 500, y: 500 }, rect)).toEqual({ x: 110, y: 70 });
    expect(clampToRect({ x: 30, y: -5 }, rect)).toEqual({ x: 30, y: 20 });
  });

  test('a rectangle without area is its own corner', () => {
    expect(clampToRect({ x: 5, y: 5 }, { left: 1, top: 2, width: 0, height: 0 })).toEqual({
      x: 1,
      y: 2,
    });
  });
});

describe('nearestList', () => {
  test('the list under the point', () => {
    expect(nearestList({ x: 150, y: 100 }, board)?.id).toBe('b');
  });

  test('in the space between two lists, the nearer one', () => {
    expect(nearestList({ x: 104, y: 100 }, board)?.id).toBe('a');
    expect(nearestList({ x: 116, y: 100 }, board)?.id).toBe('b');
  });

  test('exactly between two lists, the earlier one', () => {
    expect(nearestList({ x: 110, y: 100 }, board)?.id).toBe('a');
  });

  test('above, below and beyond the lists, still the nearest', () => {
    expect(nearestList({ x: 150, y: 0 }, board)?.id).toBe('b');
    expect(nearestList({ x: 150, y: 2000 }, board)?.id).toBe('b');
    expect(nearestList({ x: -300, y: 100 }, board)?.id).toBe('a');
    expect(nearestList({ x: 9000, y: 100 }, board)?.id).toBe('c');
  });

  test('below a short list next to a tall one, the short one: across the axis counts first', () => {
    const lists = [
      { id: 'short', rect: { left: 0, top: 0, width: 100, height: 50 } },
      { id: 'tall', rect: { left: 120, top: 0, width: 100, height: 400 } },
    ];
    expect(nearestList({ x: 50, y: 60 }, lists)?.id).toBe('short');
    // 250 below the short list and only 25 beside the tall one.
    expect(nearestList({ x: 95, y: 300 }, lists)?.id).toBe('short');
    expect(nearestList({ x: 111, y: 300 }, lists)?.id).toBe('tall');
  });

  test('lists on top of each other: among those in line, the nearest along the axis', () => {
    const stacked = [
      { id: 'top', rect: { left: 0, top: 0, width: 200, height: 100 } },
      { id: 'bottom', rect: { left: 0, top: 140, width: 200, height: 100 } },
    ];
    expect(nearestList({ x: 50, y: 110 }, stacked)?.id).toBe('top');
    expect(nearestList({ x: 50, y: 130 }, stacked)?.id).toBe('bottom');
    expect(nearestList({ x: 50, y: 900 }, stacked)?.id).toBe('bottom');
    expect(nearestList({ x: -40, y: 150 }, stacked)?.id).toBe('bottom');
  });

  test('a grid of lists: the column first, then the row', () => {
    const cell = (id: string, column: number, row: number) => ({
      id,
      rect: { left: column * 110, top: row * 60, width: 100, height: 50 },
    });
    const grid = [cell('a1', 0, 0), cell('b1', 1, 0), cell('a2', 0, 1), cell('b2', 1, 1)];
    expect(nearestList({ x: 150, y: 80 }, grid)?.id).toBe('b2');
    expect(nearestList({ x: 150, y: 54 }, grid)?.id).toBe('b1');
    expect(nearestList({ x: 104, y: 80 }, grid)?.id).toBe('a2');
  });

  test('lists that stack along x are told apart by y first', () => {
    const rows = [
      { id: 'short', rect: { left: 0, top: 0, width: 50, height: 40 } },
      { id: 'long', rect: { left: 0, top: 50, width: 400, height: 40 } },
    ];
    expect(nearestList({ x: 300, y: 20 }, rows, 'x')?.id).toBe('short');
    expect(nearestList({ x: 300, y: 20 }, rows, 'y')?.id).toBe('long');
  });

  test('no lists, no answer', () => {
    const none: ListBox[] = [];
    expect(nearestList({ x: 0, y: 0 }, none)).toBeUndefined();
  });
});

describe('insertionIndex', () => {
  const items = stack(0, 3); // middles at y = 70, 120, 170

  test('before the middle of the first item is the start', () => {
    expect(insertionIndex({ x: 50, y: 0 }, items)).toBe(0);
    expect(insertionIndex({ x: 50, y: 69 }, items)).toBe(0);
  });

  test('the middle of an item is the border: on it or past it, the place after', () => {
    expect(insertionIndex({ x: 50, y: 70 }, items)).toBe(1);
    expect(insertionIndex({ x: 50, y: 119 }, items)).toBe(1);
    expect(insertionIndex({ x: 50, y: 120 }, items)).toBe(2);
  });

  test('past the middle of the last item is the end', () => {
    expect(insertionIndex({ x: 50, y: 171 }, items)).toBe(3);
    expect(insertionIndex({ x: 50, y: 5000 }, items)).toBe(3);
  });

  test('an empty list has one place', () => {
    expect(insertionIndex({ x: 50, y: 100 }, [])).toBe(0);
  });

  test('items of different heights are split at their own middles', () => {
    const uneven = [
      { left: 0, top: 0, width: 100, height: 200 },
      { left: 0, top: 210, width: 100, height: 20 },
    ];
    expect(insertionIndex({ x: 0, y: 99 }, uneven)).toBe(0);
    expect(insertionIndex({ x: 0, y: 101 }, uneven)).toBe(1);
    expect(insertionIndex({ x: 0, y: 219 }, uneven)).toBe(1);
    expect(insertionIndex({ x: 0, y: 221 }, uneven)).toBe(2);
  });

  test('along the x axis the left and width decide', () => {
    const row = [
      { left: 0, top: 0, width: 80, height: 30 },
      { left: 90, top: 0, width: 80, height: 30 },
    ];
    expect(insertionIndex({ x: 39, y: 500 }, row, 'x')).toBe(0);
    expect(insertionIndex({ x: 41, y: -500 }, row, 'x')).toBe(1);
    expect(insertionIndex({ x: 131, y: 0 }, row, 'x')).toBe(2);
  });

  test('with the gap open at the answer, the answer stays', () => {
    // The dragged item is 40 high. Holding a gap for it at the answer pushes
    // the items from that index on down by 50.
    const point = { x: 50, y: 125 };
    const closed = stack(0, 3);
    const index = insertionIndex(point, closed);
    expect(index).toBe(2);
    const open = closed.map((rect, at) => (at >= index ? { ...rect, top: rect.top + 50 } : rect));
    expect(insertionIndex(point, open)).toBe(index);
  });
});

describe('dropTarget', () => {
  test('onto an item of another list: before it in its upper half, after it in its lower', () => {
    // Item 2 of column b spans y = 150..190.
    expect(dropTarget({ x: 170, y: 155 }, board)).toEqual({ listId: 'b', index: 2 });
    expect(dropTarget({ x: 170, y: 185 }, board)).toEqual({ listId: 'b', index: 3 });
  });

  test('into an empty list', () => {
    expect(dropTarget({ x: 290, y: 200 }, board)).toEqual({ listId: 'c', index: 0 });
  });

  test('below the last item of a short list is its end', () => {
    expect(dropTarget({ x: 50, y: 340 }, board)).toEqual({ listId: 'a', index: 3 });
  });

  test('no lists, no target', () => {
    expect(dropTarget({ x: 0, y: 0 }, [])).toBeUndefined();
  });

  test('a scrolled list: the pointer picks among the items as they are shown', () => {
    // Twenty items, scrolled by 400: item 8 is the first at the top (y = 50).
    const scrolled = [column('long', 0, 20, 400)];
    expect(dropTarget({ x: 50, y: 60 }, scrolled)).toEqual({ listId: 'long', index: 8 });
    expect(dropTarget({ x: 50, y: 80 }, scrolled)).toEqual({ listId: 'long', index: 9 });
  });

  test('a pointer above a scrolled list lands at its first visible item, not at its start', () => {
    const scrolled = [column('long', 0, 20, 400)];
    expect(dropTarget({ x: 50, y: -500 }, scrolled)).toEqual({ listId: 'long', index: 8 });
  });

  test('a pointer below a scrolled list lands after its last visible item, not at its end', () => {
    // The list shows y = 50..350; item 13 spans 300..340 and item 14 starts at 350.
    const scrolled = [column('long', 0, 20, 400)];
    expect(dropTarget({ x: 50, y: 5000 }, scrolled)).toEqual({ listId: 'long', index: 14 });
  });

  test('scrolling under a pointer that stays still moves the target', () => {
    const point = { x: 50, y: 200 };
    const targets = [0, 100, 200, 300].map(
      (scrolled) => dropTarget(point, [column('long', 0, 20, scrolled)])?.index,
    );
    expect(targets).toEqual([3, 5, 7, 9]);
  });

  test('a list scrolled out of the board cannot be hit: the pointer counts as being on the edge', () => {
    // The board shows x = 100..360 and is scrolled so that column a lies left of it.
    const lists = [column('a', -20, 3), column('b', 100, 5), column('c', 220, 0)];
    const bounds = { left: 100, top: 0, width: 260, height: 400 };
    expect(dropTarget({ x: 30, y: 155 }, lists)).toEqual({ listId: 'a', index: 2 });
    expect(dropTarget({ x: 30, y: 155 }, lists, { bounds })).toEqual({ listId: 'b', index: 2 });
  });

  test('lists that stack along x', () => {
    const row: ListBox = {
      id: 'row',
      rect: { left: 0, top: 0, width: 300, height: 40 },
      items: [
        { left: 0, top: 0, width: 80, height: 40 },
        { left: 90, top: 0, width: 80, height: 40 },
      ],
    };
    expect(dropTarget({ x: 100, y: 20 }, [row], { axis: 'x' })).toEqual({
      listId: 'row',
      index: 1,
    });
  });
});

describe('sameTarget', () => {
  test('compares list and index', () => {
    expect(sameTarget({ listId: 'a', index: 1 }, { listId: 'a', index: 1 })).toBe(true);
    expect(sameTarget({ listId: 'a', index: 1 }, { listId: 'a', index: 2 })).toBe(false);
    expect(sameTarget({ listId: 'a', index: 1 }, { listId: 'b', index: 1 })).toBe(false);
  });
});

const order: ListOrder[] = [
  { id: 'a', items: ['a1', 'a2', 'a3'] },
  { id: 'b', items: ['b1', 'b2'] },
  { id: 'c', items: [] },
];

describe('locateItem', () => {
  test('the list and index of an item', () => {
    expect(locateItem(order, 'a3')).toEqual({ listId: 'a', index: 2 });
    expect(locateItem(order, 'b1')).toEqual({ listId: 'b', index: 0 });
  });

  test('an unknown item is nowhere', () => {
    expect(locateItem(order, 'zz')).toBeUndefined();
  });
});

describe('moveItem', () => {
  test('to another list, at a chosen index', () => {
    expect(moveItem(order, 'a2', { listId: 'b', index: 1 })).toEqual([
      { id: 'a', items: ['a1', 'a3'] },
      { id: 'b', items: ['b1', 'a2', 'b2'] },
      { id: 'c', items: [] },
    ]);
  });

  test('into an empty list', () => {
    expect(moveItem(order, 'b2', { listId: 'c', index: 0 })[2]).toEqual({
      id: 'c',
      items: ['b2'],
    });
  });

  test('down within its list: the index counts the other items', () => {
    expect(moveItem(order, 'a1', { listId: 'a', index: 2 })[0]?.items).toEqual(['a2', 'a3', 'a1']);
    expect(moveItem(order, 'a1', { listId: 'a', index: 1 })[0]?.items).toEqual(['a2', 'a1', 'a3']);
  });

  test('up within its list', () => {
    expect(moveItem(order, 'a3', { listId: 'a', index: 0 })[0]?.items).toEqual(['a3', 'a1', 'a2']);
  });

  test('to the place it is at changes nothing', () => {
    const at = locateItem(order, 'a2');
    expect(at).toBeDefined();
    if (at) expect(moveItem(order, 'a2', at)).toEqual(order);
  });

  test('an index past the end is the end', () => {
    expect(moveItem(order, 'a1', { listId: 'b', index: 99 })[1]?.items).toEqual(['b1', 'b2', 'a1']);
  });

  test('the lists passed in are not changed and untouched lists are kept', () => {
    const before = structuredClone(order);
    const after = moveItem(order, 'a2', { listId: 'b', index: 0 });
    expect(order).toEqual(before);
    expect(after[2]).toBe(order[2]);
  });

  test('an unknown item or list changes nothing', () => {
    expect(moveItem(order, 'zz', { listId: 'a', index: 0 })).toEqual(order);
    expect(moveItem(order, 'a1', { listId: 'zz', index: 0 })).toEqual(order);
  });
});

describe('keyboardTarget', () => {
  // The carried item is a2, picked up at index 1 of list a; the others of that list are a1, a3.
  const carried = 'a2';
  const step = (
    current: { listId: string; index: number },
    move: Parameters<typeof keyboardTarget>[1],
  ) => keyboardTarget(current, move, order, carried);

  test('up and down within the list', () => {
    expect(step({ listId: 'a', index: 1 }, 'previous')).toEqual({ listId: 'a', index: 0 });
    expect(step({ listId: 'a', index: 1 }, 'next')).toEqual({ listId: 'a', index: 2 });
  });

  test('stops at the ends of the list', () => {
    expect(step({ listId: 'a', index: 0 }, 'previous')).toEqual({ listId: 'a', index: 0 });
    // Two other items, so index 2 is the end.
    expect(step({ listId: 'a', index: 2 }, 'next')).toEqual({ listId: 'a', index: 2 });
  });

  test('to the first and last place', () => {
    expect(step({ listId: 'a', index: 1 }, 'first')).toEqual({ listId: 'a', index: 0 });
    expect(step({ listId: 'a', index: 1 }, 'last')).toEqual({ listId: 'a', index: 2 });
    // In another list the carried item is not among the items, so the end is their count.
    expect(step({ listId: 'b', index: 0 }, 'last')).toEqual({ listId: 'b', index: 2 });
  });

  test('to the next list, keeping the index', () => {
    expect(step({ listId: 'a', index: 1 }, 'nextList')).toEqual({ listId: 'b', index: 1 });
  });

  test('to a shorter list, at its end', () => {
    expect(step({ listId: 'b', index: 2 }, 'nextList')).toEqual({ listId: 'c', index: 0 });
  });

  test('back to its own list, where the carried item does not count', () => {
    expect(step({ listId: 'b', index: 2 }, 'previousList')).toEqual({ listId: 'a', index: 2 });
  });

  test('there is no list before the first or after the last', () => {
    expect(step({ listId: 'a', index: 1 }, 'previousList')).toEqual({ listId: 'a', index: 1 });
    expect(step({ listId: 'c', index: 0 }, 'nextList')).toEqual({ listId: 'c', index: 0 });
  });

  test('a place in an unknown list is returned unchanged by a step across', () => {
    expect(step({ listId: 'zz', index: 4 }, 'nextList')).toEqual({ listId: 'zz', index: 4 });
  });

  test('a walk and the move it ends in', () => {
    let place = locateItem(order, carried) ?? { listId: '', index: -1 };
    for (const move of ['next', 'nextList', 'previous', 'previous', 'previous'] as const) {
      place = step(place, move);
    }
    expect(place).toEqual({ listId: 'b', index: 0 });
    expect(moveItem(order, carried, place)).toEqual([
      { id: 'a', items: ['a1', 'a3'] },
      { id: 'b', items: ['a2', 'b1', 'b2'] },
      { id: 'c', items: [] },
    ]);
  });
});

describe('edgeScrollFactor', () => {
  // A box from 100 to 500 with zones of 50 at both ends.
  const factor = (position: number) => edgeScrollFactor(position, 100, 400, 50);

  test('nothing in the middle', () => {
    expect(factor(300)).toBe(0);
    expect(factor(150)).toBe(0);
    expect(factor(450)).toBe(0);
  });

  test('grows toward the end', () => {
    expect(factor(475)).toBeCloseTo(0.5);
    expect(factor(490)).toBeCloseTo(0.8);
    expect(factor(500)).toBe(1);
  });

  test('negative toward the start', () => {
    expect(factor(125)).toBeCloseTo(-0.5);
    expect(factor(100)).toBe(-1);
  });

  test('full beyond the box', () => {
    expect(factor(-1000)).toBe(-1);
    expect(factor(9000)).toBe(1);
  });

  test('a short box keeps a middle', () => {
    // 60 long: zones of 20, so 30 is in the middle.
    expect(edgeScrollFactor(30, 0, 60, 50)).toBe(0);
    expect(edgeScrollFactor(10, 0, 60, 50)).toBeCloseTo(-0.5);
    expect(edgeScrollFactor(50, 0, 60, 50)).toBeCloseTo(0.5);
  });

  test('a box or a zone without size never scrolls', () => {
    expect(edgeScrollFactor(0, 0, 0, 50)).toBe(0);
    expect(edgeScrollFactor(0, 0, 400, 0)).toBe(0);
  });
});
