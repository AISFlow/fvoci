import { queryOptions } from "@tanstack/react-query";
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
