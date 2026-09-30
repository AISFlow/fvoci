// The PATCH /tasks/{id} body for a Gantt reschedule. The server derives a
// bar's range from the stored dates (start = startDate, end = dueDate or the
// UTC date of dueAt; one date alone is a one-day bar, inferred from-start or
// from-due; a reversed pair is shown swapped) and checks dependencies. This
// only decides which stored fields a change writes, and it writes no date
// the change did not set:
//
// - A move shifts every date the task has by the same days and adds none: a
//   task with only a due date keeps only a due date.
// - A handle sets the date of its own edge only. The start handle writes
//   startDate (adding it to a due-only task); the end handle writes the
//   finish: dueDate and/or dueAt where the task has them, else a new dueDate
//   (adding it to a start-only task). On a one-date task the handle of the
//   edge its date is on would only move that date, so the chart offers no
//   such handle (GanttChart.vue hasHandle); here it moves the date and the
//   bar stays one day long.
// - A task with both dates saves the range as drawn: a handle writes the
//   dates whose day changed, which on a swapped task (due before start) are
//   both, so it is saved in order.
// - dueAt is never cleared. It moves by the same whole days as its edge in
//   the user's time zone and keeps its time of day there. The server draws a
//   dueAt on its UTC date, so where the zone's UTC offset changes between the
//   two days (a daylight-saving change) the refetched bar can end one day
//   before or after the drop; see the America/New_York test.
//
// expectedDates always carries the dates exactly as the layout returned
// them, so a task changed since the layout loaded is refused with 409
// document_version_mismatch instead of being overwritten.
import { addDays, daysBetween, type IsoDate } from "@/lib/iso-date";
import { shiftInstantDays } from "@/lib/zoned-date";
import type { BarDragKind, ScheduleInference } from "./gantt-geometry";

export interface RescheduleItem {
  readonly startDate?: IsoDate | null;
  readonly dueDate?: IsoDate | null;
  readonly dueAt?: string | null;
  /** Derived range and how it was derived, as the layout returned them. */
  readonly start: IsoDate;
  readonly end: IsoDate;
  readonly inferred: ScheduleInference;
}

export interface BarChange {
  readonly kind: BarDragKind;
  readonly start: IsoDate;
  readonly end: IsoDate;
}

export interface RescheduleBody {
  startDate?: IsoDate;
  dueDate?: IsoDate;
  dueAt?: string;
  expectedDates: { startDate: IsoDate | null; dueDate: IsoDate | null; dueAt: string | null };
}

function shiftDate(date: IsoDate, days: number): IsoDate {
  const next = addDays(date, days);
  if (next === null) throw new RangeError(`invalid date: ${date}`);
  return next;
}

function days(from: IsoDate, to: IsoDate): number {
  const n = daysBetween(from, to);
  if (n === null) throw new RangeError(`invalid date range: ${from}..${to}`);
  return n;
}

/** The body for `change` of `item`, or null when no stored date changes. */
export function rescheduleBody(
  item: RescheduleItem,
  change: BarChange,
  timeZone: string,
): RescheduleBody | null {
  const startDate = item.startDate ?? null;
  const dueDate = item.dueDate ?? null;
  const dueAt = item.dueAt ?? null;
  const body: RescheduleBody = { expectedDates: { startDate, dueDate, dueAt } };

  if (change.kind === "move") {
    const delta = days(item.start, change.start);
    if (delta === 0) return null;
    if (startDate !== null) body.startDate = shiftDate(startDate, delta);
    if (dueDate !== null) body.dueDate = shiftDate(dueDate, delta);
    if (dueAt !== null) body.dueAt = shiftInstantDays(dueAt, delta, timeZone);
    return body;
  }

  const target =
    change.kind === "start"
      ? { start: change.start, end: item.end }
      : { start: item.start, end: change.end };
  if (target.start === item.start && target.end === item.end) return null;
  const hasFinish = dueDate !== null || dueAt !== null;
  // The layout's day for the finish (dueDate, else dueAt's UTC date): the
  // range's start on a swapped task, its end otherwise.
  const finishDay = item.inferred === "swapped" ? item.start : item.end;

  const setStart = (to: IsoDate) => {
    if (startDate !== to) body.startDate = to;
  };
  const setFinish = (to: IsoDate) => {
    if (!hasFinish) {
      body.dueDate = to;
      return;
    }
    const delta = days(finishDay, to);
    if (delta === 0) return;
    if (dueDate !== null) body.dueDate = to;
    if (dueAt !== null) body.dueAt = shiftInstantDays(dueAt, delta, timeZone);
  };

  if (startDate !== null && hasFinish) {
    setStart(target.start);
    setFinish(target.end);
  } else if (change.kind === "start") {
    setStart(target.start);
  } else {
    setFinish(target.end);
  }
  return body.startDate === undefined && body.dueDate === undefined && body.dueAt === undefined
    ? null
    : body;
}
