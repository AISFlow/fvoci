// Pure rules for moving a collection item to another calendar day. Source
// apps/web/src/features/collections/collection-panel.tsx (`move` with a date
// target) decides the same writes; collection-calendar.tsx and
// collection-drag.tsx provide the drop targets (a day or "none").
import {
  isoToZonedLocal,
  zonedLocalToIso,
  type CollectionValue,
} from "@/lib/collection-values";
import type { CollectionField } from "@/lib/queries/collections";
import { patchDateBody, type PatchTaskBody } from "@/features/tasks/task-edit-payload";

/** `dataTransfer` type carrying the dragged item id. */
export const CALENDAR_DRAG_TYPE = "application/x-fvoci-collection-date-item";

/** Wall time a datetime value gets when it had no time to keep (source default). */
export const DEFAULT_LOCAL_TIME = "09:00";

/** A query row or a calendar preview: what a date move needs to know. */
export type CalendarRow = {
  id: string;
  /** Calendar day the server bucketed this item into (user time zone). */
  date: string | null;
  canEdit: boolean;
  taskId: string | null;
  startDate: string | null;
  dueDate: string | null;
  dueAt: string | null;
  values: Record<string, never>;
  version: number;
};

export type DateMoveRequest =
  | {
      kind: "task";
      taskId: string;
      body: Pick<PatchTaskBody, "startDate" | "dueDate" | "dueAt" | "expectedDates">;
    }
  | {
      kind: "field";
      fieldId: string;
      expectedVersion: number;
      expectedFieldVersion: number;
      value: CollectionValue;
    }
  /** The target day has no such wall time in the user's zone (DST gap). */
  | { kind: "unavailable" };

type DateField = Pick<CollectionField, "id" | "type" | "version" | "deletedAt">;

function dateField(dateBy: string, fields: readonly DateField[]): DateField | null {
  const field = fields.find((item) => item.id === dateBy);
  if (!field || field.deletedAt !== null) return null;
  return field.type === "date" || field.type === "datetime" ? field : null;
}

/** Whether `row` can be dragged to some other day under `dateBy`. */
export function dateMovable(
  dateBy: string | null,
  row: Pick<CalendarRow, "canEdit" | "taskId">,
  fields: readonly DateField[],
): boolean {
  if (!dateBy || !row.canEdit) return false;
  if (dateBy === "due" || dateBy === "start") return row.taskId !== null;
  return dateField(dateBy, fields) !== null;
}

/**
 * The write that moves `row` to `target` (`YYYY-MM-DD`, or null for undated),
 * or null when the move is not allowed: no date basis, read-only row, same day,
 * a built-in date on a non-task item, or a missing/archived/non-date field.
 *
 * Built-in dates send `expectedDates` so a concurrent date edit conflicts
 * instead of being overwritten; a due move also clears `dueAt`. A `date` field
 * stays a plain date. A `datetime` field keeps its wall time in `timeZone` (the
 * zone the server buckets it by) and only changes the day.
 */
export function dateMoveRequest(
  dateBy: string | null,
  row: CalendarRow,
  target: string | null,
  fields: readonly DateField[],
  timeZone: string,
): DateMoveRequest | null {
  if (!dateBy || row.date === target || !dateMovable(dateBy, row, fields)) return null;
  if (dateBy === "due" || dateBy === "start") {
    if (!row.taskId) return null;
    const patch = patchDateBody(row, dateBy === "due" ? "dueDate" : "startDate", target ?? "");
    return patch.ok ? { kind: "task", taskId: row.taskId, body: patch.body } : null;
  }
  const field = dateField(dateBy, fields);
  if (!field) return null;
  const request = {
    kind: "field" as const,
    fieldId: field.id,
    expectedVersion: row.version,
    expectedFieldVersion: field.version,
  };
  if (target === null) return { ...request, value: null };
  if (field.type === "date") return { ...request, value: { date: target } };
  const previous = (row.values as Record<string, unknown>)[field.id];
  const previousLocal =
    previous && typeof previous === "object" && "datetime" in previous
      ? isoToZonedLocal(String((previous as { datetime: unknown }).datetime), timeZone)
      : "";
  const time = previousLocal ? previousLocal.slice(11, 16) : DEFAULT_LOCAL_TIME;
  const instant = zonedLocalToIso(`${target}T${time}`, timeZone);
  return instant ? { ...request, value: { datetime: instant } } : { kind: "unavailable" };
}
