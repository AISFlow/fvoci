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
  await Promise.all(
    queryClient
      .getQueryCache()
      .findAll({ queryKey })
      .map(async (query) => {
        const filters = { queryKey: query.queryKey, exact: true };
        const inFlight = queryClient.isFetching(filters) > 0;
        await queryClient.invalidateQueries(filters, { cancelRefetch: false });
        if (inFlight) await queryClient.invalidateQueries(filters, { cancelRefetch: false });
      }),
  );
}

/** Refresh only affected identity consumers. Search is still hydrated by Rust;
 * resync never promotes permissions from a cached row or exposes index totals. */
async function invalidateConnectedTaskCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  taskIds: readonly string[] = [],
  documentId?: string,
  recoverMissed = false,
): Promise<void> {
  const queries = queryClient.getQueryCache().findAll();
  const tasks = new Set(taskIds);
  const originTargets = new Set(taskIds);
  if (documentId) originTargets.add(documentId);
  let sourceKnown = !!documentId;
  for (const query of queries) {
    const key = query.queryKey;
    if (key[0] !== "task-origins" || key[1] !== workspaceId || typeof key[2] !== "string") continue;
    const data = query.state.data as
      { items?: Array<{ taskId?: string; documentId?: string }> } | undefined;
    for (const item of data?.items ?? []) {
      if (!item.taskId || !tasks.has(item.taskId)) continue;
      originTargets.add(key[2]);
      if (item.documentId) {
        sourceKnown = true;
        originTargets.add(item.documentId);
      }
    }
  }
  // Actual task SSE carries only verb/taskId. A new or unseen relation cannot
  // be ruled out by a page's absence. Recover this authorized origin family
  // when no permitted source association is cached, as on missed-hint resync.
  const recoverOrigins = recoverMissed || (tasks.size > 0 && !sourceKnown);
  if (recoverOrigins)
    for (const query of queries) {
      const key = query.queryKey;
      if (key[0] !== "task-origins" || key[1] !== workspaceId || typeof key[2] !== "string")
        continue;
      const knownTask =
        query.meta?.originTargetKind === "task"
          ? queryClient.getQueryData<TaskDetail>(["task", workspaceId, key[2]])
          : undefined;
      // Origin DTOs carry no project relation. Only a positively known task
      // target in a sibling project can be excluded, never an unseen source.
      // Its actual REST read remains the authority and can deny/return empty.
      if (!(knownTask?.id === key[2] && knownTask.projectId !== projectId))
        originTargets.add(key[2]);
    }
  const related = queries.filter((query) => {
    const key = query.queryKey;
    if (key[0] === "search" && key[1] === workspaceId)
      return (key[3] === "all" || key[3] === "task") && (!key[4] || key[4] === projectId);
    if (key[0] === "task-origins" && key[1] === workspaceId) {
      return typeof key[2] === "string" && originTargets.has(key[2]);
    }
    if (key[0] === "backlinks" && key[2] === workspaceId) {
      if (key[1] === "task" && typeof key[3] === "string" && tasks.has(key[3])) return true;
      const data = query.state.data as { items?: Array<{ id?: string }> } | undefined;
      return data?.items?.some((item) => !!item.id && tasks.has(item.id)) ?? false;
    }
    return false;
  });
  await Promise.all([
    invalidateKeepingLoadMore(queryClient, ["workspace-tasks", workspaceId]),
    ...related.map((query) => invalidateKeepingLoadMore(queryClient, query.queryKey)),
  ]);
}

export async function invalidateTaskCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  taskId: string,
  documentId?: string,
): Promise<void> {
  await Promise.all([
    invalidateConnectedTaskCaches(queryClient, workspaceId, projectId, [taskId], documentId),
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
  // committed row and keep known sibling projects/workspaces untouched. A
  // mounted detail whose first GET failed has no row to identify its project;
  // retry it within this workspace without evicting inactive empty queries.
  const details = queryClient
    .getQueryCache()
    .findAll({ queryKey: ["task", workspaceId] })
    .filter(
      (query) =>
        queryClient.getQueryData<TaskDetail>(query.queryKey)?.projectId === projectId ||
        (query.state.data === undefined && query.state.status === "error" && query.isActive()),
    );
  Promise.all([
    invalidateConnectedTaskCaches(
      queryClient,
      workspaceId,
      projectId,
      details.map((query) => String(query.queryKey[2])),
      undefined,
      true,
    ),
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
