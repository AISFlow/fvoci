/**
 * What a drop means beyond "a place in a list": outside the board (a
 * cancel), a list that places items itself (a sorted view), a zone (a target
 * that is no list), and several items dropped as one.
 *
 * The layout is that of drop-target.test.ts: columns 100 wide with a gap of
 * 20, showing 300 of height from y = 50; items 40 high with a gap of 10.
 */
import { describe, expect, test } from 'vitest';

import {
  carriedItems,
  containsPoint,
  dropChanges,
  dropRegion,
  dropTarget,
  gapFor,
  insertionIndex,
  intersectRect,
  keyboardDestination,
  keyboardTarget,
  locateItem,
  moveItem,
  moveItems,
  sameDestination,
  windowChanged,
  type Destination,
  type KeyboardScene,
  type KeyboardStep,
  type ListOrder,
  type Rect,
} from '$lib/logic/drop-target.ts';

const order: ListOrder[] = [
  { id: 'a', items: ['a1', 'a2', 'a3', 'a4'] },
  { id: 'b', items: ['b1', 'b2'] },
  { id: 'c', items: [] },
];

describe('containsPoint', () => {
  const rect = { left: 10, top: 20, width: 100, height: 50 };

  test('inside, and on the edges and corners', () => {
    expect(containsPoint(rect, { x: 50, y: 40 })).toBe(true);
    expect(containsPoint(rect, { x: 10, y: 20 })).toBe(true);
    expect(containsPoint(rect, { x: 110, y: 70 })).toBe(true);
  });

  test('just past an edge is outside', () => {
    expect(containsPoint(rect, { x: 9.5, y: 40 })).toBe(false);
    expect(containsPoint(rect, { x: 110.5, y: 40 })).toBe(false);
    expect(containsPoint(rect, { x: 50, y: 19 })).toBe(false);
    expect(containsPoint(rect, { x: 50, y: 71 })).toBe(false);
  });

  test('a rectangle without area holds nothing, not even its own corner', () => {
    expect(containsPoint({ left: 10, top: 20, width: 0, height: 50 }, { x: 10, y: 30 })).toBe(
      false,
    );
    expect(containsPoint({ left: 10, top: 20, width: 100, height: 0 }, { x: 50, y: 20 })).toBe(
      false,
    );
  });
});

describe('intersectRect', () => {
  const page = { left: 0, top: 0, width: 400, height: 300 };

  test('a rectangle inside another is itself', () => {
    const inner = { left: 10, top: 20, width: 100, height: 50 };
    expect(intersectRect(inner, page)).toEqual(inner);
    expect(intersectRect(page, inner)).toEqual(inner);
  });

  test('a rectangle that sticks out is cut off', () => {
    expect(intersectRect({ left: -50, top: 250, width: 100, height: 100 }, page)).toEqual({
      left: 0,
      top: 250,
      width: 50,
      height: 50,
    });
  });

  test('rectangles that do not meet share nothing', () => {
    const shared = intersectRect({ left: 500, top: 0, width: 100, height: 100 }, page);
    expect(shared.width).toBe(0);
    expect(containsPoint(shared, { x: 500, y: 50 })).toBe(false);
  });
});

