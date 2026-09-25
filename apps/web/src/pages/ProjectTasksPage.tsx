import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { ProjectGroupsSection } from "@/features/projects/project-groups";
import { ProjectMilestonesSection } from "@/features/projects/project-milestones";
import {
  backlogStatusId,
  findProjectByKey,
  projectsQuery,
  workflowQuery,
} from "@/features/projects/queries";
import { TaskCreateDialog } from "@/features/tasks/task-create-dialog";
import { ProjectViewNav } from "@/features/collections/project-view-nav";
import { TaskFilters } from "@/features/tasks/task-filters";
import { TaskList } from "@/features/tasks/task-list";
import { ProjectTaskSavedViews, viewConfigOf } from "@/features/tasks/task-saved-views";
import {
  projectLabelsQuery,
  projectMilestonesQuery,
  taskListQuery,
  type CreateTaskBody,
} from "@/features/tasks/queries";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { FALLBACK_TZ } from "@/lib/datetime";
import { formatDisplayId, itemPath, parseRef, projectPath, projectsPath } from "@/lib/href";
import { membersQuery, meQuery } from "@/lib/queries";
import { collectionFieldsQuery, projectCollectionQuery } from "@/lib/queries/collections";
import { encodeViewQueryParam, parseViewQueryParam, type ViewQuery } from "@/lib/view-query";
import "@/features/projects/projects.css";

function roleAtLeast(role: string, minimum: string): boolean {
  const order = ["guest", "member", "admin", "owner"];
  return order.indexOf(role) >= order.indexOf(minimum);
}

export function ProjectTasksPage() {
  const { ref } = useParams<{ ref: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");
  const projectKey = parsed?.kind === "project" ? parsed.key : null;
  const [createStatusId, setCreateStatusId] = useState<string | null>(null);
  const [search, setSearch] = useSearchParams();
  const rawQuery = search.get("query");
  const parsedQuery = parseViewQueryParam(rawQuery);
  const viewQuery: ViewQuery = parsedQuery ?? { filters: {}, sort: [] };
  const encodedQuery = encodeViewQueryParam(viewQuery);
  const selectedViewId = search.get("view");

  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, projectKey ?? "");
  const workflow = useQuery(workflowQuery(workspace?.id ?? "", project?.id ?? ""));
  const tasks = useInfiniteQuery(
    taskListQuery(workspace?.id ?? "", project?.id ?? "", encodedQuery),
  );
  const me = useQuery(meQuery);
  const members = useQuery(membersQuery(workspace?.id ?? ""));
  const labels = useQuery(projectLabelsQuery(workspace?.id ?? "", project?.id ?? ""));
  const milestones = useQuery(projectMilestonesQuery(workspace?.id ?? "", project?.id ?? ""));
  const collection = useQuery(projectCollectionQuery(workspace?.id ?? "", project?.id ?? ""));
  const fields = useQuery(collectionFieldsQuery(workspace?.id ?? "", collection.data?.id ?? ""));

  function applyQuery(next: ViewQuery, viewId: string | null | undefined = selectedViewId) {
    const params = new URLSearchParams(search);
    const encoded = encodeViewQueryParam(next);
    if (encoded) params.set("query", encoded);
    else params.delete("query");
    if (viewId) params.set("view", viewId);
    else params.delete("view");
    setSearch(params, { replace: true });
  }
  const taskPages = mergeTaskListPages(tasks.data?.pages ?? []);
  const firstPageFailed = tasks.isError && !tasks.isFetchNextPageError;

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
            <Link to={projectPath(slug, project.key)}>{project.key}</Link>
          </p>
          <div className="task-home__head">
            <h1 className="task-home__title">{project.name}</h1>
          </div>
          <ProjectViewNav slug={slug} projectKey={project.key} active="tasks" />
          <ProjectTaskSavedViews
            workspaceId={workspace.id}
            projectId={project.id}
            query={viewQuery}
            selectedId={selectedViewId}
            onSelect={(view) =>
              view ? applyQuery(viewConfigOf(view), view.id) : applyQuery(viewQuery, null)
            }
          />
          <TaskFilters
            query={viewQuery}
            statuses={workflow.data?.statuses ?? []}
            labels={labels.data?.items ?? []}
            milestones={milestones.data?.items ?? []}
            members={members.data?.items ?? []}
            fields={fields.data?.items ?? []}
            timeZone={me.data?.timezone ?? FALLBACK_TZ}
            onChange={(next) => applyQuery(next)}
          />
          {parsedQuery === null ? (
            <p role="alert" className="task-form__alert">
              {t("task.filter.lastValidResults")}
            </p>
          ) : null}
          {workflow.isLoading || tasks.isLoading ? <QueryLoading /> : null}
          {workflow.isError || firstPageFailed ? (
            <QueryError
              message={loadErrorMessage(workflow.error ?? tasks.error)}
              onRetry={() => {
                if (workflow.isError) void workflow.refetch();
                if (firstPageFailed) void tasks.refetch();
              }}
            />
          ) : null}
          {!workflow.isLoading && !tasks.isLoading && !workflow.isError && !firstPageFailed && taskPages ? (
            <TaskList
              slug={slug}
              projectKey={project.key}
              items={taskPages.items}
              statusCounts={taskPages.statusCounts}
              statuses={workflow.data?.statuses ?? []}
              canCreate={project.status === "active"}
              defaultStatusId={backlogStatusId(workflow.data?.statuses ?? [])}
              hasMore={tasks.hasNextPage}
              loadMorePending={tasks.isFetchingNextPage}
              loadMoreError={
                tasks.isFetchNextPageError ? loadErrorMessage(tasks.error) : null
              }
              onCreateClick={(statusId) => {
                createTask.reset();
                setCreateStatusId(statusId);
              }}
              onLoadMore={() => {
                void tasks.fetchNextPage();
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
          {workspace ? (
            <ProjectMilestonesSection
              workspaceId={workspace.id}
              projectId={project.id}
              canManage={project.status === "active" && project.canEdit}
            />
          ) : null}
          {workspace ? (
            <ProjectGroupsSection
              workspaceId={workspace.id}
              projectId={project.id}
              canManage={roleAtLeast(workspace.role, "admin")}
            />
          ) : null}
        </div>
      ) : null}
    </WorkspaceShell>
  );
}
