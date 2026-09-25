import { t } from "@fvoci/i18n";
import { ProblemError, problemMessage } from "@/lib/api";

const TASK_PROBLEM_KEYS: Record<string, Parameters<typeof t>[0]> = {
  document_version_mismatch: "document version mismatch (optimistic lock)",
  wip_limit_exceeded: "wip limit exceeded",
  task_hierarchy_violation: "task hierarchy violation",
  invalid_recurrence_preset: "invalid recurrence preset",
  task_archived: "task is archived — read-only",
  status_not_in_project_workflow: "status not in project workflow",
  assignee_is_not_a_member: "assignee is not a member",
};

export function taskMutationErrorMessage(
  err: unknown,
  fallback: Parameters<typeof t>[0] = "task.patch.failed",
): string {
  if (!(err instanceof ProblemError)) return t("error.network");
  const mapped = err.code ? TASK_PROBLEM_KEYS[err.code] : undefined;
  if (mapped) return t(mapped);
  return problemMessage(err, fallback);
}

export function taskFieldValidationMessage(issue: "title" | "startDate" | "dueDate" | "estimate" | "type" | "parent"): string {
  switch (issue) {
    case "title":
      return t("task.form.titleRequired");
    case "startDate":
    case "dueDate":
      return t("task.form.dateInvalid");
    case "estimate":
      return t("task.form.estimateInvalid");
    case "type":
      return t("task.form.type.label");
    case "parent":
      return t("task.parent.required");
  }
}