describe('dropRegion', () => {
  const bounds: Rect = { left: 0, top: 40, width: 480, height: 320 };
  const lists = [
    { id: 'a', rect: { left: 0, top: 50, width: 100, height: 300 } },
    { id: 'b', rect: { left: 120, top: 50, width: 100, height: 300 } },
  ];
  const zones = [{ id: 'hidden', rect: { left: 360, top: 50, width: 100, height: 80 } }];
  const region = (x: number, y: number) => dropRegion({ x, y }, { bounds, zones, lists });

  test('over a list: that list', () => {
    expect(region(150, 200)).toEqual({ kind: 'list', list: lists[1] });
  });

  test('inside the bounds but over no list: the nearest list', () => {
    expect(region(240, 200)).toEqual({ kind: 'list', list: lists[1] });
    expect(region(110, 45)).toEqual({ kind: 'list', list: lists[0] });
  });

  test('outside the bounds: outside, however near a list is', () => {
    expect(region(150, 39)).toEqual({ kind: 'outside' });
    expect(region(-1, 200)).toEqual({ kind: 'outside' });
    expect(region(481, 200)).toEqual({ kind: 'outside' });
    expect(region(150, 361)).toEqual({ kind: 'outside' });
  });

  test('the edge of the bounds is still inside', () => {
    expect(region(0, 40)).toEqual({ kind: 'list', list: lists[0] });
    expect(region(480, 360)).toEqual({ kind: 'list', list: lists[1] });
  });

  test('on a zone: the zone, and only while the pointer is on it', () => {
    expect(region(400, 100)).toEqual({ kind: 'zone', zoneId: 'hidden' });
    expect(region(400, 131)).toEqual({ kind: 'list', list: lists[1] });
    expect(region(359, 100)).toEqual({ kind: 'list', list: lists[1] });
  });

  test('a zone that lies on a list wins over the list', () => {
    const onList = [{ id: 'top', rect: { left: 0, top: 50, width: 100, height: 30 } }];
    expect(dropRegion({ x: 50, y: 60 }, { bounds, zones: onList, lists })).toEqual({
      kind: 'zone',
      zoneId: 'top',
    });
  });

  test('of two zones under the pointer, the first', () => {
    const two = [
      { id: 'first', rect: { left: 300, top: 50, width: 100, height: 100 } },
      { id: 'second', rect: { left: 350, top: 50, width: 100, height: 100 } },
    ];
    expect(dropRegion({ x: 375, y: 60 }, { bounds, zones: two, lists })).toEqual({
      kind: 'zone',
      zoneId: 'first',
    });
  });

  test('a zone outside the bounds cannot be hit', () => {
    const away = [{ id: 'away', rect: { left: 500, top: 50, width: 100, height: 80 } }];
    expect(dropRegion({ x: 550, y: 60 }, { bounds, zones: away, lists })).toEqual({
      kind: 'outside',
    });
  });

  test('without bounds nothing is outside', () => {
    expect(dropRegion({ x: 9000, y: -9000 }, { lists })).toEqual({ kind: 'list', list: lists[1] });
  });

  test('bounds without area, a board scrolled out of the window: everything is outside', () => {
    const gone = { left: 0, top: 0, width: 400, height: 0 };
    expect(dropRegion({ x: 50, y: 0 }, { bounds: gone, zones, lists })).toEqual({
      kind: 'outside',
    });
  });

  test('inside, with neither lists nor zones: nothing', () => {
    expect(dropRegion({ x: 50, y: 60 }, { bounds, lists: [] })).toBeUndefined();
  });

  test('lists in a row along x are told apart by y first', () => {
    const rows = [
      { id: 'top', rect: { left: 0, top: 50, width: 400, height: 40 } },
      { id: 'bottom', rect: { left: 0, top: 110, width: 400, height: 40 } },
    ];
    expect(dropRegion({ x: 450, y: 120 }, { bounds, lists: rows, axis: 'x' })).toEqual({
      kind: 'list',
      list: rows[1],
    });
  });
});

