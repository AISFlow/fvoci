import { infiniteQueryOptions, queryOptions } from "@/lib/query-options";
import { api, ensureOk } from "@/lib/api";

export function unfurlQueryOptions(workspaceId: string | null, url: string) {
  return queryOptions({
    queryKey: ["unfurl", workspaceId, url] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/unfurl", {
          params: {
            path: { workspace_id: workspaceId ?? "" },
            query: { url },
          },
        }),
      ),
    enabled: workspaceId !== null,
  });
}

/** Source `workspaceEventsQueryOptions` (limit=50), paged by `nextCursor`. */
export function workspaceEventsQuery(workspaceId: string) {
  return infiniteQueryOptions({
    queryKey: ["workspace-events", workspaceId] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/events", {
          params: {
            path: { workspace_id: workspaceId },
            query: pageParam ? { limit: 50, cursor: pageParam } : { limit: 50 },
          },
        }),
      ),
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    retry: false,
  });
}
