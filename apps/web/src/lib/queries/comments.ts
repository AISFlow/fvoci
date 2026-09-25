import { infiniteQueryOptions } from "@tanstack/react-query";
import { api, ensureOk } from "@/lib/api";
import type { components } from "@/generated/api";

export type CommentOutput = components["schemas"]["CommentOutput"];
export type CommentsTargetKind = "document" | "task";

export function commentsQuery(
  workspaceId: string,
  kind: CommentsTargetKind,
  targetId: string,
) {
  return infiniteQueryOptions({
    queryKey: ["comments", workspaceId, kind, targetId] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        kind === "document"
          ? await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments", {
              params: {
                path: { workspace_id: workspaceId, document_id: targetId },
                query: pageParam ? { cursor: pageParam } : {},
              },
            })
          : await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments", {
              params: {
                path: { workspace_id: workspaceId, task_id: targetId },
                query: pageParam ? { cursor: pageParam } : {},
              },
            }),
      ),
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    enabled: Boolean(workspaceId) && Boolean(targetId),
  });
}