describe('a row that runs right to left', () => {
  // Three items in reading order, the first at the right: at x = 220, 110 and 0, each 100 wide.
  const row: Rect[] = [220, 110, 0].map((left) => ({ left, top: 0, width: 100, height: 40 }));

  test('insertionIndex counts the items whose middle is to the right of the point', () => {
    expect(insertionIndex({ x: 300, y: 20 }, row, 'x', true)).toBe(0);
    expect(insertionIndex({ x: 271, y: 20 }, row, 'x', true)).toBe(0);
    expect(insertionIndex({ x: 269, y: 20 }, row, 'x', true)).toBe(1);
    expect(insertionIndex({ x: 159, y: 20 }, row, 'x', true)).toBe(2);
    expect(insertionIndex({ x: 10, y: 20 }, row, 'x', true)).toBe(3);
  });

  test('the same points without the flag give the mirror image', () => {
    const forward = [...row].reverse();
    for (const x of [300, 269, 159, 10]) {
      const reversed = insertionIndex({ x, y: 20 }, row, 'x', true);
      expect(insertionIndex({ x, y: 20 }, forward, 'x')).toBe(row.length - reversed);
    }
  });

  test('dropTarget passes the flag on', () => {
    const list = { id: 'row', rect: { left: 0, top: 0, width: 320, height: 40 }, items: row };
    expect(dropTarget({ x: 200, y: 20 }, [list], { axis: 'x', reversed: true })).toEqual({
      listId: 'row',
      index: 1,
    });
    expect(dropTarget({ x: 400, y: 20 }, [list], { axis: 'x', reversed: true })).toEqual({
      listId: 'row',
      index: 0,
    });
  });
});

describe('sameDestination', () => {
  const place: Destination = { kind: 'index', listId: 'a', index: 1 };

  test('the same kind and the same ids', () => {
    expect(sameDestination(place, { kind: 'index', listId: 'a', index: 1 })).toBe(true);
    expect(sameDestination({ kind: 'list', listId: 'a' }, { kind: 'list', listId: 'a' })).toBe(
      true,
    );
    expect(sameDestination({ kind: 'zone', zoneId: 'z' }, { kind: 'zone', zoneId: 'z' })).toBe(
      true,
    );
  });

  test('another index, list, zone or kind', () => {
    expect(sameDestination(place, { kind: 'index', listId: 'a', index: 2 })).toBe(false);
    expect(sameDestination(place, { kind: 'index', listId: 'b', index: 1 })).toBe(false);
    expect(sameDestination(place, { kind: 'list', listId: 'a' })).toBe(false);
    expect(sameDestination({ kind: 'list', listId: 'a' }, { kind: 'list', listId: 'b' })).toBe(
      false,
    );
    expect(sameDestination({ kind: 'zone', zoneId: 'a' }, { kind: 'list', listId: 'a' })).toBe(
      false,
    );
    expect(sameDestination({ kind: 'zone', zoneId: 'y' }, { kind: 'zone', zoneId: 'z' })).toBe(
      false,
    );
  });

  test('no destination equals only no destination', () => {
    expect(sameDestination(undefined, undefined)).toBe(true);
    expect(sameDestination(undefined, place)).toBe(false);
    expect(sameDestination(place, undefined)).toBe(false);
  });
});

describe('locateItem without the carried items', () => {
  test('the index counts only the items that stay', () => {
    expect(locateItem(order, 'a4', ['a2', 'a4'])).toEqual({ listId: 'a', index: 2 });
    expect(locateItem(order, 'a2', ['a2', 'a4'])).toEqual({ listId: 'a', index: 1 });
    expect(locateItem(order, 'a3', ['a1', 'a2', 'a3'])).toEqual({ listId: 'a', index: 0 });
  });

  test('carried items of other lists do not count', () => {
    expect(locateItem(order, 'a3', ['b1', 'a3'])).toEqual({ listId: 'a', index: 2 });
  });

  test('one carried item: the index it has', () => {
    expect(locateItem(order, 'a3', ['a3'])).toEqual(locateItem(order, 'a3'));
  });
});

describe('carriedItems', () => {
  test('nothing selected: the grabbed item alone', () => {
    expect(carriedItems(order, 'a2')).toEqual(['a2']);
    expect(carriedItems(order, 'a2', [])).toEqual(['a2']);
  });

  test('the grabbed item is not among the selected: alone, and the selection stays out of it', () => {
    expect(carriedItems(order, 'a2', ['a1', 'b1'])).toEqual(['a2']);
  });

  test('the grabbed item is selected: all the selected, in the order of the data', () => {
    expect(carriedItems(order, 'a3', ['b2', 'a3', 'a1'])).toEqual(['a1', 'a3', 'b2']);
  });

  test('selected items that are in no list are left out', () => {
    expect(carriedItems(order, 'a3', ['a3', 'gone'])).toEqual(['a3']);
  });
});

