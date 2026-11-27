/**
 * The arithmetic of a Gantt view: the days its axis shows and the marks on
 * it, the rows, where the bar of a card lies, and the way an arrow takes
 * from the card that comes first to the one that comes after it.
 *
 * Lengths across are in rem and lengths down are in rows: the style sheet
 * decides how high a row is (a finger gets a higher one), and the drawing
 * stretches to it. A bar in row 3 lies between 3 and 4, its middle at 3.5.
 *
 * Days are calendar dates (dates.ts). Nothing here reads a clock or the time
 * zone of the device: today and the first day of the week are arguments, and
 * the days of a card are worked out by the caller in the reader's time zone.
 */
import { lastOf, monthOf, weekdayOf, weekOf } from './calendar.ts';
import { addDays, calendarDate, daysBetween, type CalendarDate } from './dates.ts';
import {
  edgesOnLoops,
  edgesOutOfOrderByDays,
  relationEdges,
  type Days,
  type RelationEdge,
} from './dependencies.ts';
import {
  ARROW_GAP,
  TURN_INSET,
  arrowHead,
  arrowHitPoints,
  arrowLabelPoint,
  arrowPath,
  arrowPoints,
  arrowsOf,
  edgeKey,
  type Arrow,
  type Point,
} from './gantt-arrows.ts';
import type { Weekday } from './regional.ts';
import type { GanttZoom } from './view-members.ts';
import type { Card, PropertyTemplate } from './wire.ts';

export {
  ARROW_GAP,
  TURN_INSET,
  arrowHead,
  arrowHitPoints,
  arrowLabelPoint,
  arrowPath,
  arrowPoints,
  arrowsOf,
  edgeKey,
  type Arrow,
  type Point,
};

// ---------------------------------------------------------------- zoom

/** The steps of the zoom, from the closest. */
export const GANTT_ZOOMS: readonly GanttZoom[] = ['day', 'week', 'month'];
export const DEFAULT_GANTT_ZOOM: GanttZoom = 'week';

/** The zoom a view stores; the default for none and for a word this version does not know. */
export function zoomOf(stored: unknown): GanttZoom {
  return GANTT_ZOOMS.find((known) => known === stored) ?? DEFAULT_GANTT_ZOOM;
}

/** How wide a day is at each step, in rem. */
export const DAY_WIDTH: Record<GanttZoom, number> = { day: 2.25, week: 0.75, month: 0.2 };

/** The empty days kept before the first card and after the last. */
const MARGIN_DAYS: Record<GanttZoom, number> = { day: 7, week: 14, month: 31 };

/**
 * The most days an axis shows before it is rounded out to whole weeks or
 * months: 156 weeks of days, 624 weeks, 50 years. Each is some 1,100, 620
 * and 600 marks, and the page draws every mark.
 */
export const MAX_AXIS_DAYS: Record<GanttZoom, number> = { day: 1092, week: 4368, month: 18263 };

// ---------------------------------------------------------------- axis

/** The days a chart shows, left to right. */
export interface Axis {
  zoom: GanttZoom;
  first: CalendarDate;
  last: CalendarDate;
  /** How many days, both ends included. */
  days: number;
  /** In rem. */
  dayWidth: number;
  width: number;
}

const FIRST_DAY = calendarDate(1, 1, 1);
const LAST_DAY = calendarDate(9999, 12, 31);

/** `date` moved by `days`, kept among the days that can be written (the years 1 to 9999). */
function shift(date: CalendarDate, days: number): CalendarDate {
  const least = -daysBetween(FIRST_DAY, date);
  const most = daysBetween(date, LAST_DAY);
  return addDays(date, Math.min(most, Math.max(least, days)));
}

type Unit = 'day' | 'week' | 'month' | 'year';

/** How many days from `date` to the first day of the next week, month or year. */
function daysLeft(unit: Unit, date: CalendarDate, weekStart: Weekday): number {
  if (unit === 'day') return 1;
  if (unit === 'week') return 7 - ((weekdayOf(date) - weekStart + 7) % 7);
  const end =
    unit === 'month' ? lastOf(monthOf(date)) : calendarDate(Number(date.slice(0, 4)), 12, 31);
  return daysBetween(date, end) + 1;
}

/** The first day of the week or month `date` is in: an axis begins and ends on whole ones. */
function unitStart(date: CalendarDate, zoom: GanttZoom, weekStart: Weekday): CalendarDate {
  if (zoom === 'month') return calendarDate(Number(date.slice(0, 4)), Number(date.slice(5, 7)), 1);
  const first = weekOf(date, weekStart)[0] ?? date;
  return first < FIRST_DAY ? FIRST_DAY : first;
}

function unitEnd(date: CalendarDate, zoom: GanttZoom, weekStart: Weekday): CalendarDate {
  return shift(date, daysLeft(zoom === 'month' ? 'month' : 'week', date, weekStart) - 1);
}

export interface AxisSource {
  zoom: GanttZoom;
  weekStart: Weekday;
  /** Today on the reader's calendar: always on the axis. */
  today: CalendarDate;
  /** The days of the cards. */
  spans: Iterable<Days>;
  /** The axis is at least this long: what the page has room for. */
  minDays?: number;
}

