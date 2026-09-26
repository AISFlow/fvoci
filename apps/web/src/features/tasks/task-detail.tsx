import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { projectTasksPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import { TaskDetailForm } from "./task-detail-form";
import type { TaskDetail, TaskListItem, LabelItem, MilestoneItem } from "./queries";
import type { MemberOutput } from "@/lib/contracts";
import { TaskActivityPanel } from "@/features/comments/task-activity-panel";
import { StarToggle } from "@/features/share/star-toggle";
import { TaskCollectionProperties } from "@/features/collections/task-collection-properties";
import { TaskAttachmentsPanel } from "./task-attachments";
import { TaskBacklinks } from "./task-backlinks";
import { OriginPanel } from "@/features/collections/origin-panel";
import { TaskBodyEditor } from "./task-body-editor";
import { TaskTimeEntries } from "./task-time-entries";
import { Button } from "@/components/ui/button";
import { ConfirmActionButton } from "@/components/confirm-action";
import "@/features/projects/projects.css";

export function TaskDetailView({
  slug,
  workspaceId,
  projectId,
  projectKey,
  projectName,
  task,
  statuses,
  currentUserId,
  members,
  labels,
  milestones,
  dependencyCandidates,
  readOnly,
  canEdit,
  pending,
  fieldError,
  actionError,
  archivePending,
  trashPending,
  formEpoch,
  onTitleBlur,
  onStatusChange,
  onPriorityChange,
  onHierarchySave,
  onDueDateBlur,
  onAssigneesChange,
  onLabelsChange,
  onMilestoneChange,
  onAddDependency,
  onRemoveDependency,
  onArchiveToggle,
  onTrash,
  clonePending,
  deletePending,
  onClone,
  onDelete,
}: {
  slug: string;
  workspaceId: string;
  projectId: string;
  projectKey: string;
  projectName?: string;
  currentUserId: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
  members: readonly MemberOutput[];
  labels: readonly LabelItem[];
  milestones: readonly MilestoneItem[];
  dependencyCandidates: readonly Pick<TaskListItem, "id" | "number" | "title">[];
  readOnly: boolean;
  canEdit: boolean;
  pending?: boolean;
  fieldError?: string | null;
  actionError?: string | null;
  archivePending?: boolean;
  trashPending?: boolean;
  formEpoch?: number;
  onTitleBlur: (title: string) => void | Promise<void>;
  onStatusChange: (statusId: string) => void | Promise<void>;
  onPriorityChange: (priority: string) => void | Promise<void>;
  onHierarchySave: (type: string, parentId: string | null) => void | Promise<void>;
  onDueDateBlur: (value: string) => void | Promise<void>;
  onAssigneesChange: (assigneeIds: string[]) => void | Promise<void>;
  onLabelsChange: (labelIds: string[]) => void | Promise<void>;
  onMilestoneChange: (milestoneId: string | null) => void | Promise<void>;
  onAddDependency: (input: {
    blockedId: string;
    type: "FS" | "SS" | "FF";
    lagDays: number;
  }) => void | Promise<void>;
  onRemoveDependency: (edge: { blockerId: string; blockedId: string }) => void | Promise<void>;
  onArchiveToggle: (archived: boolean) => void | Promise<void>;
  onTrash: () => void | Promise<void>;
  clonePending?: boolean;
  deletePending?: boolean;
  onClone: () => void | Promise<void>;
  onDelete: () => Promise<void>;
}) {
  return (
    <div className="task-home">
      <nav className="task-home__crumb" aria-label={t("nav.breadcrumb")}>
        <Link to={projectTasksPath(slug, projectKey)}>{projectName ?? projectKey}</Link>
        <span aria-hidden="true"> / </span>
        <span>{task.title}</span>
      </nav>
      <h1 className="task-detail__title">{task.title}</h1>
      <div>
        <StarToggle workspaceId={workspaceId} type="task" targetId={task.id} />
      </div>
      <TaskDetailForm
        key={`${task.id}:${formEpoch ?? 0}`}
        slug={slug}
        workspaceId={workspaceId}
        projectId={projectId}
        projectKey={projectKey}
        task={task}
        statuses={statuses}
        members={members}
        labels={labels}
        milestones={milestones}
        dependencyCandidates={dependencyCandidates}
        readOnly={readOnly}
        canEdit={canEdit}
        pending={pending}
        fieldError={fieldError}
        actionError={actionError}
        archivePending={archivePending}
        trashPending={trashPending}
        onTitleBlur={onTitleBlur}
        onStatusChange={onStatusChange}
        onPriorityChange={onPriorityChange}
        onHierarchySave={onHierarchySave}
        onDueDateBlur={onDueDateBlur}
        onAssigneesChange={onAssigneesChange}
        onLabelsChange={onLabelsChange}
        onMilestoneChange={onMilestoneChange}
        onAddDependency={onAddDependency}
        onRemoveDependency={onRemoveDependency}
        onArchiveToggle={onArchiveToggle}
        onTrash={onTrash}
      />
      {canEdit ? (
        <div className="flex flex-wrap gap-2" data-testid="task-detail-actions">
          {!readOnly ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="task-clone"
              disabled={clonePending || pending}
              onClick={() => void onClone()}
            >
              {t("task.clone")}
            </Button>
          ) : null}
          {task.archivedAt == null ? (
            <ConfirmActionButton
              title={t("task.delete.confirm.title")}
              description={t("task.delete.confirm.body")}
              actionLabel={t("task.delete")}
              disabled={deletePending || pending}
              onConfirm={onDelete}
            >
              {t("task.delete")}
            </ConfirmActionButton>
          ) : null}
        </div>
      ) : null}
      <TaskCollectionProperties
        workspaceId={workspaceId}
        taskId={task.id}
        readOnly={readOnly}
      />
      <TaskBodyEditor
        workspaceId={workspaceId}
        slug={slug}
        taskId={task.id}
        readOnly={readOnly}
      />
      <TaskAttachmentsPanel
        workspaceId={workspaceId}
        taskId={task.id}
        readOnly={readOnly || task.archivedAt != null}
      />
      <TaskTimeEntries
        workspaceId={workspaceId}
        taskId={task.id}
        members={members}
        readOnly={readOnly}
      />
      {currentUserId ? (
        <TaskActivityPanel
          key={task.id}
          workspaceId={workspaceId}
          taskId={task.id}
          currentUserId={currentUserId}
          readOnly={readOnly}
        />
      ) : null}
      <TaskBacklinks slug={slug} workspaceId={workspaceId} taskId={task.id} />
      <OriginPanel slug={slug} workspaceId={workspaceId} taskId={task.id} hideWhenEmpty />
    </div>
  );
}
