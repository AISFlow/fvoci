// Source features/tasks/my-tasks-view.tsx grouping and routes/w.$slug.my-tasks.tsx query.
import { api, ensureOk } from "@/lib/api";
import { infiniteQueryOptions, queryOptions } from "@/lib/query-options";

/** Source `OPEN_ASSIGNED_QUERY`: open tasks assigned to the viewer, due soonest first. */
export const OPEN_ASSIGNED_QUERY = JSON.stringify({
  filters: { assigneeId: "me", openOnly: true },
  sort: [{ field: "due", direction: "asc" }],
});

/** The viewer's open assigned tasks across the workspace, a page per cursor. */
export function myTasksQuery(workspaceId: string) {
  return infiniteQueryOptions({
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
}

/** A separate, bounded home preview; never share a single-page key with the infinite list. */
export function assignedTasksPreviewQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspace-tasks", workspaceId, OPEN_ASSIGNED_QUERY, "preview", 8] as const,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks", {
          signal,
          params: {
            path: { workspace_id: workspaceId },
            query: { query: OPEN_ASSIGNED_QUERY, limit: 8 },
          },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function workspaceLabelsQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspace-labels", workspaceId] as const,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/labels", {
          signal,
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

/** Every workflow status in the workspace, for the status names of cross-project rows. */
export function workspaceStatusesQuery(workspaceId: string) {
  return queryOptions({
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
}

/** Groups in first-seen order so the server's due-date order holds across projects. */
export function groupTasksByProject<T extends { projectId: string }>(
  items: readonly T[],
): Array<[string, T[]]> {
  const grouped = new Map<string, T[]>();
  for (const item of items) {
    const bucket = grouped.get(item.projectId) ?? [];
    bucket.push(item);
    grouped.set(item.projectId, bucket);
  }
  return [...grouped.entries()];
}
