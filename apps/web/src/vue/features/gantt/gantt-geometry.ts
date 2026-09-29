// Gantt geometry, computed in the browser from the server's task layout
// (items with their derived start/end, links, calendar and the month range).
// Presentation only: permissions, derived dates and dependency rules stay on
// the server. Ported from the server's src/gantt (layout.rs, scale.rs,
// calendar.rs, links.rs) and the former React Gantt's TypeScript copy.
import { t } from "@fvoci/i18n";
import { addDays, dayOfWeek, daysBetween, fromEpochDay, toEpochDay, type IsoDate } from "@/lib/iso-date";

export type LinkType = "FS" | "SS" | "FF";
export type ScheduleInference = "none" | "from-due" | "from-start" | "swapped";
export type PackMode = "rows" | "overlap";

/** The visible date range (both ends included) at a fixed zoom. */
export interface GanttScale {
  readonly start: IsoDate;
  readonly end: IsoDate;
  readonly pxPerDay: number;
}

export interface ScheduledItem {
  readonly id: string;
  readonly start: IsoDate;
  readonly end: IsoDate;
  readonly milestone: boolean;
  readonly inferred: ScheduleInference;
}

export interface GanttLinkInput {
  readonly blockerId: string;
  readonly blockedId: string;
  readonly type: LinkType;
  readonly lagDays: number;
}

export interface WorkCalendar {
  /** 0 = Sunday .. 6 = Saturday. */
  readonly weekend: readonly number[];
  readonly holidays: ReadonlySet<IsoDate>;
}

export interface BarBox {
  readonly id: string;
  readonly lane: number;
  readonly x: number;
  readonly width: number;
  readonly milestone: boolean;
  readonly inferred: ScheduleInference;
}

export interface DayColumn {
  readonly date: IsoDate;
  readonly x: number;
  readonly width: number;
  readonly label: string;
  readonly offDuty: boolean;
}

export interface MonthBand {
  readonly key: string;
  readonly label: string;
  readonly x: number;
  readonly width: number;
}

export interface LinkPath {
  readonly blockerId: string;
  readonly blockedId: string;
  /** x1, y1, x2, y2, ... */
  readonly points: readonly number[];
}

const MILESTONE_WIDTH = 12;
const PACK_GAP = 1;
const ELBOW_PAD = 10;
const DETOUR_STEP = 6;

export function dateToX(d: IsoDate, scale: GanttScale): number | null {
  const offset = daysBetween(scale.start, d);
  return offset === null ? null : offset * scale.pxPerDay;
}

/** The day under `x`, rounded down and clamped to the scale. */
export function xToDate(x: number, scale: GanttScale): IsoDate {
  const days = scale.pxPerDay <= 0 ? 0 : Math.floor(x / scale.pxPerDay);
  const raw = addDays(scale.start, days) ?? scale.start;
  if (raw < scale.start) return scale.start;
  if (raw > scale.end) return scale.end;
  return raw;
}

/** Width of the whole range; both end days count. */
export function scaleWidth(scale: GanttScale): number {
  const span = daysBetween(scale.start, scale.end);
  return span === null || span < 0 ? 0 : (span + 1) * scale.pxPerDay;
}

/** A bar covers both its start and end day. */
export function barRect(
  item: Pick<ScheduledItem, "start" | "end" | "milestone">,
  scale: GanttScale,
): { x: number; width: number } | null {
  const x = dateToX(item.start, scale);
  if (x === null) return null;
  if (item.milestone) {
    return { x: x + scale.pxPerDay / 2 - MILESTONE_WIDTH / 2, width: MILESTONE_WIDTH };
  }
  const span = daysBetween(item.start, item.end);
  if (span === null) return null;
  return { x, width: (span + 1) * scale.pxPerDay };
}

export function laneCenterY(lane: number, laneHeight: number): number {
  return lane * laneHeight + laneHeight / 2;
}

