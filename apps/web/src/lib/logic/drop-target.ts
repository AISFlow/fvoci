/**
 * Where a dragged item would land.
 *
 * Lists of items lie side by side (the columns of a board) and each list
 * stacks its items along one axis. Everything here works on plain numbers:
 * rectangles measured by the caller, in any one coordinate system, and the
 * order of the items as ids. Nothing reads the document.
 *
 * A place is always counted among the items of a list *without* the dragged
 * ones, so that "index 2" means the same thing whether an item comes from
 * that list or from another, and whether one item is dragged or several.
 *
 * Besides a place in a list a drop can mean two things that have no index: a
 * list whose own order decides where the item goes (a sorted view), and a
 * zone, which is a target that is no list at all (a hidden column).
 */

import {
  clamp,
  clampToRect,
  containsPoint,
  insertionIndex,
  nearestList,
  type Axis,
  type Point,
  type Rect,
} from './drop-geometry.ts';

export {
  clampToRect,
  containsPoint,
  edgeScrollFactor,
  insertionIndex,
  intersectRect,
  nearestList,
  windowChanged,
} from './drop-geometry.ts';
export type { Axis, Point, Rect, WindowSize } from './drop-geometry.ts';

/** A list on screen: the part of it that is visible, and its items in order, without the dragged ones. */
export interface ListBox {
  id: string;
  rect: Rect;
  items: readonly Rect[];
}

/** A zone on screen: a target that is no list. */
export interface ZoneBox {
  id: string;
  rect: Rect;
}

/** A place in a list. `index` counts the items of the list without the dragged ones. */
export interface DropTarget {
  listId: string;
  index: number;
}

/**
 * Who decides where in a list a dropped item goes: the person dragging
 * (`free`), or the list itself, because it is sorted (`list`).
 */
export type Placement = 'free' | 'list';

/**
 * What a drop means.
 *
 * - `index`: at that place of that list.
 * - `list`: into that list, wherever the list puts it. There is no index.
 * - `zone`: onto a target that is no list. What that does is up to the caller.
 */
export type Destination =
  | { kind: 'index'; listId: string; index: number }
  | { kind: 'list'; listId: string }
  | { kind: 'zone'; zoneId: string };

/** The order of the items as data: what a drop changes. */
export interface ListOrder {
  id: string;
  items: readonly string[];
}

export interface DropTargetOptions {
  /** The direction in which each list stacks its items. Default `y`. */
  axis?: Axis;
  /**
   * The visible part of whatever holds the lists. A pointer outside it counts
   * as being on its edge, so that a list scrolled out of view cannot be hit.
   */
  bounds?: Rect;
  /**
   * The items run against the coordinate: right to left along `x`, as a row
   * does on a right-to-left page.
   */
  reversed?: boolean;
}

/**
 * The place a dragged item would land at when released at `point`.
 *
 * The list is the one under the point or nearest to it; the index comes from
 * the point pulled onto the visible part of that list, so a pointer above a
 * scrolled list lands at its first visible item and not at its hidden start.
 */
export function dropTarget(
  point: Point,
  lists: readonly ListBox[],
  options: DropTargetOptions = {},
): DropTarget | undefined {
  const inside = options.bounds ? clampToRect(point, options.bounds) : point;
  const list = nearestList(inside, lists, options.axis);
  if (!list) return undefined;
  const onList = clampToRect(inside, list.rect);
  return {
    listId: list.id,
    index: insertionIndex(onList, list.items, options.axis, options.reversed),
  };
}

export function sameTarget(first: DropTarget, second: DropTarget): boolean {
  return first.listId === second.listId && first.index === second.index;
}

/** Whether two drops mean the same. Undefined, "no drop at all", equals only itself. */
export function sameDestination(
  first: Destination | undefined,
  second: Destination | undefined,
): boolean {
  if (!first || !second) return first === second;
  switch (first.kind) {
    case 'index':
      return second.kind === 'index' && sameTarget(first, second);
    case 'list':
      return second.kind === 'list' && first.listId === second.listId;
    case 'zone':
      return second.kind === 'zone' && first.zoneId === second.zoneId;
  }
}

/** What the pointer is on: nothing that takes a drop, a zone, or a list. */
export type DropRegion<L> =
  { kind: 'outside' } | { kind: 'zone'; zoneId: string } | { kind: 'list'; list: L };

export interface DropScene<L> {
  /**
   * The visible part of whatever holds the lists and zones. A pointer that is
   * not on it is outside: letting go there drops nowhere.
   */
  bounds?: Rect;
  zones?: readonly ZoneBox[];
  lists: readonly L[];
  /** The direction in which each list stacks its items. Default `y`. */
  axis?: Axis;
}

/**
 * Which part of the scene a pointer at `point` is on.
 *
 * Outside the bounds nothing is hit, however near a list is: there a release
 * cancels. Inside, a zone is hit only by a pointer that is on it, and the
 * first such zone wins; anywhere else the nearest list is hit, as in
 * `nearestList`. Undefined when the pointer is inside and there is neither.
 */
