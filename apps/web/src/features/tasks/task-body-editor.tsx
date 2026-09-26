import { FvociEditor } from "@fvoci/editor/fvoci-editor";
import { formatPersonName, t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { collabBadge } from "@/features/documents/collab-badge";
import { CollabPresence } from "@/features/documents/collab-presence";
import { CollabRoom, collabUserOf, useCollabSession } from "@/features/documents/collab-session";
import { RevisionPanel } from "@/features/documents/revision-panel";
import { meQuery } from "@/lib/queries";
import "@/features/documents/document-shell.css";

/* Source task-detail-screen: the task body is the `${ws}:task:${id}` collab room.
 * There is no body GET/PUT; the editor, block patch and revision restore write it. */
export function TaskBodyEditor({
  workspaceId,
  slug,
  taskId,
  readOnly,
}: {
  workspaceId: string;
  slug: string;
  taskId: string;
  readOnly: boolean;
}) {
  return (
    <CollabRoom workspaceId={workspaceId} kind="task" id={taskId}>
      <TaskBodyConnected
        workspaceId={workspaceId}
        slug={slug}
        taskId={taskId}
        readOnly={readOnly}
      />
    </CollabRoom>
  );
}

function TaskBodyConnected({
  workspaceId,
  slug,
  taskId,
  readOnly: pageReadOnly,
}: {
  workspaceId: string;
  slug: string;
  taskId: string;
  readOnly: boolean;
}) {
  const me = useQuery(meQuery);
  const collabUser = useMemo(() => {
    if (!me.data) return null;
    return collabUserOf(me.data.userId, formatPersonName(me.data, me.data.locale));
  }, [me.data]);
  const session = useCollabSession(collabUser);
  const [persisting, setPersisting] = useState(false);
  const [persistError, setPersistError] = useState<string | null>(null);

  const readOnly = pageReadOnly || (session?.readOnly ?? false);
  const ready = Boolean(session?.synced && collabUser);
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
      {!ready && session?.status !== "unauthorized" ? <QueryLoading /> : null}
      {ready && session && collabUser ? (
        <div className="document-page__body document-page__body--editor">
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
        </div>
      ) : null}
    </section>
  );
}
