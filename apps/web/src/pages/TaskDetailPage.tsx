import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { findProjectByKey, projectsQuery, workflowQuery } from "@/features/projects/queries";
import { TaskDetailView } from "@/features/tasks/task-detail";
import { taskQuery } from "@/features/tasks/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { ProblemError } from "@/lib/api";
import { parseRef, projectsPath, projectTasksPath } from "@/lib/href";
import "@/features/projects/projects.css";

export function TaskDetailPage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");
  const item = parsed?.kind === "item" ? parsed : null;

  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, item?.prefix ?? "");
  const lookupTaskId: string | undefined = undefined;
  const task = useQuery(taskQuery(workspace?.id ?? "", lookupTaskId ?? ""));
  const workflow = useQuery(workflowQuery(workspace?.id ?? "", project?.id ?? ""));

  if (!workspace) return null;

  const projectsDenied =
    projects.isError &&
    projects.error instanceof ProblemError &&
    projects.error.status === 404;
  const missingItem = item == null || item.prefix === "WIKI";
  const missingProject = projects.isSuccess && project === undefined;
  const missingLookup = lookupTaskId === undefined;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {projects.isLoading ? <QueryLoading /> : null}
      {projects.isError && !projectsDenied ? (
        <QueryError
          message={loadErrorMessage(projects.error)}
          onRetry={() => {
            void projects.refetch();
          }}
        />
      ) : null}
      {projectsDenied || missingItem || missingProject || missingLookup ? (
        <p role="alert" className="task-form__alert">
          {t("task.error.notFound")}
        </p>
      ) : null}
      {lookupTaskId && task.isLoading ? <QueryLoading /> : null}
      {lookupTaskId && task.isError ? (
        <QueryError
          message={
            task.error instanceof ProblemError && task.error.status === 404
              ? t("task.error.notFound")
              : loadErrorMessage(task.error)
          }
          onRetry={() => {
            void task.refetch();
          }}
        />
      ) : null}
      {workflow.isError ? (
        <QueryError
          message={loadErrorMessage(workflow.error)}
          onRetry={() => {
            void workflow.refetch();
          }}
        />
      ) : null}
      {project && task.data ? (
        <TaskDetailView
          slug={slug}
          projectKey={project.key}
          projectName={project.name}
          task={task.data}
          statuses={workflow.data?.statuses ?? []}
        />
      ) : null}
      {missingItem || missingProject || missingLookup || projectsDenied ? (
        <p className="task-home__note">
          <Link to={project ? projectTasksPath(slug, project.key) : projectsPath(slug)}>
            {t("nav.projects")}
          </Link>
        </p>
      ) : null}
    </WorkspaceShell>
  );
}
