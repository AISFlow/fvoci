// FVOCI adapter for nuxt-ui-templates/calendar @11809148a32a40612d1d7ddab8aef5372ad46edf.
// Dates are server buckets; instants are points, never invented event intervals.
import {
  dateMovable,
  dateMoveRequest,
  type CalendarRow,
  type DateMoveRequest,
} from "@/features/collections/calendar-model";
import { isIsoDate, patchDateBody } from "@/features/tasks/task-edit-payload";
import { isoToZonedLocal, zonedLocalToIso } from "@/lib/collection-values";
import type { CollectionField, CollectionQueryPreview } from "@/lib/queries/collections";

export type CalendarView = "day" | "week" | "month";
export type CalendarEvent = CollectionQueryPreview & { local: string; timed: boolean };
export type CalendarWrite = Exclude<DateMoveRequest, { kind: "unavailable" }>;
export function eventFor(
  row: CollectionQueryPreview,
  dateBy: string,
  fields: readonly CollectionField[],
  zone: string,
): CalendarEvent {
  const field = fields.find((f) => f.id === dateBy);
  const value = row.values[dateBy] as { datetime?: string } | undefined;
  const instant =
    dateBy === "due"
      ? row.dueDate === null
        ? row.dueAt
        : null
      : field?.type === "datetime"
        ? value?.datetime
        : null;
  return {
    ...row,
    timed: Boolean(instant),
    local: instant ? isoToZonedLocal(instant, zone) : (row.date ?? ""),
  };
}
export function addDays(day: string, count: number): string {
  const date = new Date(`${day}T12:00:00Z`);
  date.setUTCDate(date.getUTCDate() + count);
  return date.toISOString().slice(0, 10);
}
export function weekDays(day: string, starts: number): string[] {
  const weekday = new Date(`${day}T12:00:00Z`).getUTCDay();
  const first = addDays(day, -((weekday - starts + 7) % 7));
  return Array.from({ length: 7 }, (_, i) => addDays(first, i));
}
export function editorWrite(
  dateBy: string,
  row: CalendarRow,
  raw: string,
  timed: boolean,
  fields: readonly CollectionField[],
  zone: string,
): CalendarWrite | null {
  if (!dateMovable(dateBy, row, fields)) return null;
  const field = fields.find((f) => f.id === dateBy && f.deletedAt === null);
  if (dateBy === "due" && timed) {
    if (!row.taskId) return null;
    const instant =
      raw === ""
        ? null
        : row.dueAt && isoToZonedLocal(row.dueAt, zone) === raw
          ? row.dueAt
          : zonedLocalToIso(raw, zone);
    if (raw && !instant) return null;
    return {
      kind: "task",
      taskId: row.taskId,
      body: {
        dueDate: null,
        dueAt: instant,
        expectedDates: { startDate: row.startDate, dueDate: row.dueDate, dueAt: row.dueAt },
      },
    };
  }
  if (dateBy === "due" && row.taskId && row.dueDate !== null && raw === row.dueDate) {
    // A no-change save must preserve a secondary dueAt in existing dual-field
    // records. Only an explicit changed date or timed conversion clears it.
    return {
      kind: "task",
      taskId: row.taskId,
      body: {
        dueDate: row.dueDate,
        expectedDates: { startDate: row.startDate, dueDate: row.dueDate, dueAt: row.dueAt },
      },
    };
  }
  if (dateBy === "start" || dateBy === "due") {
    const patch = patchDateBody(row, dateBy === "due" ? "dueDate" : "startDate", raw);
    return patch.ok && row.taskId ? { kind: "task", taskId: row.taskId, body: patch.body } : null;
  }
  if (!field || !["date", "datetime"].includes(field.type)) return null;
  if (field.type === "date" && raw && !isIsoDate(raw)) return null;
  const previous = row.values[field.id] as { datetime?: string } | undefined;
  const instant =
    field.type === "datetime" && raw
      ? previous?.datetime && isoToZonedLocal(previous.datetime, zone) === raw
        ? previous.datetime
        : zonedLocalToIso(raw, zone)
      : null;
  if (field.type === "datetime" && raw && !instant) return null;
  return {
    kind: "field",
    fieldId: field.id,
    expectedVersion: row.version,
    expectedFieldVersion: field.version,
    value: raw === "" ? null : field.type === "date" ? { date: raw } : { datetime: instant! },
  };
}
export function dayMove(
  dateBy: string,
  row: CalendarRow,
  day: string | null,
  fields: readonly CollectionField[],
  zone: string,
): CalendarWrite | null {
  if (day !== null && !isIsoDate(day)) return null;
  const write = dateMoveRequest(dateBy, row, day, fields, zone);
  return write && write.kind !== "unavailable" ? write : null;
}
/** Preview only; accepted responses and refetch replace it before pending settles. */
export function optimisticRow(
  row: CollectionQueryPreview,
  write: CalendarWrite,
  zone: string,
  dateBy?: string,
): CollectionQueryPreview {
  if (write.kind === "task") {
    const next = { ...row, ...write.body };
    const date = (dateBy ? dateBy === "start" : "startDate" in write.body)
      ? next.startDate
      : (next.dueDate ?? (next.dueAt ? isoToZonedLocal(next.dueAt, zone).slice(0, 10) : null));
    return { ...next, date };
  }
  const value = write.value as { date?: string; datetime?: string } | null;
  return {
    ...row,
    values: { ...row.values, [write.fieldId]: write.value } as never,
    date:
      value?.date ?? (value?.datetime ? isoToZonedLocal(value.datetime, zone).slice(0, 10) : null),
  };
}
/** Existing plain task endpoints only. Rust schedule_task exposes these as start/end;
 * reversed dates are stored but inferred/swapped, so never silently resize those. */
export function resizable(row: CalendarRow, dateBy: string): boolean {
  return (
    (dateBy === "due" || dateBy === "start") &&
    row.canEdit &&
    Boolean(
      row.taskId && row.startDate && row.dueDate && !row.dueAt && row.startDate <= row.dueDate,
    )
  );
}
export function resizeWrite(
  row: CalendarRow,
  dateBy: string,
  edge: "start" | "end",
  day: string,
): CalendarWrite | null {
  if (!resizable(row, dateBy) || !isIsoDate(day)) return null;
  // Refuse crossing the other stored endpoint; no normalization or new date.
  if (edge === "start" ? day > row.dueDate! : day < row.startDate!) return null;
  if (day === (edge === "start" ? row.startDate : row.dueDate)) return null;
  return {
    kind: "task",
    taskId: row.taskId!,
    body: {
      [edge === "start" ? "startDate" : "dueDate"]: day,
      expectedDates: { startDate: row.startDate, dueDate: row.dueDate, dueAt: row.dueAt },
    },
  };
}
