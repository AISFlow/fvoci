import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { formatDisplayId, projectTasksPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import { formatEstimate, stringIds, type TaskDetail } from "./queries";
import { taskTypeLabel } from "./task-types";
import "@/features/projects/projects.css";

export function TaskDetailView({
  slug,
  projectKey,
  projectName,
  task,
  statuses,
}: {
  slug: string;
  projectKey: string;
  projectName?: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
}) {
  const displayId = formatDisplayId(projectKey, task.number);
  const status = statuses.find((item) => item.id === task.statusId);
  const estimate = formatEstimate(task.estimate);
  const assignees = stringIds(task.assigneeIds);
  const labels = stringIds(task.labelIds);

  return (
    <div className="task-home">
      <nav className="task-home__crumb" aria-label={t("nav.breadcrumb")}>
        <Link to={projectTasksPath(slug, projectKey)}>{projectName ?? projectKey}</Link>
        <span aria-hidden="true"> / </span>
        <span>{displayId}</span>
      </nav>
      <h1 className="task-detail__title">{task.title}</h1>
      <dl className="task-detail__dl">
        <div>
          <dt>{t("task.col.status")}</dt>
          <dd>{status?.name ?? task.statusId}</dd>
        </div>
        <div>
          <dt>{t("task.form.type.label")}</dt>
          <dd>{taskTypeLabel(task.type)}</dd>
        </div>
        <div>
          <dt>{t("task.priority")}</dt>
          <dd>{task.priority}</dd>
        </div>
        {estimate ? (
          <div>
            <dt>{t("task.activity.field.estimate")}</dt>
            <dd>{estimate}</dd>
          </div>
        ) : null}
        {assignees.length > 0 ? (
          <div>
            <dt>{t("task.assignee")}</dt>
            <dd>{assignees.length}</dd>
          </div>
        ) : null}
        {labels.length > 0 ? (
          <div>
            <dt>{t("task.filter.labelsShort")}</dt>
            <dd>{labels.length}</dd>
          </div>
        ) : null}
      </dl>
      <section className="task-detail__body" aria-label={t("doc.body.a11y")}>
        <p className="task-home__note">{t("task.body.unavailable")}</p>
      </section>
    </div>
  );
}