function comparePacking(a: ScheduledItem, b: ScheduledItem): number {
  if (a.start !== b.start) return a.start < b.start ? -1 : 1;
  if (a.id === b.id) return 0;
  return a.id < b.id ? -1 : 1;
}

function box(item: ScheduledItem, lane: number, rect: { x: number; width: number }): BarBox {
  return { id: item.id, lane, x: rect.x, width: rect.width, milestone: item.milestone, inferred: item.inferred };
}

/** One lane per item, ordered by (start, id). */
export function stackRows(items: readonly ScheduledItem[], scale: GanttScale): { bars: BarBox[]; overflow: string[] } {
  const bars: BarBox[] = [];
  const overflow: string[] = [];
  for (const item of [...items].sort(comparePacking)) {
    const rect = barRect(item, scale);
    if (rect === null) overflow.push(item.id);
    else bars.push(box(item, bars.length, rect));
  }
  return { bars, overflow };
}

function firstFreeLane(laneEnd: readonly number[], x: number, minLane: number): number {
  for (let i = minLane; i < laneEnd.length; i++) {
    if ((laneEnd[i] ?? 0) + PACK_GAP <= x) return i;
  }
  return Math.max(laneEnd.length, minLane);
}

function orderByDependency(items: readonly ScheduledItem[], links: readonly GanttLinkInput[]): ScheduledItem[] {
  const byId = new Map(items.map((item) => [item.id, item]));
  const indegree = new Map(items.map((item) => [item.id, 0]));
  const next = new Map(items.map((item) => [item.id, [] as string[]]));
  for (const link of links) {
    if (!byId.has(link.blockerId) || !byId.has(link.blockedId)) continue;
    next.get(link.blockerId)!.push(link.blockedId);
    indegree.set(link.blockedId, (indegree.get(link.blockedId) ?? 0) + 1);
  }
  const ready = items.filter((item) => indegree.get(item.id) === 0).sort(comparePacking);
  const out: ScheduledItem[] = [];
  while (ready.length > 0) {
    const item = ready.shift()!;
    out.push(item);
    for (const id of next.get(item.id) ?? []) {
      const left = (indegree.get(id) ?? 1) - 1;
      indegree.set(id, left);
      if (left === 0) {
        ready.push(byId.get(id)!);
        ready.sort(comparePacking);
      }
    }
  }
  if (out.length < items.length) {
    const seen = new Set(out.map((item) => item.id));
    out.push(...items.filter((item) => !seen.has(item.id)).sort(comparePacking));
  }
  return out;
}

/**
 * Overlap packing: items share a lane when they do not overlap, placed in
 * dependency order, and a blocked item sits no higher than its blockers.
 */
export function packFlow(
  items: readonly ScheduledItem[],
  scale: GanttScale,
  links: readonly GanttLinkInput[],
): { bars: BarBox[]; overflow: string[] } {
  const blockers = new Map<string, string[]>();
  for (const link of links) {
    const list = blockers.get(link.blockedId) ?? [];
    list.push(link.blockerId);
    blockers.set(link.blockedId, list);
  }
  const laneEnd: number[] = [];
  const placed = new Map<string, BarBox>();
  const bars: BarBox[] = [];
  const overflow: string[] = [];
  for (const item of orderByDependency(items, links)) {
    const rect = barRect(item, scale);
    if (rect === null) {
      overflow.push(item.id);
      continue;
    }
    let minLane = 0;
    for (const id of blockers.get(item.id) ?? []) {
      const blocker = placed.get(id);
      if (!blocker) continue;
      const overlaps =
        blocker.x < rect.x + rect.width + PACK_GAP && rect.x < blocker.x + blocker.width + PACK_GAP;
      minLane = Math.max(minLane, overlaps ? blocker.lane + 1 : blocker.lane);
    }
    const lane = firstFreeLane(laneEnd, rect.x, minLane);
    while (laneEnd.length <= lane) laneEnd.push(0);
    laneEnd[lane] = rect.x + rect.width;
    const bar = box(item, lane, rect);
    placed.set(item.id, bar);
    bars.push(bar);
  }
  return { bars, overflow };
}

