import { useQuery } from "@tanstack/react-query";
import { useParams, useSearchParams } from "react-router-dom";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { t } from "@fvoci/i18n";
import { AttachmentViewer } from "@/features/attachments/attachment-viewer";
import {
  attachmentDownloadUrl,
  chunkSearch,
} from "@/features/attachments/attachment-kind";
import "@/features/attachments/attachment-shell.css";

export function AttachmentViewPage() {
  const { attachmentId } = useParams<{ attachmentId: string }>();
  const [search] = useSearchParams();
  const { slug, workspace } = useWorkspaceContext();
  const { chunk } = chunkSearch(search);
  const id = attachmentId ?? "";
  const workspaceId = workspace?.id ?? "";
  const query = useQuery({
    queryKey: ["attachment", workspaceId, id],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
          params: { path: { workspace_id: workspaceId, attachment_id: id } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(id),
    retry: false,
  });

  if (!workspace) return null;

  const downloadUrl = attachmentDownloadUrl(workspace.id, id);
  const notFound = query.error instanceof ProblemError && (query.error.status === 404 || query.error.status === 403);
  const retryable =
    query.isError &&
    !notFound &&
    (!(query.error instanceof ProblemError) || query.error.status >= 500);

  if (query.isLoading && !query.data) {
    return (
      <WorkspaceShell
        slug={slug}
        workspaceId={workspace.id}
        workspaceName={workspace.name}
        activeNav="wiki"
      >
        <p className="attachment-viewer__status">{t("attachment.preview.loading")}</p>
      </WorkspaceShell>
    );
  }

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="wiki"
    >
      {query.isError ? (
        <AttachmentViewer
          name=""
          mime=""
          image={false}
          downloadUrl={downloadUrl}
          error={notFound ? t("attachment.error.notFound") : t("load.failed")}
          {...(retryable ? { onMetadataRetry: () => void query.refetch() } : {})}
        />
      ) : null}
      {query.data ? (
        <AttachmentViewer
          name={query.data.name}
          mime={query.data.mime}
          image={query.data.image}
          downloadUrl={downloadUrl}
          {...(chunk === undefined ? {} : { chunk })}
        />
      ) : null}
    </WorkspaceShell>
  );
}