describe('moveItems', () => {
  test('several items of one list go to another as a run, in their order', () => {
    expect(moveItems(order, ['a3', 'a1'], { listId: 'b', index: 1 })).toEqual([
      { id: 'a', items: ['a2', 'a4'] },
      { id: 'b', items: ['b1', 'a1', 'a3', 'b2'] },
      { id: 'c', items: [] },
    ]);
  });

  test('items from several lists: list by list, start to end', () => {
    expect(moveItems(order, ['b2', 'a4', 'a2'], { listId: 'c', index: 0 })).toEqual([
      { id: 'a', items: ['a1', 'a3'] },
      { id: 'b', items: ['b1'] },
      { id: 'c', items: ['a2', 'a4', 'b2'] },
    ]);
  });

  test('items from several lists into one of those lists', () => {
    expect(moveItems(order, ['a1', 'b1'], { listId: 'b', index: 1 })[1]?.items).toEqual([
      'b2',
      'a1',
      'b1',
    ]);
  });

  test('the index counts the target list without any of the moved items', () => {
    // Without a1 and a3 the list is a2, a4: index 1 is between them.
    expect(moveItems(order, ['a1', 'a3'], { listId: 'a', index: 1 })[0]?.items).toEqual([
      'a2',
      'a1',
      'a3',
      'a4',
    ]);
    expect(moveItems(order, ['a1', 'a3'], { listId: 'a', index: 2 })[0]?.items).toEqual([
      'a2',
      'a4',
      'a1',
      'a3',
    ]);
  });

  test('a place between the chosen items closes them up around it', () => {
    // a1 and a4 chosen, dropped where a1 was: a4 comes up to join it.
    expect(moveItems(order, ['a1', 'a4'], { listId: 'a', index: 0 })[0]?.items).toEqual([
      'a1',
      'a4',
      'a2',
      'a3',
    ]);
  });

  test('items next to each other, dropped where they are: nothing changes', () => {
    expect(moveItems(order, ['a2', 'a3'], { listId: 'a', index: 1 })).toEqual(order);
  });

  test('an index past the end is the end, and Infinity is past every end', () => {
    expect(moveItems(order, ['a1', 'a2'], { listId: 'b', index: 99 })[1]?.items).toEqual([
      'b1',
      'b2',
      'a1',
      'a2',
    ]);
    expect(moveItems(order, ['a1'], { listId: 'a', index: Infinity })[0]?.items).toEqual([
      'a2',
      'a3',
      'a4',
      'a1',
    ]);
  });

  test('unknown items are left out; with none left, or an unknown list, nothing changes', () => {
    expect(moveItems(order, ['zz', 'a1'], { listId: 'c', index: 0 })[2]?.items).toEqual(['a1']);
    expect(moveItems(order, ['zz'], { listId: 'c', index: 0 })).toEqual(order);
    expect(moveItems(order, [], { listId: 'c', index: 0 })).toEqual(order);
    expect(moveItems(order, ['a1'], { listId: 'zz', index: 0 })).toEqual(order);
  });

  test('an item named twice moves once', () => {
    expect(moveItems(order, ['a1', 'a1'], { listId: 'c', index: 0 })[2]?.items).toEqual(['a1']);
  });

  test('the lists passed in are not changed and untouched lists are kept', () => {
    const before = structuredClone(order);
    const after = moveItems(order, ['a1', 'a2'], { listId: 'c', index: 0 });
    expect(order).toEqual(before);
    expect(after[1]).toBe(order[1]);
  });

  test('one item: the same as moveItem', () => {
    const target = { listId: 'b', index: 1 };
    expect(moveItems(order, ['a2'], target)).toEqual(moveItem(order, 'a2', target));
  });
});