export function isWorkingDay(d: IsoDate, calendar: WorkCalendar): boolean {
  const dow = dayOfWeek(d);
  if (dow === null) return false;
  return !calendar.weekend.includes(dow) && !calendar.holidays.has(d);
}

/** One column per day; weekends and workspace holidays are off duty. */
export function dayColumns(scale: GanttScale, calendar: WorkCalendar): DayColumn[] {
  const first = toEpochDay(scale.start);
  const last = toEpochDay(scale.end);
  if (first === null || last === null || last < first) return [];
  const out: DayColumn[] = [];
  for (let day = first; day <= last; day++) {
    const date = fromEpochDay(day);
    out.push({
      date,
      x: (day - first) * scale.pxPerDay,
      width: scale.pxPerDay,
      label: String(Number(date.slice(8, 10))),
      offDuty: !isWorkingDay(date, calendar),
    });
  }
  return out;
}

export function monthBands(columns: readonly DayColumn[]): MonthBand[] {
  const out: MonthBand[] = [];
  for (const column of columns) {
    const key = column.date.slice(0, 7);
    const last = out[out.length - 1];
    if (last && last.key === key) {
      out[out.length - 1] = { ...last, width: last.width + column.width };
      continue;
    }
    out.push({ key, label: t("gantt.month", { month: Number(key.slice(5, 7)) }), x: column.x, width: column.width });
  }
  return out;
}

function hitsLabel(y: number, left: number, right: number, bars: readonly BarBox[], laneHeight: number): boolean {
  const half = laneHeight * 0.32;
  return bars.some((bar) => {
    const cy = laneCenterY(bar.lane, laneHeight);
    if (y < cy - half || y > cy + half) return false;
    const x1 = bar.x + bar.width + (bar.milestone ? 64 : 0);
    return left < x1 && right > bar.x;
  });
}

function pickGutter(
  left: number,
  right: number,
  y1: number,
  y2: number,
  bars: readonly BarBox[],
  laneHeight: number,
  used: [number, number, number][],
): number {
  const lo = Math.min(y1, y2);
  const hi = Math.max(y1, y2);
  const preferred: number[] = [];
  if (y1 !== y2) preferred.push((y1 + y2) / 2);
  const gutterY = (Math.floor(lo / laneHeight) + 1) * laneHeight;
  if (gutterY > lo && gutterY < hi) preferred.push(gutterY);
  preferred.push(lo - laneHeight * 0.38, hi + laneHeight * 0.38);
  const seen = new Set<number>();
  for (const base of preferred) {
    const key = Math.trunc(base * 1000);
    if (seen.has(key)) continue;
    seen.add(key);
    for (const delta of [0, -DETOUR_STEP, DETOUR_STEP, -DETOUR_STEP * 2, DETOUR_STEP * 2]) {
      const y = base + delta;
      if (y < 4) continue;
      if (hitsLabel(y, left, right, bars, laneHeight)) continue;
      if (used.some(([uy, ul, ur]) => Math.abs(uy - y) < DETOUR_STEP - 1 && left < ur && right > ul)) continue;
      used.push([y, left, right]);
      return y;
    }
  }
  const fallback = hi + laneHeight * 0.38;
  used.push([fallback, left, right]);
  return fallback;
}

/**
 * Elbow paths from each blocker bar to its blocked bar (server links.rs):
 * FS from the blocker's end to the blocked start, SS start to start, FF end
 * to end, each shifted by the lag. Links whose bars are not drawn are skipped.
 */
