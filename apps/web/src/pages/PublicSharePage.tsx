import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useParams } from "react-router-dom";
import {
  PublicShareGate,
  PublicShareLoading,
  PublicShareView,
} from "@/features/share/public-share";
import { ProblemError } from "@/lib/api";
import {
  sharePublicBodyQuery,
  sharePublicMetaQuery,
  sharePublicTreeQuery,
} from "@/lib/queries/share";

function failMessage(err: unknown): string {
  if (err instanceof ProblemError && err.status === 404) return t("share.expired");
  if (err instanceof ProblemError && err.titleKnown) return err.title;
  if (err instanceof ProblemError) return t("error.share.failed");
  return t("error.network");
}

/**
 * Anonymous `/s/:token` reader. It never needs a session and only calls
 * `/api/v1/share/{token}/…` (no `/workspaces`, `/auth/me` or setup guard).
 */
export function PublicSharePage() {
  const { token = "" } = useParams<{ token: string }>();
  const [selectedDocumentId, setSelectedDocumentId] = useState<string | null>(null);
  const meta = useQuery(sharePublicMetaQuery(token));
  const tree = useQuery(sharePublicTreeQuery(token, meta.isSuccess));
  const body = useQuery(sharePublicBodyQuery(token, selectedDocumentId, meta.isSuccess));

  if (meta.error) {
    return <PublicShareGate message={failMessage(meta.error)} />;
  }
  const data = meta.data;
  if (!data) {
    return <PublicShareLoading />;
  }

  return (
    <PublicShareView
      title={data.title}
      expiresAt={data.expiresAt}
      tree={tree.data?.items ?? []}
      activeDocumentId={selectedDocumentId ?? data.documentId}
      onSelectDocument={(documentId) => {
        setSelectedDocumentId(documentId === data.documentId ? null : documentId);
      }}
      body={body.data ?? null}
      bodyLoading={body.isLoading}
      bodyError={body.error ? failMessage(body.error) : null}
      onRetryBody={() => {
        void body.refetch();
      }}
    />
  );
}
