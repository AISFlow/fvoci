import { t, formatPersonName } from "@fvoci/i18n";
import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatDisplayId, itemPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import {
  PRIORITIES,
  clearsHierarchyParent,
  eligibleParentCandidates,
  patchTypeBody,
  priorityLabel,
  type TaskDetail,
  type TaskListItem,
} from "./task-edit-payload";
import { TASK_TYPES, TASK_TYPE_LABELS, isTaskType, type TaskType } from "./task-types";
import type { MemberOutput } from "@/lib/contracts";
import type { LabelItem } from "./queries";
import "@/features/projects/projects.css";

const NONE = "";

export function TaskDetailForm({
  slug,
  projectKey,
  task,
  statuses,
  parentItems,
  members,
  labels,
  readOnly,
  canEdit,
  pending,
  fieldError,
  actionError,
  onTitleBlur,
  onStatusChange,
  onPriorityChange,
  onHierarchySave,
  onDueDateBlur,
  onAssigneesChange,
  onLabelsChange,
  onArchiveToggle,
  onTrash,
  archivePending,
  trashPending,
}: {
  slug: string;
  projectKey: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
  parentItems: readonly Pick<TaskListItem, "id" | "type" | "number" | "title">[];
  members: readonly MemberOutput[];
  labels: readonly LabelItem[];
  readOnly: boolean;
  canEdit: boolean;
  pending?: boolean;
  fieldError?: string | null;
  actionError?: string | null;
  onTitleBlur: (title: string) => void | Promise<void>;
  onStatusChange: (statusId: string) => void | Promise<void>;
  onPriorityChange: (priority: string) => void | Promise<void>;
  onHierarchySave: (type: string, parentId: string | null) => void | Promise<void>;
  onDueDateBlur: (value: string) => void | Promise<void>;
  onAssigneesChange: (assigneeIds: string[]) => void | Promise<void>;
  onLabelsChange: (labelIds: string[]) => void | Promise<void>;
  onArchiveToggle: (archived: boolean) => void | Promise<void>;
  onTrash: () => void | Promise<void>;
  archivePending?: boolean;
  trashPending?: boolean;
}) {
  const displayId = formatDisplayId(projectKey, task.number);
  const archived = task.archivedAt !== null;
  const [draftType, setDraftType] = useState<TaskType>(isTaskType(task.type) ? task.type : "task");
  const [draftParentId, setDraftParentId] = useState<string | null>(task.parentId);
  const [draftAssigneeIds, setDraftAssigneeIds] = useState<string[]>(() => [...task.assigneeIds]);
  const [draftLabelIds, setDraftLabelIds] = useState<string[]>(() => [...task.labelIds]);
  const [hierarchyError, setHierarchyError] = useState<string | null>(null);

  useEffect(() => {
    setDraftType(isTaskType(task.type) ? task.type : "task");
    setDraftParentId(task.parentId);
    setHierarchyError(null);
  }, [task.id, task.type, task.parentId]);

  const parentCandidates = useMemo(
    () => eligibleParentCandidates({ id: task.id, type: draftType }, parentItems),
    [draftType, parentItems, task.id],
  );
  const showParent = draftType !== "epic";
  const hierarchyDirty = draftType !== task.type || draftParentId !== task.parentId;

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
            aria-label={t("task.title")}
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
        <fieldset className="task-form__hierarchy" disabled={readOnly || pending}>
          <legend>{t("task.hierarchy.edit")}</legend>
          <div className="task-form__field">
            <Label htmlFor="task-edit-type">{t("task.detail.type.label")}</Label>
            <select
              id="task-edit-type"
              data-testid="task-edit-type"
              disabled={readOnly || pending}
              value={draftType}
              onChange={(event) => {
                const next = event.target.value;
                if (!isTaskType(next)) return;
                if (clearsHierarchyParent(draftType, next)) {
                  setDraftParentId(null);
                }
                setDraftType(next);
                setHierarchyError(null);
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
                value={draftParentId ?? NONE}
                onChange={(event) => {
                  const next = event.target.value;
                  setDraftParentId(next === NONE ? null : next);
                  setHierarchyError(null);
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
          {hierarchyError ? (
            <p className="task-form__alert" role="alert" data-testid="task-edit-hierarchy-error">
              {hierarchyError}
            </p>
          ) : null}
          <div className="task-form__hierarchy-actions">
            <Button
              type="button"
              variant="outline"
              data-testid="task-edit-hierarchy-cancel"
              disabled={readOnly || pending || !hierarchyDirty}
              onClick={() => {
                setDraftType(isTaskType(task.type) ? task.type : "task");
                setDraftParentId(task.parentId);
                setHierarchyError(null);
              }}
            >
              {t("task.hierarchy.cancel")}
            </Button>
            <Button
              type="button"
              data-testid="task-edit-hierarchy-save"
              disabled={readOnly || pending || !hierarchyDirty}
              onClick={() => {
                const parsed = patchTypeBody(draftType, draftParentId);
                if (!parsed.ok) {
                  setHierarchyError(t("task.parent.required"));
                  return;
                }
                void onHierarchySave(draftType, parsed.body.parentId ?? null);
              }}
            >
              {t("task.hierarchy.save")}
            </Button>
          </div>
        </fieldset>
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
        <fieldset className="task-form__field" disabled={readOnly} data-testid="task-edit-assignees">
          <legend>{t("task.assignee")}</legend>
          <div className="task-form__checks">
            {members.map((member) => {
              const checked = draftAssigneeIds.includes(member.userId);
              return (
                <label key={member.userId} className="task-form__check">
                  <input
                    type="checkbox"
                    data-testid={`task-edit-assignee-${member.userId}`}
                    checked={checked}
                    disabled={readOnly}
                    onChange={() => {
                      const next = checked
                        ? draftAssigneeIds.filter((id) => id !== member.userId)
                        : [...draftAssigneeIds, member.userId];
                      setDraftAssigneeIds(next);
                      void onAssigneesChange(next);
                    }}
                  />
                  <span>{formatPersonName(member)}</span>
                </label>
              );
            })}
          </div>
        </fieldset>
        <fieldset className="task-form__field" disabled={readOnly} data-testid="task-edit-labels">
          <legend>{t("task.filter.labelsShort")}</legend>
          <div className="task-form__checks">
            {labels.length === 0 ? (
              <p className="task-home__note">{t("task.activity.value.none")}</p>
            ) : (
              labels.map((label) => {
                const checked = draftLabelIds.includes(label.id);
                return (
                  <label key={label.id} className="task-form__check">
                    <input
                      type="checkbox"
                      data-testid={`task-edit-label-${label.id}`}
                      checked={checked}
                      disabled={readOnly}
                      onChange={() => {
                        const next = checked
                          ? draftLabelIds.filter((id) => id !== label.id)
                          : [...draftLabelIds, label.id];
                        setDraftLabelIds(next);
                        void onLabelsChange(next);
                      }}
                    />
                    <span>{label.name}</span>
                  </label>
                );
              })
            )}
          </div>
        </fieldset>
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
