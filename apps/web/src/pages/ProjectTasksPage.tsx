import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import {
  backlogStatusId,
  findProjectByKey,
  projectsQuery,
  workflowQuery,
} from "@/features/projects/queries";
import { TaskCreateDialog } from "@/features/tasks/task-create-dialog";
import { TaskList } from "@/features/tasks/task-list";
import { taskListQuery, type CreateTaskBody } from "@/features/tasks/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { formatDisplayId, itemPath, parseRef, projectsPath } from "@/lib/href";
import "@/features/projects/projects.css";

export function ProjectTasksPage() {
  const { ref } = useParams<{ ref: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");
  const projectKey = parsed?.kind === "project" ? parsed.key : null;
  const [createStatusId, setCreateStatusId] = useState<string | null>(null);

  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, projectKey ?? "");
  const workflow = useQuery(workflowQuery(workspace?.id ?? "", project?.id ?? ""));
  const tasks = useQuery(taskListQuery(workspace?.id ?? "", project?.id ?? ""));

  const createTask = useMutation({
    mutationFn: async (body: CreateTaskBody) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
          params: {
            path: { workspace_id: workspace!.id, project_id: project!.id },
          },
          body,
        }),
      ),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["tasks", workspace?.id, project?.id] }),
        queryClient.invalidateQueries({ queryKey: ["projects", workspace?.id] }),
      ]);
    },
  });

  if (!workspace) return null;

  const notFound =
    parsed === null ||
    parsed.kind !== "project" ||
    (projects.isSuccess && project === undefined) ||
    (projects.isError &&
      projects.error instanceof ProblemError &&
      projects.error.status === 404);

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {projects.isLoading ? <QueryLoading /> : null}
      {projects.isError && !notFound ? (
        <QueryError
          message={loadErrorMessage(projects.error)}
          onRetry={() => {
            void projects.refetch();
          }}
        />
      ) : null}
      {notFound ? (
        <p role="alert" className="task-form__alert">
          {t("project.notFound")}
        </p>
      ) : null}
      {!notFound && project ? (
        <div className="task-home">
          <p className="task-home__crumb">
            <Link to={projectsPath(slug)}>{t("nav.projects")}</Link>
            <span aria-hidden="true"> / </span>
            <span>{project.key}</span>
          </p>
          <div className="task-home__head">
            <h1 className="task-home__title">{project.name}</h1>
          </div>
          {workflow.isLoading || tasks.isLoading ? <QueryLoading /> : null}
          {workflow.isError || tasks.isError ? (
            <QueryError
              message={loadErrorMessage(workflow.error ?? tasks.error)}
              onRetry={() => {
                if (workflow.isError) void workflow.refetch();
                if (tasks.isError) void tasks.refetch();
              }}
            />
          ) : null}
          {!workflow.isLoading && !tasks.isLoading && !workflow.isError && !tasks.isError ? (
            <TaskList
              slug={slug}
              projectKey={project.key}
              items={tasks.data?.items ?? []}
              statuses={workflow.data?.statuses ?? []}
              canCreate={project.status === "active"}
              defaultStatusId={backlogStatusId(workflow.data?.statuses ?? [])}
              onCreateClick={(statusId) => {
                createTask.reset();
                setCreateStatusId(statusId);
              }}
            />
          ) : null}
          <TaskCreateDialog
            open={createStatusId !== null}
            projectKey={project.key}
            pending={createTask.isPending}
            error={
              createTask.isError
                ? createTask.error instanceof ProblemError
                  ? problemMessage(createTask.error, "task.create.failed")
                  : t("error.network")
                : null
            }
            onOpenChange={(open) => {
              if (!open) {
                setCreateStatusId(null);
                createTask.reset();
              }
            }}
            onSubmit={async (values) => {
              if (!createStatusId) return;
              try {
                const created = await createTask.mutateAsync({
                  title: values.title,
                  type: values.type,
                  statusId: createStatusId,
                });
                setCreateStatusId(null);
                await navigate(itemPath(slug, formatDisplayId(project.key, created.number)));
              } catch {
                // Keep the dialog open; mutation error is shown in the form.
              }
            }}
          />
        </div>
      ) : null}
    </WorkspaceShell>
  );
}
