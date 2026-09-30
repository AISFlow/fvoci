import { t } from "@fvoci/i18n";
import { TASK_TYPE_LABELS, isTaskType } from "@/features/tasks/task-types";
import type { components } from "@/generated/api";

type ApiActivityItem = components["schemas"]["ActivityItemOutput"];

type ActivityValue =
  | null
  | string
  | boolean
  | { id: string; label: string | null }
  | { items: { id: string; label: string | null }[]; totalCount: number };

type ActivityChange = {
  field: string;
  from: ActivityValue;
  to: ActivityValue;
};

export type ActivityChangeItem = {
  type: "change";
  id: string;
  createdAt: string;
  actor: { id: string; name: string } | null;
  channel: string;
  kind: "created" | "changed";
  changes: ActivityChange[];
};

const FIELD_KEYS: Record<string, string> = {
  title: "task.activity.field.title",
  type: "task.activity.field.type",
  priority: "task.activity.field.priority",
  statusId: "task.activity.field.status",
  startDate: "task.activity.field.startDate",
  dueDate: "task.activity.field.dueDate",
  dueAt: "task.activity.field.dueTime",
  estimate: "task.activity.field.estimate",
  parentId: "task.activity.field.parent",
  milestoneId: "task.activity.field.milestone",
  recurrence: "task.activity.field.recurrence",
  archived: "task.activity.field.archived",
  assigneeIds: "task.activity.field.assignees",
  labelIds: "task.activity.field.labels",
};

export const FALLBACK_TIME_ZONE = "Asia/Seoul";

export function formatActivityTime(iso: string, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat("ko", {
      timeZone,
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(iso));
  } catch {
    return iso;
  }
}

export function actorName(item: ActivityChangeItem): string {
  if (item.actor !== null) return item.actor.name;
  switch (item.channel) {
    case "api":
      return t("task.activity.actor.api");
    case "mcp":
      return t("task.activity.actor.mcp");
    case "webhook":
      return t("task.activity.actor.webhook");
    case "system":
      return t("task.activity.actor.system");
    case "web":
      return t("task.activity.actor.unknown");
    default:
      return t("task.activity.actor.unknown");
  }
}

export function fieldLabel(field: string): string {
  const key = FIELD_KEYS[field];
  return key ? t(key as Parameters<typeof t>[0]) : field;
}

export function displayValue(field: string, value: ActivityValue): string {
  if (value === null) return t("task.activity.value.none");
  if (typeof value === "boolean") {
    if (field === "archived") {
      return value ? t("task.activity.value.archived") : t("task.activity.value.active");
    }
    return String(value);
  }
  if (typeof value === "string") {
    if (field === "type" && isTaskType(value)) return TASK_TYPE_LABELS[value];
    if (field === "priority") {
      switch (value) {
        case "none":
          return t("task.activity.priority.none");
        case "low":
          return t("task.activity.priority.low");
        case "medium":
          return t("task.activity.priority.medium");
        case "high":
          return t("task.activity.priority.high");
        case "urgent":
          return t("task.activity.priority.urgent");
      }
    }
    if (field === "recurrence") {
      switch (value) {
        case "daily":
          return t("task.activity.recurrence.daily");
        case "weekly":
          return t("task.activity.recurrence.weekly");
        case "monthly":
          return t("task.activity.recurrence.monthly");
      }
    }
    return value;
  }
  if ("items" in value) {
    const labels = value.items
      .map((entry) => entry.label ?? t("task.activity.value.unavailable"))
      .join(", ");
    const remaining = value.totalCount - value.items.length;
    const suffix =
      remaining > 0 ? ` ${t("task.activity.value.additional", { count: remaining })}` : "";
    return value.totalCount === 0 ? t("task.activity.value.none") : `${labels}${suffix}`;
  }
  return value.label ?? t("task.activity.value.unavailable");
}

export function changeItem(item: Extract<ApiActivityItem, { type: "change" }>): ActivityChangeItem {
  return {
    ...item,
    actor: item.actor ?? null,
    kind: item.kind === "created" ? "created" : "changed",
    changes: item.changes as ActivityChangeItem["changes"],
  };
}
