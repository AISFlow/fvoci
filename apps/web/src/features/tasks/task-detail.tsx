import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { projectTasksPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import { TaskDetailForm } from "./task-detail-form";
import type { TaskDetail } from "./queries";
import type { TaskListItem } from "./queries";
import "@/features/projects/projects.css";

export function TaskDetailView({
  slug,
  projectKey,
  projectName,
  task,
  statuses,
  parentCandidates,
  readOnly,
  canEdit,
  pending,
  fieldError,
  actionError,
  archivePending,
  trashPending,
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
}: {
  slug: string;
  projectKey: string;
  projectName?: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
  parentCandidates: readonly Pick<TaskListItem, "id" | "type" | "number" | "title">[];
  readOnly: boolean;
  canEdit: boolean;
  pending?: boolean;
  fieldError?: string | null;
  actionError?: string | null;
  archivePending?: boolean;
  trashPending?: boolean;
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
}) {
  return (
    <div className="task-home">
      <nav className="task-home__crumb" aria-label={t("nav.breadcrumb")}>
        <Link to={projectTasksPath(slug, projectKey)}>{projectName ?? projectKey}</Link>
        <span aria-hidden="true"> / </span>
        <span>{task.title}</span>
      </nav>
      <TaskDetailForm
        slug={slug}
        projectKey={projectKey}
        task={task}
        statuses={statuses}
        parentCandidates={parentCandidates}
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
        onTypeChange={onTypeChange}
        onParentChange={onParentChange}
        onStartDateBlur={onStartDateBlur}
        onDueDateBlur={onDueDateBlur}
        onEstimateBlur={onEstimateBlur}
        onRecurrenceChange={onRecurrenceChange}
        onArchiveToggle={onArchiveToggle}
        onTrash={onTrash}
      />
      <section className="task-detail__body" aria-label={t("doc.body.a11y")}>
        <p className="task-home__note">{t("task.body.unavailable")}</p>
      </section>
    </div>
  );
}
