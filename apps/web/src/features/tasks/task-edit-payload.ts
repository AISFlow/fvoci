import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { isTaskType } from "./task-types.ts";

export type PatchTaskBody = components["schemas"]["PatchTaskBody"];
export type ExpectedDatesBody = components["schemas"]["ExpectedDatesBody"];
export type TaskDetail = components["schemas"]["TaskOutput"];
export type TaskListItem = components["schemas"]["TaskListItemOutput"];

export const PRIORITIES = ["none", "low", "medium", "high", "urgent"] as const;
export type TaskPriority = (typeof PRIORITIES)[number];

export const RECURRENCE_KINDS = ["daily", "weekly", "monthly"] as const;
export type RecurrenceKind = (typeof RECURRENCE_KINDS)[number];

const ISO_DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

/** Matches Rust `parse_iso_date`: strict `YYYY-MM-DD`, rejects year 0000. */
export function isIsoDate(value: string): boolean {
  if (!ISO_DATE_RE.test(value) || value.startsWith("0000")) return false;
  const [year, month, day] = value.split("-").map((part) => Number.parseInt(part, 10));
  const date = new Date(Date.UTC(year, month - 1, day));
  return (
    date.getUTCFullYear() === year &&
    date.getUTCMonth() === month - 1 &&
    date.getUTCDate() === day
  );
}

/** Matches Rust `estimate_is_valid`. */
export function isEstimateValid(value: string): boolean {
  const bytes = value;
  if (bytes.length === 0 || bytes.length > 19) return false;
  const dot = bytes.indexOf(".");
  if (dot === -1) {
    return bytes.length <= 12 && /^\d+$/.test(bytes);
  }
  const intPart = bytes.slice(0, dot);
  const fracPart = bytes.slice(dot + 1);
  return (
    intPart.length > 0 &&
    intPart.length <= 12 &&
    fracPart.length > 0 &&
    fracPart.length <= 6 &&
    /^\d+$/.test(intPart) &&
    /^\d+$/.test(fracPart)
  );
}

export function isTaskPriority(value: string): value is TaskPriority {
  return (PRIORITIES as readonly string[]).includes(value);
}

export function isRecurrenceKind(value: string): value is RecurrenceKind {
  return (RECURRENCE_KINDS as readonly string[]).includes(value);
}

export function parseRecurrence(value: unknown): RecurrenceKind | null {
  if (value == null) return null;
  if (typeof value !== "object" || value === null) return null;
  const kind = (value as { kind?: unknown }).kind;
  return typeof kind === "string" && isRecurrenceKind(kind) ? kind : null;
}

export function recurrenceBody(kind: RecurrenceKind | null): unknown | null {
  return kind ? { kind } : null;
}

export function priorityLabel(value: string): string {
  if (value === "none") return t("task.priority.none");
  if (value === "low") return t("priority.low");
  if (value === "medium") return t("priority.medium");
  if (value === "high") return t("priority.high");
  if (value === "urgent") return t("priority.urgent");
  return value;
}

export function recurrenceLabel(value: unknown): string {
  const kind = parseRecurrence(value);
  if (kind === "daily") return t("task.activity.recurrence.daily");
  if (kind === "weekly") return t("task.activity.recurrence.weekly");
  if (kind === "monthly") return t("task.activity.recurrence.monthly");
  return t("task.activity.value.none");
}

export function violatesTaskHierarchy(childType: string, parentType: string): boolean {
  if (childType === "subtask") {
    return !["task", "bug", "story"].includes(parentType);
  }
  if (childType === "epic") return true;
  return parentType !== "epic";
}

export function eligibleParentCandidates(
  task: Pick<TaskDetail, "id" | "type">,
  items: readonly Pick<TaskListItem, "id" | "type" | "number" | "title">[],
): Pick<TaskListItem, "id" | "type" | "number" | "title">[] {
  return items.filter((item) => {
    if (item.id === task.id) return false;
    return !violatesTaskHierarchy(task.type, item.type);
  });
}

export type TitlePatchIssue = "title";
export type DatePatchIssue = "startDate" | "dueDate";
export type EstimatePatchIssue = "estimate";

export function patchTitleBody(title: string): { ok: true; body: Pick<PatchTaskBody, "title"> } | { ok: false; issue: TitlePatchIssue } {
  const trimmed = title.trim();
  if (trimmed.length < 1 || trimmed.length > 500) {
    return { ok: false, issue: "title" };
  }
  return { ok: true, body: { title: trimmed } };
}

export function patchDateBody(
  task: Pick<TaskDetail, "startDate" | "dueDate" | "dueAt">,
  field: DatePatchIssue,
  raw: string,
): { ok: true; body: Pick<PatchTaskBody, "startDate" | "dueDate" | "expectedDates"> } | { ok: false; issue: DatePatchIssue } {
  const next = raw === "" ? null : raw;
  if (next !== null && !isIsoDate(next)) {
    return { ok: false, issue: field };
  }
  const expectedDates: ExpectedDatesBody = {
    startDate: task.startDate,
    dueDate: task.dueDate,
    dueAt: task.dueAt,
  };
  if (field === "startDate") {
    return { ok: true, body: { startDate: next, expectedDates } };
  }
  return { ok: true, body: { dueDate: next, expectedDates } };
}

export function patchEstimateBody(
  raw: string,
): { ok: true; body: Pick<PatchTaskBody, "estimate"> } | { ok: false; issue: EstimatePatchIssue } {
  const next = raw.trim() === "" ? null : raw.trim();
  if (next !== null && !isEstimateValid(next)) {
    return { ok: false, issue: "estimate" };
  }
  return { ok: true, body: { estimate: next } };
}

export function patchTypeBody(
  type: string,
  parentId: string | null,
): { ok: true; body: Pick<PatchTaskBody, "type" | "parentId"> } | { ok: false; issue: "type" | "parent" } {
  if (!isTaskType(type)) return { ok: false, issue: "type" };
  if (type === "subtask" && parentId === null) {
    return { ok: false, issue: "parent" };
  }
  return {
    ok: true,
    body: {
      type,
      parentId: type === "epic" ? null : parentId,
    },
  };
}
