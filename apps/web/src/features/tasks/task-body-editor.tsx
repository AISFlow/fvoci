import { FvociEditor } from "@fvoci/editor/fvoci-editor";
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { collabBadge, collabRefusalNote } from "@/features/documents/collab-badge";
import { CollabPresence } from "@/features/documents/collab-presence";
import type { CollabSession, CollabUser } from "@/features/documents/collab-session";
import { RevisionPanel } from "@/features/documents/revision-panel";
import { UrlEmbedProvider } from "@/features/workspace/document-editor";
import "@/features/documents/document-shell.css";

/* Source task-detail-screen: the task body is the `${ws}:task:${id}` collab room.
 * Parent owns the single `useCollabSession` (TaskDetailView). */
export function TaskBodyEditor({
  workspaceId,
  slug,
  taskId,
  readOnly: pageReadOnly,
  session,
  collabUser,
}: {
  workspaceId: string;
  slug: string;
  taskId: string;
  readOnly: boolean;
  session: CollabSession | null;
  collabUser: CollabUser | null;
}) {
  const [persisting, setPersisting] = useState(false);
  const [persistError, setPersistError] = useState<string | null>(null);

  const readOnly = pageReadOnly || (session?.readOnly ?? false);
  const ready = Boolean(session?.synced && collabUser);
  const refusalNote = collabRefusalNote(session?.status, ready);
  const badge = session
    ? collabBadge(session.status, session.pending || persisting, session.durableSaved)
    : null;
  const canPersist =
    ready && !readOnly && session !== null && session.status === "connected" && !persisting;

  async function persistBody() {
    if (!session || !canPersist) return;
    setPersistError(null);
    setPersisting(true);
    try {
      await session.persistNow();
    } catch (error) {
      const timedOut = error instanceof Error && error.message.includes("timed out");
      setPersistError(timedOut ? t("collab timeout — retry") : t("collab unavailable"));
      throw error;
    } finally {
      setPersisting(false);
    }
  }

  return (
    <section
      className="task-detail__body"
      aria-label={t("doc.body.a11y")}
      data-testid="task-body"
    >
      <div className="document-page__collab">
        {badge ? (
          <span
            className={`document-page__collab-status document-page__collab-status--${badge.tone}`}
            data-collab-status={session?.status}
            data-collab-pending={session?.pending ? "true" : "false"}
            data-collab-persisted={session?.durableSaved ? "true" : "false"}
          >
            {t(badge.label)}
          </span>
        ) : (
          <span
            className="document-page__collab-status document-page__collab-status--wait"
            data-collab-persisted="false"
          >
            {t("doc.collab.connecting")}
          </span>
        )}
        <Button type="button" size="sm" disabled={!canPersist} onClick={() => void persistBody()}>
          {persisting ? t("doc.title.saving") : t("doc.title.save")}
        </Button>
        {session ? <CollabPresence peers={session.peers} /> : null}
        <RevisionPanel
          workspaceId={workspaceId}
          targetKind="task"
          documentId={taskId}
          readOnly={readOnly}
          persistNow={canPersist ? persistBody : undefined}
        />
      </div>
      {persistError ? (
        <p role="alert" className="document-page__error">
          {persistError}
        </p>
      ) : null}
      {session?.status === "unauthorized" ? (
        <p className="document-page__body-note" role="alert">
          {t("task.collab.unauthorized")}
        </p>
      ) : null}
      {refusalNote ? (
        <p className="document-page__body-note" role="status">
          {t(refusalNote)}
        </p>
      ) : null}
      {!ready && session?.status !== "unauthorized" && !refusalNote ? <QueryLoading /> : null}
      {ready && session && collabUser ? (
        <div className="document-page__body document-page__body--editor">
          <UrlEmbedProvider workspaceId={workspaceId}>
            <FvociEditor
              ydoc={session.doc}
              provider={session.provider}
              user={collabUser}
              editable={!readOnly}
              ariaLabel={t("doc.body.a11y")}
              workspaceSlug={slug}
              gutterAddLabel={t("editor.gutter.add")}
              gutterMoveLabel={t("editor.gutter.move")}
              insertLabel={t("editor.mobile.insert")}
            />
          </UrlEmbedProvider>
        </div>
      ) : null}
    </section>
  );
}
