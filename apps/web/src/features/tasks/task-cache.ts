import type { QueryClient, QueryKey } from "@tanstack/query-core";

/**
 * Refetch a paged list (the project task list, the collection board columns)
 * without cancelling an in-flight "load more". The default `cancelRefetch` would
 * silently drop the requested page and refetch only the pages already loaded, so
 * the click does nothing. Instead, let the running fetch finish, then refetch
 * every loaded page so the hint still applies.
 */
async function invalidateKeepingLoadMore(
  queryClient: QueryClient,
  queryKey: QueryKey,
): Promise<void> {
  const filters = { queryKey };
  const inFlight = queryClient.isFetching(filters) > 0;
  await queryClient.invalidateQueries(filters, { cancelRefetch: false });
  // A fetch that was running joined the call above and may predate the hint.
  if (inFlight) await queryClient.invalidateQueries(filters, { cancelRefetch: false });
}

export async function invalidateTaskCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  taskId: string,
): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["task", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["task-activity", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["task-layout", workspaceId, projectId] }),
    invalidateKeepingLoadMore(queryClient, ["tasks", workspaceId, projectId]),
    queryClient.invalidateQueries({ queryKey: ["project-collection", workspaceId, projectId] }),
    invalidateKeepingLoadMore(queryClient, ["collection", workspaceId]),
    queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
  ]);
}

export function invalidateTaskStreamResyncCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
): void {
  void queryClient.invalidateQueries({ queryKey: ["task-layout", workspaceId, projectId] });
  void invalidateKeepingLoadMore(queryClient, ["tasks", workspaceId, projectId]);
  void queryClient.invalidateQueries({ queryKey: ["project-collection", workspaceId, projectId] });
  void invalidateKeepingLoadMore(queryClient, ["collection", workspaceId]);
  void queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] });
}