describe('dropChanges', () => {
  test('at the place the item is at: nothing', () => {
    expect(dropChanges(order, ['a2'], { kind: 'index', listId: 'a', index: 1 })).toBe(false);
  });

  test('at another place: something', () => {
    expect(dropChanges(order, ['a2'], { kind: 'index', listId: 'a', index: 0 })).toBe(true);
    expect(dropChanges(order, ['a2'], { kind: 'index', listId: 'b', index: 0 })).toBe(true);
  });

  test('several items that already stand together at that place: nothing', () => {
    expect(dropChanges(order, ['a2', 'a3'], { kind: 'index', listId: 'a', index: 1 })).toBe(false);
  });

  test('several items that stand apart, wherever in their list they are dropped: something', () => {
    for (const index of [0, 1, 2]) {
      expect(dropChanges(order, ['a1', 'a3'], { kind: 'index', listId: 'a', index })).toBe(true);
    }
  });

  test('into a list that places items itself: only when an item comes from elsewhere', () => {
    expect(dropChanges(order, ['a2'], { kind: 'list', listId: 'a' })).toBe(false);
    expect(dropChanges(order, ['a1', 'a3'], { kind: 'list', listId: 'a' })).toBe(false);
    expect(dropChanges(order, ['a2'], { kind: 'list', listId: 'b' })).toBe(true);
    expect(dropChanges(order, ['a2', 'b1'], { kind: 'list', listId: 'a' })).toBe(true);
  });

  test('onto a zone: always', () => {
    expect(dropChanges(order, ['a2'], { kind: 'zone', zoneId: 'hidden' })).toBe(true);
  });

  test('items that are in no list, or a list that is unknown: nothing', () => {
    expect(dropChanges(order, ['zz'], { kind: 'zone', zoneId: 'hidden' })).toBe(false);
    expect(dropChanges(order, ['zz'], { kind: 'list', listId: 'a' })).toBe(false);
    expect(dropChanges(order, ['a1'], { kind: 'list', listId: 'zz' })).toBe(false);
    expect(dropChanges(order, ['a1'], { kind: 'index', listId: 'zz', index: 0 })).toBe(false);
  });
});

describe('gapFor', () => {
  const from = { listId: 'a', index: 1 };
  const scene = { from, lists: order, carried: ['a2'] };

  test('a place in a list: there', () => {
    expect(gapFor({ kind: 'index', listId: 'b', index: 2 }, scene)).toEqual({
      listId: 'b',
      index: 2,
    });
  });

  test('no destination, the pointer is outside: back where the item was', () => {
    expect(gapFor(undefined, scene)).toEqual(from);
  });

  test('a zone: no gap', () => {
    expect(gapFor({ kind: 'zone', zoneId: 'hidden' }, scene)).toBeUndefined();
  });

  test('a list that places items itself: where its order puts the item', () => {
    const sortedIndex = (listId: string) => (listId === 'b' ? 1 : undefined);
    expect(gapFor({ kind: 'list', listId: 'b' }, { ...scene, sortedIndex })).toEqual({
      listId: 'b',
      index: 1,
    });
  });

  test('the list the item came from: where it was, whatever the order says', () => {
    const sortedIndex = () => 3;
    expect(gapFor({ kind: 'list', listId: 'a' }, { ...scene, sortedIndex })).toEqual(from);
  });

  test('nobody says where the order puts it: the end of the list', () => {
    expect(gapFor({ kind: 'list', listId: 'b' }, scene)).toEqual({ listId: 'b', index: 2 });
    expect(gapFor({ kind: 'list', listId: 'c' }, scene)).toEqual({ listId: 'c', index: 0 });
    const silent = () => undefined;
    expect(gapFor({ kind: 'list', listId: 'b' }, { ...scene, sortedIndex: silent })).toEqual({
      listId: 'b',
      index: 2,
    });
  });

  test('the end counts the list without the carried items', () => {
    const carried = ['a2', 'b1'];
    expect(gapFor({ kind: 'list', listId: 'b' }, { from, lists: order, carried })).toEqual({
      listId: 'b',
      index: 1,
    });
  });

  test('an index the order gives that is out of range is pulled in', () => {
    expect(gapFor({ kind: 'list', listId: 'b' }, { ...scene, sortedIndex: () => 99 })).toEqual({
      listId: 'b',
      index: 2,
    });
    expect(gapFor({ kind: 'list', listId: 'b' }, { ...scene, sortedIndex: () => -1 })).toEqual({
      listId: 'b',
      index: 0,
    });
  });
});

