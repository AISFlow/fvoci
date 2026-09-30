import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

/**
 * A workspace attachment's metadata (session route). No retry: a 403 or 404
 * means the attachment is not readable, and the page offers its own retry
 * for a failure that may pass.
 */
export function attachmentQuery(workspaceId: string, attachmentId: string) {
  return queryOptions({
    queryKey: ["attachment", workspaceId, attachmentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
          params: { path: { workspace_id: workspaceId, attachment_id: attachmentId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(attachmentId),
    retry: false,
  });
}

/** Source `hwpEditable`: whether this session may save an edited HWP/HWPX copy. */
export function attachmentEditContextQuery(
  workspaceId: string,
  attachmentId: string,
  enabled: boolean,
) {
  return queryOptions({
    queryKey: ["attachment-edit-context", workspaceId, attachmentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET(
          "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/edit-context",
          {
            params: { path: { workspace_id: workspaceId, attachment_id: attachmentId } },
          },
        ),
      ),
    enabled,
    retry: false,
  });
}
