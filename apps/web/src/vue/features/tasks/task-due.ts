import { formatInstant } from "@/lib/datetime";
// Source393795 task-bits taskDueLabel: an all-day due date is a calendar date, not a UTC instant.
export function taskDueLabel(
  dueDate: string | null | undefined,
  dueAt: string | null | undefined,
  timeZone: string,
): string {
  if (dueAt)
    return formatInstant(dueAt, timeZone, {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  if (!dueDate) return "";
  const [, month, day] = dueDate.split("-");
  return month === undefined || day === undefined ? dueDate : `${Number(month)}. ${Number(day)}.`;
}
