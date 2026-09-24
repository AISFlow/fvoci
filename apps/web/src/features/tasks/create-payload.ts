import { isTaskType, type TaskType } from "./task-types.ts";

/** Source `TASK_TITLE_MAX`. */
export const TASK_TITLE_MAX = 500;

export type TaskCreateFormValues = {
  title: string;
  type: string;
};

export type TaskCreateBody = {
  title: string;
  type: TaskType;
};

export type TaskCreateIssue = "title" | "type" | "parent";

/**
 * Source `taskCreateInput` strict: trim title 1–500, type enum default `task`.
 * Omits `parentId` (null is invalid). Subtask without a parent picker is rejected.
 */
export function taskCreatePayload(
  values: TaskCreateFormValues,
): { ok: true; body: TaskCreateBody } | { ok: false; issue: TaskCreateIssue } {
  const title = values.title.trim();
  if (title.length < 1 || title.length > TASK_TITLE_MAX) {
    return { ok: false, issue: "title" };
  }
  if (!isTaskType(values.type)) {
    return { ok: false, issue: "type" };
  }
  if (values.type === "subtask") {
    return { ok: false, issue: "parent" };
  }
  return { ok: true, body: { title, type: values.type } };
}
