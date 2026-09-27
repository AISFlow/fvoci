import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useParams, useSearchParams } from "react-router-dom";
import { loadErrorMessage } from "@/components/query-status";
import { AttachmentViewer } from "@/features/attachments/attachment-viewer";
import { chunkSearch } from "@/features/attachments/attachment-kind";
import { ProblemError } from "@/lib/api";
import {
  shareAttachmentDownloadUrl,
  shareAttachmentQuery,
} from "@/lib/queries/share-attachment";
import "@/features/attachments/attachment-shell.css";
import "@/features/share/share.css";

/**
 * Anonymous `/s/:token/attachments/:attachmentId/view`. Like `/s/:token` it needs no
 * session and only calls `/api/v1/share/{token}/…`. The share is view-only: no
 * preview-html, edit-context or edit-copy, and no workspace chrome.
 */
export function ShareAttachmentViewPage() {
  const { token = "", attachmentId = "" } = useParams<{
    token: string;
    attachmentId: string;
  }>();
  const [search] = useSearchParams();
  const { chunk } = chunkSearch(search);
  const query = useQuery(shareAttachmentQuery(token, attachmentId));
  const downloadUrl = shareAttachmentDownloadUrl(token, attachmentId);

  let content;
  if (query.error) {
    const notFound = query.error instanceof ProblemError && query.error.status === 404;
    const forbidden = query.error instanceof ProblemError && query.error.status === 403;
    const retryable =
      !notFound &&
      !forbidden &&
      (!(query.error instanceof ProblemError) || query.error.status >= 500);
    content = (
      <AttachmentViewer
        name=""
        mime=""
        image={false}
        downloadUrl={downloadUrl}
        error={notFound ? t("attachment.error.notFound") : loadErrorMessage(query.error)}
        {...(retryable ? { onMetadataRetry: () => void query.refetch() } : {})}
      />
    );
  } else if (!query.data) {
    content = (
      <p role="status" className="attachment-viewer__status">
        {t("attachment.preview.loading")}
      </p>
    );
  } else {
    content = (
      <AttachmentViewer
        name={query.data.name}
        mime={query.data.mime}
        image={query.data.image}
        downloadUrl={downloadUrl}
        {...(chunk === undefined ? {} : { chunk })}
      />
    );
  }

  return (
    <div className="share-page">
      <main className="share-page__frame share-page__frame--solo">
        <div className="share-page__main">{content}</div>
      </main>
    </div>
  );
}
