import { queryOptions } from "@tanstack/react-query";
import { api, ensureOk } from "@/lib/api";

/**
 * Anonymous share attachment reads. Only `/api/v1/share/{token}/…` is used, never the
 * session `/workspaces/…/attachments` routes, so a share viewer cannot reach bytes the
 * share does not cover.
 */
function sharePath(token: string, attachmentId: string): string {
  return `/api/v1/share/${encodeURIComponent(token)}/attachments/${encodeURIComponent(attachmentId)}`;
}

/** Source `routes.share.attachmentDownload`: original bytes behind the share token. */
export function shareAttachmentDownloadUrl(token: string, attachmentId: string): string {
  return `${sharePath(token, attachmentId)}/download`;
}

/** Source share view query: no retry, a 404 covers revoked, expired and out-of-share alike. */
export function shareAttachmentQuery(token: string, attachmentId: string) {
  return queryOptions({
    queryKey: ["share-attachment", token, attachmentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/share/{token}/attachments/{attachment_id}", {
          params: { path: { token, attachment_id: attachmentId } },
        }),
      ),
    enabled: Boolean(token) && Boolean(attachmentId),
    retry: false,
  });
}