describe('keyboardTarget with several carried items', () => {
  test('the ends of a list count it without any of them', () => {
    const carried = ['a1', 'a3'];
    expect(keyboardTarget({ listId: 'a', index: 0 }, 'last', order, carried)).toEqual({
      listId: 'a',
      index: 2,
    });
    expect(keyboardTarget({ listId: 'a', index: 2 }, 'next', order, carried)).toEqual({
      listId: 'a',
      index: 2,
    });
  });

  test('a walk and the move it ends in', () => {
    const carried = ['a1', 'a3'];
    let place = locateItem(order, 'a3', carried) ?? { listId: '', index: -1 };
    expect(place).toEqual({ listId: 'a', index: 1 });
    for (const step of ['next', 'nextList', 'previous'] as const) {
      place = keyboardTarget(place, step, order, carried);
    }
    expect(place).toEqual({ listId: 'b', index: 1 });
    expect(moveItems(order, carried, place)).toEqual([
      { id: 'a', items: ['a2', 'a4'] },
      { id: 'b', items: ['b1', 'a1', 'a3', 'b2'] },
      { id: 'c', items: [] },
    ]);
  });
});

describe('keyboardDestination', () => {
  const free: KeyboardScene = { lists: order, zones: ['hidden', 'bin'], carried: ['a2'] };
  const sorted: KeyboardScene = { ...free, placement: 'list' };
  const walk = (from: Destination, steps: KeyboardStep[], scene: KeyboardScene): Destination =>
    steps.reduce((at, step) => keyboardDestination(at, step, scene), from);

  describe('free placement', () => {
    const start: Destination = { kind: 'index', listId: 'a', index: 1 };

    test('within and between lists the steps are those of keyboardTarget', () => {
      expect(walk(start, ['next'], free)).toEqual({ kind: 'index', listId: 'a', index: 2 });
      expect(walk(start, ['last'], free)).toEqual({ kind: 'index', listId: 'a', index: 3 });
      expect(walk(start, ['nextList'], free)).toEqual({ kind: 'index', listId: 'b', index: 1 });
      expect(walk(start, ['nextList', 'nextList'], free)).toEqual({
        kind: 'index',
        listId: 'c',
        index: 0,
      });
    });

    test('a step that leads nowhere returns the destination it was given', () => {
      const first: Destination = { kind: 'index', listId: 'a', index: 0 };
      expect(keyboardDestination(first, 'previous', free)).toBe(first);
      expect(keyboardDestination(first, 'previousList', free)).toBe(first);
    });

    test('past the last list come the zones, in order, and there it ends', () => {
      const last: Destination = { kind: 'index', listId: 'c', index: 0 };
      expect(walk(last, ['nextList'], free)).toEqual({ kind: 'zone', zoneId: 'hidden' });
      expect(walk(last, ['nextList', 'nextList'], free)).toEqual({ kind: 'zone', zoneId: 'bin' });
      expect(walk(last, ['nextList', 'nextList', 'nextList'], free)).toEqual({
        kind: 'zone',
        zoneId: 'bin',
      });
    });

    test('back from the zones: zone by zone, then the last list', () => {
      const bin: Destination = { kind: 'zone', zoneId: 'bin' };
      expect(walk(bin, ['previousList'], free)).toEqual({ kind: 'zone', zoneId: 'hidden' });
      expect(walk(bin, ['previousList', 'previousList'], free)).toEqual({
        kind: 'index',
        listId: 'c',
        index: 0,
      });
    });

    test('the list entered from a zone is entered at the index to resume, pulled into range', () => {
      const lists = order.slice(0, 2);
      const zone: Destination = { kind: 'zone', zoneId: 'hidden' };
      const scene = { ...free, lists };
      expect(keyboardDestination(zone, 'previousList', { ...scene, resumeIndex: 1 })).toEqual({
        kind: 'index',
        listId: 'b',
        index: 1,
      });
      expect(keyboardDestination(zone, 'previousList', { ...scene, resumeIndex: 9 })).toEqual({
        kind: 'index',
        listId: 'b',
        index: 2,
      });
      expect(keyboardDestination(zone, 'previousList', scene)).toEqual({
        kind: 'index',
        listId: 'b',
        index: 0,
      });
    });

    test('steps within a list mean nothing on a zone', () => {
      const zone: Destination = { kind: 'zone', zoneId: 'hidden' };
      for (const step of ['previous', 'next', 'first', 'last'] as const) {
        expect(keyboardDestination(zone, step, free)).toBe(zone);
      }
    });

    test('without zones the last list is the end', () => {
      const last: Destination = { kind: 'index', listId: 'c', index: 0 };
      expect(keyboardDestination(last, 'nextList', { lists: order, carried: ['a2'] })).toBe(last);
    });

    test('an unknown list or zone stays what it is', () => {
      const lost: Destination = { kind: 'index', listId: 'zz', index: 4 };
      expect(keyboardDestination(lost, 'nextList', free)).toBe(lost);
      const nowhere: Destination = { kind: 'zone', zoneId: 'zz' };
      expect(keyboardDestination(nowhere, 'previousList', free)).toBe(nowhere);
    });
  });

  describe('a view whose lists place items themselves', () => {
    const start: Destination = { kind: 'list', listId: 'a' };

    test('steps within a list go nowhere', () => {
      for (const step of ['previous', 'next', 'first', 'last'] as const) {
        expect(keyboardDestination(start, step, sorted)).toBe(start);
      }
    });

    test('steps between lists move, and carry no index', () => {
      expect(walk(start, ['nextList'], sorted)).toEqual({ kind: 'list', listId: 'b' });
      expect(walk(start, ['nextList', 'previousList'], sorted)).toEqual(start);
      expect(keyboardDestination(start, 'previousList', sorted)).toBe(start);
    });

    test('the zones follow the last list, and the way back is to a list without an index', () => {
      const last: Destination = { kind: 'list', listId: 'c' };
      expect(walk(last, ['nextList'], sorted)).toEqual({ kind: 'zone', zoneId: 'hidden' });
      expect(walk(last, ['nextList', 'previousList'], sorted)).toEqual(last);
    });
  });

  test('a row of columns: one list, where steps across lead nowhere', () => {
    const row: KeyboardScene = { lists: [{ id: 'row', items: ['x', 'y', 'z'] }], carried: ['x'] };
    const start: Destination = { kind: 'index', listId: 'row', index: 0 };
    expect(walk(start, ['next', 'next', 'next'], row)).toEqual({
      kind: 'index',
      listId: 'row',
      index: 2,
    });
    expect(keyboardDestination(start, 'nextList', row)).toBe(start);
    expect(keyboardDestination(start, 'previousList', row)).toBe(start);
  });
});

describe('windowChanged', () => {
  const size = { width: 626, height: 890 };

  test('the same size is no change', () => {
    expect(windowChanged(size, { ...size }, 120)).toBe(false);
  });

  test('any change of the width is one: layouts follow the width', () => {
    expect(windowChanged(size, { width: 627, height: 890 }, 120)).toBe(true);
    expect(windowChanged(size, { width: 890, height: 626 }, 120)).toBe(true);
  });

  test('the height may move by the tolerance, up or down, and no further', () => {
    expect(windowChanged(size, { width: 626, height: 890 - 120 }, 120)).toBe(false);
    expect(windowChanged(size, { width: 626, height: 890 + 120 }, 120)).toBe(false);
    expect(windowChanged(size, { width: 626, height: 890 - 121 }, 120)).toBe(true);
    expect(windowChanged(size, { width: 626, height: 890 + 121 }, 120)).toBe(true);
  });
});