export function linkPaths(
  links: readonly GanttLinkInput[],
  bars: readonly BarBox[],
  laneHeight: number,
  pxPerDay: number,
): LinkPath[] {
  const byId = new Map(bars.map((bar) => [bar.id, bar]));
  const used: [number, number, number][] = [];
  const out: LinkPath[] = [];
  for (const link of links) {
    const from = byId.get(link.blockerId);
    const to = byId.get(link.blockedId);
    if (!from || !to) continue;
    const lag = link.lagDays * pxPerDay;
    const [x1, x2] =
      link.type === "SS"
        ? [from.x, to.x + lag]
        : link.type === "FF"
          ? [from.x + from.width, to.x + to.width + lag]
          : [from.x + from.width, to.x + lag];
    const y1 = laneCenterY(from.lane, laneHeight);
    const y2 = laneCenterY(to.lane, laneHeight);
    let points: number[];
    const overlapLeft = Math.max(from.x, to.x);
    const overlapRight = Math.min(from.x + from.width, to.x + to.width);
    if (overlapRight > overlapLeft && y1 !== y2) {
      const cx = (overlapLeft + overlapRight) / 2;
      const edge = laneHeight * 0.26;
      points = [cx, y1 < y2 ? y1 + edge : y1 - edge, cx, y2 < y1 ? y2 + edge : y2 - edge];
    } else {
      const gap = x2 - x1;
      if (gap >= 0 && (gap >= ELBOW_PAD * 2 || y1 !== y2)) {
        const mid = (x1 + x2) / 2;
        points = [x1, y1, mid, y1, mid, y2, x2, y2];
      } else if (gap >= 0) {
        points = [x1, y1, x2, y2];
      } else {
        const detour = pickGutter(x2 - ELBOW_PAD, x1 + ELBOW_PAD, y1, y2, bars, laneHeight, used);
        points = [
          x1, y1,
          x1 + ELBOW_PAD, y1,
          x1 + ELBOW_PAD, detour,
          x2 - ELBOW_PAD, detour,
          x2 - ELBOW_PAD, y2,
          x2, y2,
        ];
      }
    }
    out.push({ blockerId: link.blockerId, blockedId: link.blockedId, points });
  }
  return out;
}

/** Chart height: every lane, and any link detour below the last one. */
export function chartHeight(laneCount: number, laneHeight: number, paths: readonly LinkPath[]): number {
  let maxY = 0;
  for (const path of paths) {
    for (let i = 1; i < path.points.length; i += 2) maxY = Math.max(maxY, path.points[i]!);
  }
  return Math.max(laneCount * laneHeight, maxY + 12, laneHeight);
}

export type BarDragKind = "move" | "start" | "end";

export interface BarDragOrigin {
  readonly kind: BarDragKind;
  readonly originStart: IsoDate;
  readonly originEnd: IsoDate;
  /** Pointer x where the drag started, in chart coordinates. */
  readonly originX: number;
}

/**
 * The bar's dates for pointer `x` during a drag. A move keeps the span and
 * stays inside the scale; a handle moves one end and never passes the other.
 */
export function applyBarPointer(drag: BarDragOrigin, x: number, scale: GanttScale): { start: IsoDate; end: IsoDate } {
  if (drag.kind === "move") {
    const delta = daysBetween(xToDate(drag.originX, scale), xToDate(x, scale)) ?? 0;
    const span = daysBetween(drag.originStart, drag.originEnd) ?? 0;
    return shiftWithin(drag.originStart, span, delta, scale);
  }
  if (drag.kind === "start") {
    const start = xToDate(x, scale);
    return { start: start > drag.originEnd ? drag.originEnd : start, end: drag.originEnd };
  }
  const end = xToDate(x, scale);
  return { start: drag.originStart, end: end < drag.originStart ? drag.originStart : end };
}

/** Moves a bar of `span` extra days by `delta` days, kept inside the scale when it fits. */
export function shiftWithin(
  start: IsoDate,
  span: number,
  delta: number,
  scale: Pick<GanttScale, "start" | "end">,
): { start: IsoDate; end: IsoDate } {
  let next = addDays(start, delta) ?? start;
  if (next < scale.start) next = scale.start;
  let end = addDays(next, span) ?? next;
  if (end > scale.end) {
    end = scale.end;
    next = addDays(end, -span) ?? scale.start;
    if (next < scale.start) next = scale.start;
  }
  return { start: next, end: end < next ? next : end };
}