export function dropRegion<L extends { rect: Rect }>(
  point: Point,
  scene: DropScene<L>,
): DropRegion<L> | undefined {
  if (scene.bounds && !containsPoint(scene.bounds, point)) return { kind: 'outside' };
  const zone = scene.zones?.find((candidate) => containsPoint(candidate.rect, point));
  if (zone) return { kind: 'zone', zoneId: zone.id };
  const list = nearestList(point, scene.lists, scene.axis);
  return list ? { kind: 'list', list } : undefined;
}

/**
 * Where an item is now, as a place: its index among the other items of its
 * list. With `without`, the items carried along with it, the index counts
 * only the items that stay.
 */
export function locateItem(
  lists: readonly ListOrder[],
  itemId: string,
  without: readonly string[] = [],
): DropTarget | undefined {
  for (const list of lists) {
    const at = list.items.indexOf(itemId);
    if (at === -1) continue;
    const staying = list.items.slice(0, at).filter((id) => !without.includes(id));
    return { listId: list.id, index: staying.length };
  }
  return undefined;
}

/**
 * The items one drag carries: the grabbed item alone, or, when it is one of
 * the selected items, all of those. Selected items that are in no list are
 * left out, and the result is in the order of the data (list by list, start
 * to end) whatever order they were selected in.
 */
export function carriedItems(
  lists: readonly ListOrder[],
  grabbedId: string,
  selected: readonly string[] = [],
): string[] {
  if (!selected.includes(grabbedId)) return [grabbedId];
  const chosen = new Set(selected);
  return lists.flatMap((list) => list.items.filter((id) => chosen.has(id)));
}

/**
 * The order after the items were dropped at `target` together.
 *
 * They are taken out of whatever lists they are in and put in as one run, in
 * the order they had in the data (list by list, start to end), not the order
 * of `itemIds`. `target.index` counts the items of the target list without
 * any of the moved ones; past the end means the end. Unknown items are left
 * out; with none left, or an unknown list, nothing changes. The lists passed
 * in are not changed, and lists the move does not touch are returned as they
 * are.
 */
export function moveItems(
  lists: readonly ListOrder[],
  itemIds: readonly string[],
  target: DropTarget,
): ListOrder[] {
  const chosen = new Set(itemIds);
  const moving = lists.flatMap((list) => list.items.filter((id) => chosen.has(id)));
  if (moving.length === 0 || !lists.some((list) => list.id === target.listId)) return [...lists];
  return lists.map((list) => {
    const isTarget = list.id === target.listId;
    if (!isTarget && !list.items.some((id) => chosen.has(id))) return list;
    const items = list.items.filter((id) => !chosen.has(id));
    if (isTarget) items.splice(clamp(target.index, 0, items.length), 0, ...moving);
    return { ...list, items };
  });
}

/** The order after `itemId` was dropped at `target`: `moveItems` with one item. */
export function moveItem(
  lists: readonly ListOrder[],
  itemId: string,
  target: DropTarget,
): ListOrder[] {
  return moveItems(lists, [itemId], target);
}

/**
 * Whether dropping the items there changes anything.
 *
 * At a place: when the order would differ. Into a list that places items
 * itself: when one of them comes from another list. Onto a zone: always.
 * Never when none of the items is in any list.
 */
export function dropChanges(
  lists: readonly ListOrder[],
  itemIds: readonly string[],
  destination: Destination,
): boolean {
  const chosen = new Set(itemIds);
  const homes = lists.filter((list) => list.items.some((id) => chosen.has(id)));
  if (homes.length === 0) return false;
  switch (destination.kind) {
    case 'zone':
      return true;
    case 'list':
      return (
        lists.some((list) => list.id === destination.listId) &&
        homes.some((list) => list.id !== destination.listId)
      );
    case 'index': {
      const after = moveItems(lists, itemIds, destination);
      return after.some((list, at) => {
        const before = lists[at]?.items ?? [];
        return (
          list.items.length !== before.length ||
          list.items.some((id, index) => id !== before[index])
        );
      });
    }
  }
}

export interface GapScene {
  /** Where the grabbed item was picked up, counted without the carried items. */
  from: DropTarget;
  /** The order as data, still holding the carried items. */
  lists: readonly ListOrder[];
  carried: readonly string[];
  /**
   * For a list that places items itself: the index, counted without the
   * carried items, at which its order would put the grabbed item. Without it,
   * or when it has no answer for a list, the end.
   */
  sortedIndex?: (listId: string) => number | undefined;
}

/**
 * Where the page shows the gap for a drop that would go to `destination`.
 *
 * - A place in a list: there.
 * - A list that places items itself: where its order would put the item, and
 *   in the list the item came from, where it was. The pointer has no say.
 * - A zone: nowhere, the zone shows it instead.
 * - No destination (the pointer is outside): back where it was.
 */
