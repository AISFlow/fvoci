import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatDisplayId, itemPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import {
  PRIORITIES,
  priorityLabel,
  recurrenceLabel,
  type TaskDetail,
  type TaskListItem,
} from "./task-edit-payload";
import { TASK_TYPES, TASK_TYPE_LABELS, isTaskType } from "./task-types";
import "@/features/projects/projects.css";

const NONE = "";

export function TaskDetailForm({
  slug,
  projectKey,
  task,
  statuses,
  parentCandidates,
  readOnly,
  canEdit,
  pending,
  fieldError,
  actionError,
  onTitleBlur,
  onStatusChange,
  onPriorityChange,
  onTypeChange,
  onParentChange,
  onStartDateBlur,
  onDueDateBlur,
  onEstimateBlur,
  onRecurrenceChange,
  onArchiveToggle,
  onTrash,
  archivePending,
  trashPending,
}: {
  slug: string;
  projectKey: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
  parentCandidates: readonly Pick<TaskListItem, "id" | "type" | "number" | "title">[];
  readOnly: boolean;
  canEdit: boolean;
  pending?: boolean;
  fieldError?: string | null;
  actionError?: string | null;
  onTitleBlur: (title: string) => void | Promise<void>;
  onStatusChange: (statusId: string) => void | Promise<void>;
  onPriorityChange: (priority: string) => void | Promise<void>;
  onTypeChange: (type: string) => void | Promise<void>;
  onParentChange: (parentId: string | null) => void | Promise<void>;
  onStartDateBlur: (value: string) => void | Promise<void>;
  onDueDateBlur: (value: string) => void | Promise<void>;
  onEstimateBlur: (value: string) => void | Promise<void>;
  onRecurrenceChange: (kind: string) => void | Promise<void>;
  onArchiveToggle: (archived: boolean) => void | Promise<void>;
  onTrash: () => void | Promise<void>;
  archivePending?: boolean;
  trashPending?: boolean;
}) {
  const displayId = formatDisplayId(projectKey, task.number);
  const archived = task.archivedAt !== null;
  const recurrence = parseRecurrenceValue(task.recurrence);
  const parentValue = task.parentId ?? NONE;
  const showParent = task.type !== "epic";

  return (
    <div className="task-detail">
      {archived ? (
        <div className="task-detail__archived">
          <p className="task-home__note">{t("task.archive.detailStatus")}</p>
          {canEdit ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={archivePending || pending}
              onClick={() => void onArchiveToggle(false)}
            >
              {t("task.restore.action")}
            </Button>
          ) : null}
        </div>
      ) : null}
      <div className="task-form">
        <div className="task-form__field">
          <Label htmlFor="task-edit-title">{t("task.col.title")}</Label>
          <Input
            id="task-edit-title"
            data-testid="task-edit-title"
            defaultValue={task.title}
            disabled={readOnly || pending}
            onBlur={(event) => {
              if (event.target.value !== task.title) {
                void onTitleBlur(event.target.value);
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
              if (event.key === "Escape") {
                event.currentTarget.value = task.title;
                event.currentTarget.blur();
              }
            }}
          />
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-status">{t("task.col.status")}</Label>
          <select
            id="task-edit-status"
            data-testid="task-edit-status"
            disabled={readOnly || pending}
            value={task.statusId}
            onChange={(event) => void onStatusChange(event.target.value)}
          >
            {statuses.map((status) => (
              <option key={status.id} value={status.id}>
                {status.name}
              </option>
            ))}
          </select>
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-type">{t("task.form.type.label")}</Label>
          <select
            id="task-edit-type"
            data-testid="task-edit-type"
            disabled={readOnly || pending}
            value={task.type}
            onChange={(event) => {
              const next = event.target.value;
              if (isTaskType(next)) void onTypeChange(next);
            }}
          >
            {TASK_TYPES.map((value) => (
              <option key={value} value={value}>
                {TASK_TYPE_LABELS[value]}
              </option>
            ))}
          </select>
        </div>
        {showParent ? (
          <div className="task-form__field">
            <Label htmlFor="task-edit-parent">{t("task.parent.label")}</Label>
            <select
              id="task-edit-parent"
              data-testid="task-edit-parent"
              disabled={readOnly || pending}
              value={parentValue}
              onChange={(event) => {
                const next = event.target.value;
                void onParentChange(next === NONE ? null : next);
              }}
            >
              <option value={NONE}>{t("task.parent.none")}</option>
              {parentCandidates.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {formatDisplayId(projectKey, candidate.number)} {candidate.title}
                </option>
              ))}
            </select>
            {task.parent ? (
              <p className="task-home__note">
                <Link to={itemPath(slug, formatDisplayId(projectKey, task.parent.number))}>
                  {t("task.parent.current")}: {formatDisplayId(projectKey, task.parent.number)}
                </Link>
              </p>
            ) : null}
          </div>
        ) : null}
        <div className="task-form__field">
          <Label htmlFor="task-edit-priority">{t("task.priority")}</Label>
          <select
            id="task-edit-priority"
            data-testid="task-edit-priority"
            disabled={readOnly || pending}
            value={task.priority}
            onChange={(event) => void onPriorityChange(event.target.value)}
          >
            {PRIORITIES.map((value) => (
              <option key={value} value={value}>
                {priorityLabel(value)}
              </option>
            ))}
          </select>
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-start-date">{t("task.activity.field.startDate")}</Label>
          <Input
            id="task-edit-start-date"
            data-testid="task-edit-start-date"
            type="date"
            disabled={readOnly || pending}
            defaultValue={task.startDate ?? ""}
            onBlur={(event) => {
              const next = event.target.value;
              if (next !== (task.startDate ?? "")) void onStartDateBlur(next);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
              if (event.key === "Escape") {
                event.currentTarget.value = task.startDate ?? "";
                event.currentTarget.blur();
              }
            }}
          />
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-due-date">{t("task.dueAllDay")}</Label>
          <Input
            id="task-edit-due-date"
            data-testid="task-edit-due-date"
            type="date"
            disabled={readOnly || pending}
            defaultValue={task.dueDate ?? ""}
            onBlur={(event) => {
              const next = event.target.value;
              if (next !== (task.dueDate ?? "")) void onDueDateBlur(next);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
              if (event.key === "Escape") {
                event.currentTarget.value = task.dueDate ?? "";
                event.currentTarget.blur();
              }
            }}
          />
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-estimate">{t("task.activity.field.estimate")}</Label>
          <Input
            id="task-edit-estimate"
            data-testid="task-edit-estimate"
            inputMode="decimal"
            disabled={readOnly || pending}
            defaultValue={task.estimate ?? ""}
            onBlur={(event) => {
              const next = event.target.value;
              if (next !== (task.estimate ?? "")) void onEstimateBlur(next);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
              if (event.key === "Escape") {
                event.currentTarget.value = task.estimate ?? "";
                event.currentTarget.blur();
              }
            }}
          />
        </div>
        <div className="task-form__field">
          <Label htmlFor="task-edit-recurrence">{t("task.activity.field.recurrence")}</Label>
          <select
            id="task-edit-recurrence"
            data-testid="task-edit-recurrence"
            disabled={readOnly || pending}
            value={recurrence ?? NONE}
            onChange={(event) => void onRecurrenceChange(event.target.value)}
          >
            <option value={NONE}>{t("task.activity.value.none")}</option>
            <option value="daily">{t("task.activity.recurrence.daily")}</option>
            <option value="weekly">{t("task.activity.recurrence.weekly")}</option>
            <option value="monthly">{t("task.activity.recurrence.monthly")}</option>
          </select>
          {recurrence ? (
            <p className="task-home__note">{recurrenceLabel(task.recurrence)}</p>
          ) : null}
        </div>
        <div className="task-form__field">
          <Label>{t("task.assignee")}</Label>
          <p className="task-home__note" data-testid="task-edit-assignee-disabled">
            {t("task.field.notYetAvailable")}
          </p>
        </div>
        <div className="task-form__field">
          <Label>{t("task.filter.labelsShort")}</Label>
          <p className="task-home__note" data-testid="task-edit-labels-disabled">
            {t("task.field.notYetAvailable")}
          </p>
        </div>
        <div className="task-form__field">
          <Label>{t("project.milestones")}</Label>
          <p className="task-home__note" data-testid="task-edit-milestone-disabled">
            {t("task.field.notYetAvailable")}
          </p>
        </div>
        {fieldError ? (
          <p className="task-form__alert" role="alert" data-testid="task-edit-field-error">
            {fieldError}
          </p>
        ) : null}
        {actionError ? (
          <p className="task-form__alert" role="alert" data-testid="task-edit-action-error">
            {actionError}
          </p>
        ) : null}
        {canEdit ? (
          <div className="task-form__actions">
            {!archived ? (
              <Button
                type="button"
                variant="outline"
                disabled={archivePending || pending}
                onClick={() => void onArchiveToggle(true)}
              >
                {t("task.archive.action")}
              </Button>
            ) : null}
            <Button
              type="button"
              variant="outline"
              data-testid="task-edit-trash"
              disabled={trashPending || pending}
              onClick={() => void onTrash()}
            >
              {t("task.trash.action")}
            </Button>
          </div>
        ) : null}
      </div>
      <p className="task-home__note tabular-nums">{displayId}</p>
    </div>
  );
}

function parseRecurrenceValue(value: unknown): string | null {
  if (value == null || typeof value !== "object") return null;
  const kind = (value as { kind?: unknown }).kind;
  return typeof kind === "string" ? kind : null;
}
