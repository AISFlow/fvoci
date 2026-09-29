// The PATCH /tasks/{id} body for a Gantt reschedule. The server derives a
// bar's range from the stored dates (start = startDate, end = dueDate or the
// UTC date of dueAt; one date alone is a one-day bar; a reversed pair is
// shown swapped) and checks dependencies; this only decides which stored
// fields a drag writes:
//
// - Moving a bar shifts every date the task has by the same number of days
//   and adds none: a task with only a due date keeps only a due date.
// - A handle sets that edge of the range. When the task had a single date,
//   the other edge (where the bar was) is written too, so the saved range is
//   the one drawn.
// - dueAt is never cleared; it moves by whole days in the user's time zone
//   and keeps its time of day there.
//
// expectedDates always carries the dates exactly as the layout returned
// them, so a task changed since the layout loaded is refused with 409
// document_version_mismatch instead of being overwritten.
import { addDays, daysBetween, type IsoDate } from "@/lib/iso-date";
import { shiftInstantDays } from "@/lib/zoned-date";
import type { BarDragKind } from "./gantt-geometry";

export interface RescheduleItem {
  readonly startDate?: IsoDate | null;
  readonly dueDate?: IsoDate | null;
  readonly dueAt?: string | null;
  /** Derived range as the layout returned it. */
  readonly start: IsoDate;
  readonly end: IsoDate;
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
export function rescheduleBody(item: RescheduleItem, change: BarChange, timeZone: string): RescheduleBody | null {
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

  const target = change.kind === "start" ? { start: change.start, end: item.end } : { start: item.start, end: change.end };
  if (target.start === item.start && target.end === item.end) return null;
  // The finish the server derives: dueDate, else dueAt's UTC date.
  const finish = dueDate ?? (dueAt === null ? null : new Date(dueAt).toISOString().slice(0, 10));
  const setFinish = (to: IsoDate) => {
    if (finish === null) {
      body.dueDate = to;
      return;
    }
    const delta = days(finish, to);
    if (delta === 0) return;
    if (dueDate !== null) body.dueDate = to;
    if (dueAt !== null) body.dueAt = shiftInstantDays(dueAt, delta, timeZone);
  };

  if (target.start === target.end && (startDate === null || finish === null)) {
    // Still a one-date task: move its single date.
    if (startDate !== null) body.startDate = target.start;
    else setFinish(target.end);
  } else {
    if (startDate !== target.start) body.startDate = target.start;
    setFinish(target.end);
  }
  return body.startDate === undefined && body.dueDate === undefined && body.dueAt === undefined ? null : body;
}