export function gapFor(
  destination: Destination | undefined,
  scene: GapScene,
): DropTarget | undefined {
  if (!destination) return scene.from;
  switch (destination.kind) {
    case 'zone':
      return undefined;
    case 'index':
      return { listId: destination.listId, index: destination.index };
    case 'list': {
      if (destination.listId === scene.from.listId) return scene.from;
      const list = scene.lists.find((candidate) => candidate.id === destination.listId);
      const others = list ? list.items.filter((id) => !scene.carried.includes(id)).length : 0;
      const sorted = scene.sortedIndex?.(destination.listId) ?? others;
      return { listId: destination.listId, index: clamp(sorted, 0, others) };
    }
  }
}

/** A step taken with the keyboard while carrying an item. */
export type KeyboardStep = 'previous' | 'next' | 'first' | 'last' | 'previousList' | 'nextList';

/**
 * The place a carried item moves to with one step of the keyboard.
 *
 * `lists` is the order as data, still holding the carried item where it was
 * picked up. Within a list a step stops at the ends. A step to the list on
 * either side keeps the index when that list is long enough and otherwise
 * goes to its end. A step that leads nowhere returns the place unchanged.
 * `carried` is the carried item, or all of them when several are carried.
 */
export function keyboardTarget(
  current: DropTarget,
  step: KeyboardStep,
  lists: readonly ListOrder[],
  carried: string | readonly string[],
): DropTarget {
  const gone = typeof carried === 'string' ? [carried] : carried;
  const at = lists.findIndex((list) => list.id === current.listId);
  const others = (list: ListOrder | undefined): number =>
    list ? list.items.filter((id) => !gone.includes(id)).length : 0;
  const here = others(lists[at]);

  switch (step) {
    case 'previous':
      return { listId: current.listId, index: clamp(current.index - 1, 0, here) };
    case 'next':
      return { listId: current.listId, index: clamp(current.index + 1, 0, here) };
    case 'first':
      return { listId: current.listId, index: 0 };
    case 'last':
      return { listId: current.listId, index: here };
    case 'previousList':
    case 'nextList': {
      const neighbour = lists[step === 'nextList' ? at + 1 : at - 1];
      if (at === -1 || !neighbour) return current;
      return { listId: neighbour.id, index: clamp(current.index, 0, others(neighbour)) };
    }
  }
}

export interface KeyboardScene {
  /** The order as data, still holding the carried items where they were picked up. */
  lists: readonly ListOrder[];
  /** The zones the keyboard can reach, in order. They come after the last list. */
  zones?: readonly string[];
  carried: readonly string[];
  /** Default `free`. */
  placement?: Placement;
  /** In free placement, the index to take up in a list entered from a zone. Default 0. */
  resumeIndex?: number;
}

/**
 * What a carried item would be dropped on after one step of the keyboard.
 *
 * The lists stand in a row and the zones follow the last of them: a step to
 * the next list from the last list reaches the first zone, further steps go
 * on through the zones, and steps to the previous list lead back the same
 * way. Steps within a list mean nothing on a zone.
 *
 * In free placement the steps within and between lists are those of
 * `keyboardTarget`. In a list that places items itself there is no place to
 * step to within a list: only steps between lists and zones move. A step that
 * leads nowhere returns the destination it was given.
 */
export function keyboardDestination(
  current: Destination,
  step: KeyboardStep,
  scene: KeyboardScene,
): Destination {
  const { lists, carried } = scene;
  const zones = scene.zones ?? [];
  const across = step === 'previousList' || step === 'nextList';

  if (current.kind === 'zone') {
    const at = zones.indexOf(current.zoneId);
    if (!across || at === -1) return current;
    if (step === 'nextList') {
      const next = zones[at + 1];
      return next === undefined ? current : { kind: 'zone', zoneId: next };
    }
    const previous = zones[at - 1];
    if (previous !== undefined) return { kind: 'zone', zoneId: previous };
    const last = lists.at(-1);
    if (!last) return current;
    if (scene.placement === 'list') return { kind: 'list', listId: last.id };
    const others = last.items.filter((id) => !carried.includes(id)).length;
    return { kind: 'index', listId: last.id, index: clamp(scene.resumeIndex ?? 0, 0, others) };
  }

  const at = lists.findIndex((list) => list.id === current.listId);
  if (at === -1) return current;
  if (step === 'nextList' && at === lists.length - 1) {
    const first = zones[0];
    return first === undefined ? current : { kind: 'zone', zoneId: first };
  }

  if (current.kind === 'list') {
    if (!across) return current;
    const neighbour = lists[step === 'nextList' ? at + 1 : at - 1];
    return neighbour ? { kind: 'list', listId: neighbour.id } : current;
  }

  const to = keyboardTarget(current, step, lists, carried);
  return sameTarget(to, current) ? current : { kind: 'index', ...to };
}
