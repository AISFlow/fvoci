// Source routes/w.$slug.my-tasks.tsx + features/tasks/my-tasks-view.tsx: open
// tasks assigned to the viewer across every project they can view.
import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { projectsQuery } from "@/features/projects/queries";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import { TaskListLoadMore } from "@/features/tasks/task-list-load-more";
import { groupTasksByProject, OPEN_ASSIGNED_QUERY } from "@/features/tasks/my-tasks";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { FALLBACK_TZ, formatDateKo } from "@/lib/datetime";
import { formatDisplayId, itemPath } from "@/lib/href";
import { meQuery } from "@/lib/queries";
import "@/features/projects/projects.css";

export function MyTasksPage() {
  const { slug, workspace } = useWorkspaceContext();
  const workspaceId = workspace?.id ?? "";
  const tasks = useInfiniteQuery({
    queryKey: ["workspace-tasks", workspaceId, OPEN_ASSIGNED_QUERY] as const,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks", {
          params: {
            path: { workspace_id: workspaceId },
            query: { query: OPEN_ASSIGNED_QUERY, ...(pageParam ? { cursor: pageParam } : {}) },
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
    enabled: Boolean(workspaceId),
    retry: false,
  });
  const statuses = useQuery({
    queryKey: ["workspace-statuses", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/statuses", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
  const projects = useQuery(projectsQuery(workspaceId));
  const me = useQuery(meQuery);

  if (!workspace) return null;

  const merged = mergeTaskListPages(tasks.data?.pages ?? []);
  const items = merged?.items ?? [];
  const projectById = new Map((projects.data?.items ?? []).map((p) => [p.id, p]));
  const statusById = new Map((statuses.data?.items ?? []).map((s) => [s.id, s]));
  const timeZone = me.data?.timezone ?? FALLBACK_TZ;
  /* A rejected later page after the list changed: restart from the top like the source. */
  const cursorStale =
    tasks.error instanceof ProblemError && tasks.error.status === 400 && items.length > 0;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="myTasks"
    >
      <div className="task-home" data-testid="my-tasks">
        <div className="task-home__head">
          <h1 className="task-home__title">{t("task.mine")}</h1>
        </div>
        {tasks.isLoading ? <QueryLoading /> : null}
        {tasks.isError && items.length === 0 ? (
          <QueryError
            message={loadErrorMessage(tasks.error)}
            onRetry={() => {
              void tasks.refetch();
            }}
          />
        ) : null}
        {tasks.isSuccess && items.length === 0 ? <EmptyState title={t("task.assigned.empty")} /> : null}
        <div className="flex flex-col gap-8">
          {groupTasksByProject(items).map(([projectId, projectItems]) => {
            const project = projectById.get(projectId);
            return (
              <section key={projectId} className="flex flex-col gap-1">
                <h2 className="border-b border-border pb-2 text-ui font-medium">
                  <span className="tabular-nums text-muted-foreground">{project?.key ?? "—"}</span>
                  {project?.name ? <span className="ml-2 break-keep">{project.name}</span> : null}
                </h2>
                <ul className="task-status-list">
                  {projectItems.map((item) => {
                    const displayId = project ? formatDisplayId(project.key, item.number) : null;
                    const status = statusById.get(item.statusId);
                    const due = item.dueDate ?? item.dueAt;
                    const row = (
                      <>
                        <span className="task-row__id">{displayId ?? item.id.slice(0, 8)}</span>
                        <span className="task-row__title">{item.title}</span>
                        {status ? <span className="project-list__private">{status.name}</span> : null}
                        {due ? (
                          <span className="project-list__private">{formatDateKo(due, timeZone)}</span>
                        ) : null}
                      </>
                    );
                    return (
                      <li key={item.id}>
                        {displayId ? (
                          <Link to={itemPath(slug, displayId)} className="task-row" data-testid={`my-task-${item.id}`}>
                            {row}
                          </Link>
                        ) : (
                          <span className="task-row">{row}</span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </section>
            );
          })}
        </div>
        {cursorStale ? (
          <p role="status" className="break-keep text-ui text-muted-foreground">
            {t("task.mine.cursorRestarted")}
          </p>
        ) : null}
        {tasks.hasNextPage || cursorStale ? (
          <TaskListLoadMore
            error={tasks.isFetchNextPageError && !cursorStale ? t("task.mine.loadMoreFailed") : null}
            pending={tasks.isFetchingNextPage}
            onLoadMore={() => {
              if (cursorStale) void tasks.refetch();
              else void tasks.fetchNextPage();
            }}
          />
        ) : null}
      </div>
    </WorkspaceShell>
  );
}
