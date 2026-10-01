import type { QueryClient, QueryKey } from "@tanstack/query-core";
import type { TaskDetail } from "./queries";

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
    queryClient.invalidateQueries({ queryKey: ["task-time-entries", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["collection-item", workspaceId, "task", taskId] }),
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
  // The server's authorized open recovers missed hints, including a mounted
  // detail. Its key has no project ID, so select retained details by their
  // committed row and keep sibling projects/workspaces untouched.
  const details = queryClient
    .getQueryCache()
    .findAll({ queryKey: ["task", workspaceId] })
    .filter(
      (query) => queryClient.getQueryData<TaskDetail>(query.queryKey)?.projectId === projectId,
    );
  Promise.all([
    ...details.flatMap((query) => [
      queryClient.invalidateQueries({ queryKey: query.queryKey, exact: true }),
      queryClient.invalidateQueries({
        queryKey: ["task-activity", workspaceId, query.queryKey[2]],
      }),
      queryClient.invalidateQueries({
        queryKey: ["task-time-entries", workspaceId, query.queryKey[2]],
      }),
      queryClient.invalidateQueries({
        queryKey: ["collection-item", workspaceId, "task", query.queryKey[2]],
      }),
    ]),
    queryClient.invalidateQueries({ queryKey: ["task-layout", workspaceId, projectId] }),
    invalidateKeepingLoadMore(queryClient, ["tasks", workspaceId, projectId]),
    queryClient.invalidateQueries({ queryKey: ["project-collection", workspaceId, projectId] }),
    invalidateKeepingLoadMore(queryClient, ["collection", workspaceId]),
    queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
  ]).catch((error: unknown) => {
    // Stream callbacks cannot await resync; retain the cache's error state and report failure.
    console.error("task stream cache resync failed", error);
  });
}