/**
 * The axis: from the first day of any card to the last, today among them,
 * with empty days at both ends, begun and ended on whole weeks (whole months
 * at the zoom `month`).
 *
 * Cards too far apart to draw (`MAX_AXIS_DAYS`) get a window of that many
 * days instead: today a third of the way in, moved as far as it takes to
 * leave no part of the window empty. The cards outside it keep a stub at the
 * edge (`barBox`).
 */
export function axisOf(source: AxisSource): Axis {
  const { zoom, weekStart, today } = source;
  let from = today;
  let to = today;
  for (const span of source.spans) {
    if (span.first < from) from = span.first;
    if (span.last > to) to = span.last;
  }
  const margin = MARGIN_DAYS[zoom];
  const cap = MAX_AXIS_DAYS[zoom];
  let back = daysBetween(from, today) + margin;
  let ahead = daysBetween(today, to) + margin;
  if (back + ahead + 1 > cap) {
    const third = Math.floor(cap / 3);
    const short = Math.min(back, third);
    ahead = Math.min(ahead, cap - 1 - short);
    back = Math.min(back, cap - 1 - ahead);
  }
  let first = unitStart(shift(today, -back), zoom, weekStart);
  let last = unitEnd(shift(today, ahead), zoom, weekStart);
  const wanted = Math.min(source.minDays ?? 0, cap);
  if (daysBetween(first, last) + 1 < wanted) {
    last = unitEnd(shift(first, wanted - 1), zoom, weekStart);
  }
  // The last year that can be written leaves no room after: the room is taken before.
  if (daysBetween(first, last) + 1 < wanted) {
    first = unitStart(shift(last, 1 - wanted), zoom, weekStart);
  }
  const days = daysBetween(first, last) + 1;
  const dayWidth = DAY_WIDTH[zoom];
  return { zoom, first, last, days, dayWidth, width: days * dayWidth };
}

/** Where a day begins on the axis, in rem; outside the axis for a day that is not on it. */
export function xOfDay(axis: Axis, date: CalendarDate): number {
  return daysBetween(axis.first, date) * axis.dayWidth;
}

// --------------------------------------------------------------- marks

/** One mark of the axis: the day it begins on, where, and how wide it is. */
export interface Tick {
  date: CalendarDate;
  x: number;
  width: number;
}

export interface Scale {
  /** The upper row: months, or years at the zoom `month`. */
  upper: Tick[];
  /** The lower row: days, weeks or months. */
  lower: Tick[];
  /** Where a line goes down the chart: at each week of days, else at each lower mark. */
  lines: number[];
}

/** The axis cut into days, weeks, months or years; the first and the last may be cut short. */
function cut(axis: Axis, unit: Unit, weekStart: Weekday): Tick[] {
  const ticks: Tick[] = [];
  for (let at = 0; at < axis.days;) {
    const date = addDays(axis.first, at);
    const length = Math.min(daysLeft(unit, date, weekStart), axis.days - at);
    ticks.push({ date, x: at * axis.dayWidth, width: length * axis.dayWidth });
    at += length;
  }
  return ticks;
}

export function scaleOf(axis: Axis, weekStart: Weekday): Scale {
  const lower = cut(axis, axis.zoom, weekStart);
  const upper = cut(axis, axis.zoom === 'month' ? 'year' : 'month', weekStart);
  const lined = axis.zoom === 'day' ? cut(axis, 'week', weekStart) : lower;
  return { upper, lower, lines: lined.map((tick) => tick.x).filter((x) => x > 0) };
}

// ---------------------------------------------------------------- rows

/** The cards between two headings: all of them, or those of one group. */
export interface RowList {
  /** The group the cards are in; none where the view is not grouped. */
  groupId?: string;
  cardIds: readonly string[];
}

export type ChartRow =
  | { kind: 'group'; id: string; row: number; count: number }
  | { kind: 'card'; id: string; row: number };

/** The rows from the top: the heading of each group, then its cards in the order given. */
export function rowsOf(lists: readonly RowList[]): ChartRow[] {
  const rows: ChartRow[] = [];
  for (const list of lists) {
    if (list.groupId !== undefined) {
      rows.push({ kind: 'group', id: list.groupId, row: rows.length, count: list.cardIds.length });
    }
    for (const id of list.cardIds) rows.push({ kind: 'card', id, row: rows.length });
  }
  return rows;
}

// ---------------------------------------------------------------- bars

/** No bar is narrower, in rem: a day is a fifth of a rem at the zoom `month`. */
export const MIN_BAR_WIDTH = 0.5;

/** Where the bar of a card lies across the axis. */
export interface BarBox {
  x: number;
  width: number;
  /** The card began before the axis, or goes on after it: that end is cut. */
  fromBefore: boolean;
  toAfter: boolean;
}

/**
 * The bar of a card: from the start of its first day to the end of its last
 * (a card of one day is one day wide). A card that is partly off the axis is
 * cut at the edge; one wholly off it keeps a stub at the edge it is beyond,
 * so that it still has something to press and for arrows to reach.
 */
