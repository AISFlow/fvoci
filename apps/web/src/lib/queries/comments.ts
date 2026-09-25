import { infiniteQueryOptions } from "@tanstack/react-query";
import { api, ensureOk } from "@/lib/api";
import type { components } from "@/generated/api";

export type CommentOutput = components["schemas"]["CommentOutput"];

export function documentCommentsQuery(workspaceId: string, documentId: string) {
  return infiniteQueryOptions({
    queryKey: ["comments", workspaceId, documentId] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
            query: pageParam ? { cursor: pageParam } : {},
          },
        }),
      ),
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    enabled: Boolean(workspaceId) && Boolean(documentId),
  });
}
