import { t } from "@fvoci/i18n";

/** Source `packages/contracts/src/tasks.ts` taskType. Not `milestone`. */
export const TASK_TYPES = ["task", "bug", "story", "epic", "subtask"] as const;
export type TaskType = (typeof TASK_TYPES)[number];

export const TASK_TYPE_LABELS: Record<TaskType, string> = {
  task: t("task.type.task"),
  bug: t("task.type.bug"),
  story: t("task.type.story"),
  epic: t("task.type.epic"),
  subtask: t("task.type.subtask"),
};

export function isTaskType(value: string): value is TaskType {
  return (TASK_TYPES as readonly string[]).includes(value);
}

export function taskTypeLabel(value: string): string {
  return isTaskType(value) ? TASK_TYPE_LABELS[value] : value;
}
