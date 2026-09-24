import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { findProjectByKey, projectsQuery, workflowQuery } from "@/features/projects/queries";
import { TaskDetailView } from "@/features/tasks/task-detail";
import { lookupQuery, pickLookupTask } from "@/features/tasks/lookup";
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
  const item = parsed?.kind === "item" && parsed.prefix !== "WIKI" ? parsed : null;
  const displayId = item?.displayId ?? "";

  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, item?.prefix ?? "");
  const lookup = useQuery(lookupQuery(workspace?.id ?? "", displayId));
  const lookupTask = lookup.isSuccess ? pickLookupTask(lookup.data.items, displayId) : null;
  const task = useQuery(taskQuery(workspace?.id ?? "", lookupTask?.id ?? ""));
  const workflow = useQuery(
    workflowQuery(workspace?.id ?? "", project?.id ?? lookupTask?.projectId ?? ""),
  );

  if (!workspace) return null;

  const projectsDenied =
    projects.isError &&
    projects.error instanceof ProblemError &&
    projects.error.status === 404;
  const missingItem = item == null;
  const lookup404 =
    lookup.isError && lookup.error instanceof ProblemError && lookup.error.status === 404;
  const lookupMiss = lookup.isSuccess && lookupTask === null;
  const task404 =
    Boolean(lookupTask) &&
    task.isError &&
    task.error instanceof ProblemError &&
    task.error.status === 404;
  const realNotFound = missingItem || projectsDenied || lookup404 || lookupMiss || task404;

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
      {realNotFound ? (
        <p role="alert" className="task-form__alert">
          {t("task.error.notFound")}
        </p>
      ) : null}
      {lookup.isLoading ? <QueryLoading /> : null}
      {lookup.isError && !lookup404 ? (
        <QueryError
          message={loadErrorMessage(lookup.error)}
          onRetry={() => {
            void lookup.refetch();
          }}
        />
      ) : null}
      {lookupTask && task.isLoading ? <QueryLoading /> : null}
      {lookupTask && task.isError && !task404 ? (
        <QueryError
          message={loadErrorMessage(task.error)}
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
      {item && task.data ? (
        <TaskDetailView
          slug={slug}
          projectKey={project?.key ?? item.prefix}
          projectName={project?.name}
          task={task.data}
          statuses={workflow.data?.statuses ?? []}
        />
      ) : null}
      {realNotFound ? (
        <p className="task-home__note">
          <Link to={project ? projectTasksPath(slug, project.key) : projectsPath(slug)}>
            {t("nav.projects")}
          </Link>
        </p>
      ) : null}
    </WorkspaceShell>
  );
}
