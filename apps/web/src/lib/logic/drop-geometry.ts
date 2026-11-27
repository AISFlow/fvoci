/** Geometry shared by pointer targeting and automatic scrolling; no DOM access. */
export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The direction in which a list stacks its items. */
export type Axis = 'x' | 'y';

export function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), Math.max(low, high));
}

/** The point of the rectangle that is nearest to `point`: the point itself when it is inside. */
export function clampToRect(point: Point, rect: Rect): Point {
  return {
    x: clamp(point.x, rect.left, rect.left + rect.width),
    y: clamp(point.y, rect.top, rect.top + rect.height),
  };
}

/** Whether the point is on the rectangle, its edges included. A rectangle without area holds nothing. */
export function containsPoint(rect: Rect, point: Point): boolean {
  if (rect.width <= 0 || rect.height <= 0) return false;
  return (
    point.x >= rect.left &&
    point.x <= rect.left + rect.width &&
    point.y >= rect.top &&
    point.y <= rect.top + rect.height
  );
}

/** The part two rectangles share. Without area when they do not overlap. */
export function intersectRect(first: Rect, second: Rect): Rect {
  const left = Math.max(first.left, second.left);
  const top = Math.max(first.top, second.top);
  const right = Math.min(first.left + first.width, second.left + second.width);
  const bottom = Math.min(first.top + first.height, second.top + second.height);
  return { left, top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) };
}

/** How far `position` is outside the stretch that starts at `start` and is `size` long. */
function distanceOutside(position: number, start: number, size: number): number {
  return Math.max(start - position, position - (start + size), 0);
}

/**
 * The list the point is over, or the one nearest to it when it is over none.
 *
 * Nearest means nearest across the axis first, and only among lists equally
 * near that way, nearest along it. With columns side by side a point below a
 * short column therefore still belongs to that column, however close a taller
 * neighbour is; with lists stacked on top of each other, or laid out as a
 * grid, the distance along the axis decides between those in line with the
 * point. Of two lists at the same distance the earlier one wins. Undefined
 * only when there is no list.
 */
export function nearestList<L extends { rect: Rect }>(
  point: Point,
  lists: readonly L[],
  axis: Axis = 'y',
): L | undefined {
  let best: L | undefined;
  let bestAcross = Infinity;
  let bestAlong = Infinity;
  for (const list of lists) {
    const horizontal = distanceOutside(point.x, list.rect.left, list.rect.width);
    const vertical = distanceOutside(point.y, list.rect.top, list.rect.height);
    const across = axis === 'y' ? horizontal : vertical;
    const along = axis === 'y' ? vertical : horizontal;
    if (across < bestAcross || (across === bestAcross && along < bestAlong)) {
      best = list;
      bestAcross = across;
      bestAlong = along;
    }
  }
  return best;
}

/**
 * The index at which an item dropped at `point` is inserted: the number of
 * items whose middle lies before the point along the axis.
 *
 * `items` are the rectangles of the list's items in order, without the
 * dragged ones. With a gap held open at the returned index the answer does
 * not flip back: opening the gap moves the items only further to the side of
 * the point they were already on.
 *
 * `reversed` is for items that run against the coordinate (a row on a
 * right-to-left page): there an item is before the point when its middle is
 * at a greater coordinate.
 */
export function insertionIndex(
  point: Point,
  items: readonly Rect[],
  axis: Axis = 'y',
  reversed = false,
): number {
  const position = axis === 'y' ? point.y : point.x;
  const index = items.findIndex((rect) => {
    const middle = axis === 'y' ? rect.top + rect.height / 2 : rect.left + rect.width / 2;
    return reversed ? position > middle : position < middle;
  });
  return index === -1 ? items.length : index;
}

/**
 * How hard a pointer at `position` pushes a scrolling box along one axis.
 *
 * The box starts at `start` and is `size` long. Within `edge` of either end
 * the result grows from 0 at the inner border of that zone to 1 at the end
 * and stays 1 beyond it; it is negative toward the start. In the middle it is
 * 0. A box too short for two zones gives each a third of itself, so that a
 * middle without scrolling always remains.
 */
export function edgeScrollFactor(
  position: number,
  start: number,
  size: number,
  edge: number,
): number {
  const zone = Math.min(edge, size / 3);
  if (zone <= 0) return 0;
  const fromStart = position - start;
  const fromEnd = start + size - position;
  if (fromStart < zone) return -clamp((zone - fromStart) / zone, 0, 1);
  if (fromEnd < zone) return clamp((zone - fromEnd) / zone, 0, 1);
  return 0;
}

/** The size of the window, as far as a drag cares. */
export interface WindowSize {
  width: number;
  height: number;
}

/**
 * Whether the window changed so much under a drag that the drag cannot go
 * on: every place it measured is somewhere else now. The width decides, since
 * layouts follow the width; the height may move by `tolerance` (a bar of the
 * browser sliding away) before it counts.
 */
export function windowChanged(before: WindowSize, after: WindowSize, tolerance: number): boolean {
  return before.width !== after.width || Math.abs(before.height - after.height) > tolerance;
}
