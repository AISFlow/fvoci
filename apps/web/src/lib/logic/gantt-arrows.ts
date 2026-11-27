/**
 * Arrow geometry for a Gantt chart: four dependency kinds (`fs`, `ss`, `ff`,
 * `sf`), straight horizontal/vertical routing kept within the axis, hit-path
 * trimming clear of bar grips, label anchor points, and SVG path encoding.
 */
import type { RelationEdge } from './dependencies.ts';
import type { RelationKind } from './relations.ts';

/** Minimal bar geometry needed to route arrows between rows. */
export interface ArrowBar {
  id: string;
  row: number;
  x: number;
  width: number;
}

/** Minimal chart relation set needed to build drawn arrows. */
export interface ArrowRelations {
  edges: readonly RelationEdge[];
  late: ReadonlySet<string>;
  loops: ReadonlySet<string>;
}

export type Point = readonly [x: number, y: number];

/** An arrow between the bars of `from` and `to`. */
export interface Arrow extends RelationEdge {
  /** The corners of its way: x in rem, y in rows. The last is the tip. */
  points: Point[];
  late: boolean;
  loop: boolean;
}

/** How far an arrow goes straight out of a bar and into one, in rem. */
export const ARROW_GAP = 0.5;
/** The head: how long in rem, and half its height in rows. */
const HEAD_LENGTH = 0.4;
const HEAD_HALF = 0.14;

/**
 * How far from the edge of its own row an arrow runs where it turns back, in
 * rows: under (or over) the bar it leaves, clear of the line between the
 * rows. An arrow down out of one row and an arrow up out of the next would
 * otherwise lie on that same line and read as one.
 */
export const TURN_INSET = 0.1;

/** Names an edge in a set. */
export function edgeKey(edge: Pick<RelationEdge, 'from' | 'to'>): string {
  return `${edge.from}\u0000${edge.to}`;
}

/**
 * The way of an arrow, in straight pieces across and down, for any of the four
 * dependency kinds (`fs`, `ss`, `ff`, `sf`):
 *
 * - Departure: `fs`/`ff` leave the end (`from.x + from.width`), `ss`/`sf` leave
 *   the start (`from.x`).
 * - Arrival: `fs`/`ss` enter the start (`to.x`, pointing right), `ff`/`sf`
 *   enter the end (`to.x + to.width`, pointing left).
 * - No corner leaves `0..=width`, and detours stay `TURN_INSET` inside `from`'s
 *   row so adjacent rows never merge on the border between them.
 */
export function arrowPoints(
  from: ArrowBar,
  to: ArrowBar,
  width = Infinity,
  kind: RelationKind = 'fs',
): Point[] {
  const fromEnd = kind === 'fs' || kind === 'ff';
  const toStart = kind === 'fs' || kind === 'ss';
  const startX = fromEnd ? from.x + from.width : from.x;
  const endX = toStart ? to.x : to.x + to.width;
  const startY = from.row + 0.5;
  const endY = to.row + 0.5;
  const clamp = (x: number): number => Math.min(width, Math.max(0, x));

  // Same-side relations (`ss` on the left, `ff` on the right) route cleanly in a
  // 4-point C-shape outside both bars.
  if (kind === 'ss') {
    const channel = clamp(Math.min(startX, endX) - ARROW_GAP);
    return [
      [startX, startY],
      [channel, startY],
      [channel, endY],
      [endX, endY],
    ];
  }
  if (kind === 'ff') {
    const channel = clamp(Math.max(startX, endX) + ARROW_GAP);
    return [
      [startX, startY],
      [channel, startY],
      [channel, endY],
      [endX, endY],
    ];
  }

  // Opposite-side relations (`fs` right->left, `sf` left->right): direct 4-point
  // route when there are at least two gaps between anchor and target; otherwise
  // a 6-point detour along `from`'s row edge.
  const out = clamp(fromEnd ? startX + ARROW_GAP : startX - ARROW_GAP);
  const direct = fromEnd
    ? endX - ARROW_GAP >= startX + ARROW_GAP
    : startX - ARROW_GAP >= endX + ARROW_GAP;
  if (direct) {
    return [
      [startX, startY],
      [out, startY],
      [out, endY],
      [endX, endY],
    ];
  }
  const before = clamp(toStart ? endX - ARROW_GAP : endX + ARROW_GAP);
  const along = to.row > from.row ? from.row + 1 - TURN_INSET : from.row + TURN_INSET;
  return [
    [startX, startY],
    [out, startY],
    [out, along],
    [before, along],
    [before, endY],
    [endX, endY],
  ];
}