export function barBox(axis: Axis, span: Days): BarBox {
  const start = daysBetween(axis.first, span.first);
  const end = daysBetween(axis.first, span.last) + 1;
  const from = Math.min(axis.days, Math.max(0, start));
  const to = Math.min(axis.days, Math.max(0, end));
  const width = Math.max((to - from) * axis.dayWidth, MIN_BAR_WIDTH);
  return {
    x: Math.max(0, Math.min(from * axis.dayWidth, axis.width - width)),
    width,
    fromBefore: start < 0,
    toAfter: end > axis.days,
  };
}

/** A bar in its row. */
export interface Bar extends BarBox {
  id: string;
  row: number;
}

/** The bars of the card rows; a card without days has none. */
export function barsOf(
  axis: Axis,
  rows: readonly ChartRow[],
  daysOf: (cardId: string) => Days | undefined,
): Map<string, Bar> {
  const bars = new Map<string, Bar>();
  for (const row of rows) {
    if (row.kind !== 'card') continue;
    const days = daysOf(row.id);
    if (days) bars.set(row.id, { id: row.id, row: row.row, ...barBox(axis, days) });
  }
  return bars;
}

// ----------------------------------------------------------- relations

/**
 * The property a Gantt view reads as "the cards that come before this one":
 * the one it names, for as long as the board has it and it is a card
 * relation. A view that names none, or one that was deleted or made another
 * kind since, has none: no arrows are drawn.
 */
export function dependencyProperty(
  dependencyPropertyId: string | undefined,
  templates: readonly PropertyTemplate[],
): PropertyTemplate | undefined {
  if (!dependencyPropertyId) return undefined;
  const named = templates.find((template) => template.id === dependencyPropertyId);
  return named?.type === 'relation' ? named : undefined;
}

/**
 * The card relations of a board that some Gantt view reads as what comes
 * before what. Where a card names others in one of them, a card that would
 * close a loop is not offered (the editor's `preventLoops`): a loop made on
 * the screen is kept out; one made elsewhere is only shown.
 */
export function orderingRelations(
  /** The fields of the board's views. */
  views: readonly { viewType: string; dependencyPropertyId?: string }[],
  templates: readonly PropertyTemplate[],
): Set<string> {
  const ids = new Set<string>();
  for (const view of views) {
    if (view.viewType !== 'gantt') continue;
    const relation = dependencyProperty(view.dependencyPropertyId, templates);
    if (relation) ids.add(relation.id);
  }
  return ids;
}

/** What one relation property says about the cards of a chart. */
export interface ChartRelations {
  /** The edges between two cards that both have a bar: one arrow each. */
  edges: RelationEdge[];
  /** Of those, by `edgeKey`: the ones whose dates are out of order, and the ones on a loop. */
  late: Set<string>;
  loops: Set<string>;
  /** Every card of the board that is on a loop, drawn or not. */
  loopCards: Set<string>;
}

/**
 * The edges a chart draws: those between two cards of `shown` (the cards of
 * the view's rows) that both have days, and so a bar. `all` are the cards of
 * the board, so that a loop is found even where one of its cards is filtered
 * out of the view or has no date. A card of the loop that is not drawn has
 * no arrow, but the arrows between the others are marked.
 */
export function relationsOf(
  shown: readonly Card[],
  all: Iterable<Card>,
  propertyId: string | undefined,
  daysOf: (cardId: string) => Days | undefined,
): ChartRelations {
  if (!propertyId) return { edges: [], late: new Set(), loops: new Set(), loopCards: new Set() };
  const drawn = shown.filter((card) => daysOf(card.id) !== undefined);
  const edges = relationEdges(drawn, propertyId);
  const looping = edgesOnLoops(relationEdges(all, propertyId));
  const onLoops = new Set(looping.map(edgeKey));
  return {
    edges,
    late: new Set(edgesOutOfOrderByDays(edges, daysOf).map(edgeKey)),
    loops: new Set(edges.map(edgeKey).filter((key) => onLoops.has(key))),
    loopCards: new Set(looping.flatMap(({ from, to }) => [from, to])),
  };
}

/** Of `cardIds`, those on a loop. */
export function onLoops(cardIds: Iterable<string>, loopCards: ReadonlySet<string>): Set<string> {
  return new Set([...cardIds].filter((id) => loopCards.has(id)));
}

/** The name of the group each card is in, from groups as the view has them. */
export function groupOfCards<G extends { cards: readonly { id: string }[] }>(
  groups: readonly G[],
  nameOf: (group: G) => string,
): Map<string, string> {
  const names = new Map<string, string>();
  for (const group of groups) {
    const name = nameOf(group);
    for (const card of group.cards) names.set(card.id, name);
  }
  return names;
}

/** For each card that comes after others, those others, in the order of the edges. */
export function cardsBefore(edges: readonly RelationEdge[]): Map<string, string[]> {
  const before = new Map<string, string[]>();
  for (const { from, to } of edges) {
    const list = before.get(to);
    if (list) list.push(from);
    else before.set(to, [from]);
  }
  return before;
}