/** One arrow for each edge whose two cards have a bar. */
export function arrowsOf(
  relations: ArrowRelations,
  bars: ReadonlyMap<string, ArrowBar>,
  /** The width of the axis: no arrow leaves it. */
  width = Infinity,
): Arrow[] {
  return relations.edges.flatMap((edge) => {
    const from = bars.get(edge.from);
    const to = bars.get(edge.to);
    if (!from || !to) return [];
    const key = edgeKey(edge);
    return {
      ...edge,
      points: arrowPoints(from, to, width, edge.kind),
      late: relations.late.has(key),
      loop: relations.loops.has(key),
    };
  });
}

/** Vertical clearance in rows around a bar's midpoint so hit paths stay clear of resize grips. */
const HIT_ROW_TRIM = 0.35;

/**
 * Trims `trim` rem from the start and end horizontal stubs (and vertical entry
 * stubs inside the endpoint rows) so an interactive hit path never steals
 * pointer events from bar grips at either bar edge.
 */
export function arrowHitPoints(points: readonly Point[], trim = ARROW_GAP): Point[] {
  if (points.length < 2) return [...points];
  const next: Point[] = [...points];
  const [first, second] = next;
  if (first !== undefined && second !== undefined) {
    if (first[1] === second[1]) {
      const dx = second[0] - first[0];
      if (Math.abs(dx) > trim) next[0] = [first[0] + Math.sign(dx) * trim, first[1]];
      else if (next.length > 2) next.shift();
    }
  }
  const last = next.at(-1);
  const prev = next.at(-2);
  if (last !== undefined && prev !== undefined) {
    if (last[1] === prev[1]) {
      const dx = last[0] - prev[0];
      if (Math.abs(dx) > trim) next[next.length - 1] = [last[0] - Math.sign(dx) * trim, last[1]];
      else if (next.length > 2) next.pop();
    }
  }
  // Also trim vertical stubs adjacent to endpoint bar rows when the segment stays non-degenerate.
  const s0 = next[0];
  const s1 = next[1];
  if (s0 !== undefined && s1 !== undefined) {
    if (s0[0] === s1[0]) {
      const dy = s1[1] - s0[1];
      if (Math.abs(dy) > HIT_ROW_TRIM * 2) next[0] = [s0[0], s0[1] + Math.sign(dy) * HIT_ROW_TRIM];
      else if (next.length > 2) next.shift();
    }
  }
  const e1 = next.at(-1);
  const e0 = next.at(-2);
  if (e0 !== undefined && e1 !== undefined) {
    if (e0[0] === e1[0]) {
      const dy = e1[1] - e0[1];
      if (Math.abs(dy) > HIT_ROW_TRIM * 2) {
        next[next.length - 1] = [e1[0], e1[1] - Math.sign(dy) * HIT_ROW_TRIM];
      } else if (next.length > 2) {
        next.pop();
      }
    }
  }
  return next;
}

/**
 * The anchor point for an arrow's badge/button: the midpoint of the horizontal
 * detour for a 6-point arrow, or the midpoint of the vertical drop for a
 * 4-point arrow — always clear of both bars.
 */
export function arrowLabelPoint(points: readonly Point[]): Point {
  const p1 = points[1];
  const p2 = points[2];
  const p3 = points[3];
  const p4 = points[4];
  if (points.length >= 6 && p2 && p3 && p4) {
    if (p2[0] !== p3[0]) return [(p2[0] + p3[0]) / 2, p2[1]];
    return [p3[0], (p3[1] + p4[1]) / 2];
  }
  if (points.length >= 4 && p1 && p2) {
    return [p1[0], (p1[1] + p2[1]) / 2];
  }
  const first = points[0];
  const last = points.at(-1);
  if (!first || !last) return [0, 0];
  return [(first[0] + last[0]) / 2, (first[1] + last[1]) / 2];
}

const number = (value: number): string => String(Math.round(value * 1000) / 1000);

/** The line of an arrow as the `d` of an SVG path. */
export function arrowPath(points: readonly Point[]): string {
  return points.map(([x, y], at) => `${at === 0 ? 'M' : 'L'}${number(x)} ${number(y)}`).join('');
}

/**
 * The head of an arrow, a triangle whose tip is the last point, as the `d` of
 * an SVG path. `fs` and `ss` point right into the start of the target bar;
 * `ff` and `sf` point left into its end.
 */
export function arrowHead(points: readonly Point[], kind: RelationKind = 'fs'): string {
  const tip = points.at(-1);
  if (!tip) return '';
  const [x, y] = tip;
  const pointsLeft = kind === 'ff' || kind === 'sf';
  const back = number(pointsLeft ? x + HEAD_LENGTH : x - HEAD_LENGTH);
  return `M${number(x)} ${number(y)}L${back} ${number(y - HEAD_HALF)}L${back} ${number(y + HEAD_HALF)}Z`;
}
