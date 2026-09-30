import { FvociEditor, type TiptapEditor } from "@fvoci/editor/fvoci-editor";
import { AttachmentBlockContext } from "@fvoci/editor/react";
import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { UrlEmbedProvider } from "@/features/workspace/document-editor";
import { documentPath, formatDisplayId, projectPath, trashPath } from "@/lib/href";
import { ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { projectDocumentMetaQuery } from "@/lib/queries/documents";
import { projectAncestors } from "./project-ancestors";
import { projectDocumentsQuery } from "@/features/projects/queries";
import { CommentPanel } from "@/features/comments/comment-panel";
import { OriginPanel } from "@/features/collections/origin-panel";
import { createProjectDocumentAttachmentBridge } from "@/features/workspace/attachment-upload";
import { bindBlockPresence, isBlockPresenceAwareness } from "./block-presence";
import { collabBadge, collabRefusalNote } from "./collab-badge";
import { CollabPresence } from "./collab-presence";
import { collabUserOf, setTitleEditing, useCollabSession } from "./collab-session";
import { RevisionPanel } from "./revision-panel";
import { DocumentAiMenu } from "./document-ai-menu";
import { DocumentExportMenu } from "./document-export-menu";
import { ShareDialog } from "@/features/share/share-dialog";
import { StarToggle } from "@/features/share/star-toggle";
import { DocumentTagsBar } from "./document-tags-bar";
import {
  type DocumentScope,
  moveDocument,
  type PatchDocumentBody,
  patchDocument,
  trashDocument,
} from "./document-api";
import "./document-shell.css";

const STATUSES = ["draft", "published", "archived"] as const;
const TITLE_MAX = 300;
const ICON_MAX = 50;

function flashBlock(id: string): (() => void) | undefined {
  const el = document.querySelector<HTMLElement>(`.fvoci-editor [data-id="${CSS.escape(id)}"]`);
  if (!el) return undefined;
  el.setAttribute("data-afn-flash", "");
  const timer = window.setTimeout(() => {
    el.removeAttribute("data-afn-flash");
  }, 800);
  return () => {
    window.clearTimeout(timer);
    el.removeAttribute("data-afn-flash");
  };
}

/** The project document's page context (wiki documents are the Vue app's page). */
export interface ProjectDocumentContext {
  id: string;
  key: string;
  rootDocumentId: string | null;
  canEdit: boolean;
  archived: boolean;
}

interface DocumentViewProps {
  workspaceId: string;
  slug: string;
  documentId: string;
  project: ProjectDocumentContext;
}

export function DocumentView({ workspaceId, slug, documentId, project }: DocumentViewProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const me = useQuery(meQuery);
  const projectId = project.id;
  const metaQuery = useQuery(projectDocumentMetaQuery(workspaceId, projectId, documentId));
  const tree = useQuery(projectDocumentsQuery(workspaceId, projectId));
  const metaKey = ["project-document", workspaceId, projectId, documentId];
  const treeKey = ["project-documents", workspaceId, projectId];

  const [title, setTitle] = useState("");
  const [icon, setIcon] = useState("");
  const [status, setStatus] = useState<string>("draft");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [persistError, setPersistError] = useState<string | null>(null);
  const [persisting, setPersisting] = useState(false);
  const [editor, setEditor] = useState<TiptapEditor | null>(null);
  const [moveParentId, setMoveParentId] = useState("");
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);

  useEffect(() => {
    if (!metaQuery.data) return;
    setTitle(metaQuery.data.title);
    setIcon(metaQuery.data.icon ?? "");
    setStatus(metaQuery.data.status);
  }, [metaQuery.data]);

  // Project documents upload through the project route (affiliation checked by the server).
  const attachmentBridge = useMemo(
    () => createProjectDocumentAttachmentBridge(workspaceId, projectId, documentId),
    [workspaceId, documentId, projectId],
  );
  const collabUser = useMemo(() => {
    if (!me.data) return null;
    return collabUserOf(me.data.userId, formatPersonName(me.data, me.data.locale));
  }, [me.data]);
  const collabSession = useCollabSession(collabUser);

  useEffect(() => {
    const awareness = collabSession?.provider.awareness;
    if (!editor || !isBlockPresenceAwareness(awareness)) return;
    return bindBlockPresence(editor, awareness);
  }, [editor, collabSession?.provider.awareness]);

  const scope: DocumentScope = { workspaceId, documentId, projectId };

  const trashDoc = useMutation({
    mutationFn: () => trashDocument(scope),
    onSuccess: async () => {
      setLifecycleError(null);
      await navigate(trashPath(slug));
      await queryClient.invalidateQueries({ queryKey: treeKey });
      await queryClient.invalidateQueries({ queryKey: ["trash", workspaceId] });
    },
    onError: (error: unknown) => {
      setLifecycleError(loadErrorMessage(error));
    },
  });

  const moveDoc = useMutation({
    mutationFn: (newParentId: string) => moveDocument(scope, newParentId),
    onSuccess: async () => {
      setLifecycleError(null);
      setMoveParentId("");
      await queryClient.invalidateQueries({ queryKey: treeKey });
      await queryClient.invalidateQueries({ queryKey: metaKey });
    },
    onError: (error: unknown) => {
      setLifecycleError(loadErrorMessage(error));
    },
  });

  const patchMeta = useMutation({
    mutationFn: (body: PatchDocumentBody) => patchDocument(scope, body),
    onSuccess: async () => {
      setSaveError(null);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: metaKey }),
        queryClient.invalidateQueries({ queryKey: treeKey }),
      ]);
    },
    onError: (error: unknown) => {
      setSaveError(loadErrorMessage(error));
    },
  });

  const notFound =
    metaQuery.error instanceof ProblemError && metaQuery.error.status === 404;

  if (notFound) {
    return (
      <div className="document-page">
        <p>{t("doc.error.notFound")}</p>
        <Link to={projectPath(slug, project.key)}>{t("nav.projects")}</Link>
      </div>
    );
  }

  if (metaQuery.isError) {
    return (
      <QueryError
        message={loadErrorMessage(metaQuery.error)}
        onRetry={() => {
          void metaQuery.refetch();
        }}
      />
    );
  }

  if (metaQuery.isLoading || !metaQuery.data) {
    return <QueryLoading />;
  }

  const displayRef = formatDisplayId(project.key, metaQuery.data.number);
  const refOf = (number: number) => formatDisplayId(project.key, number);
  const treeNode = tree.data?.items.find((node) => node.id === documentId);
  const crumbAncestors = projectAncestors(tree.data?.items ?? [], documentId, project.rootDocumentId);
  const meta = metaQuery.data;
  const docPath = meta.path;
  const docPathPrefix = `${docPath}.`;
  const saving = patchMeta.isPending;
  const archived = meta.status === "archived";
  const projectReadOnly = !project.canEdit || project.archived;
  const readOnly = archived || projectReadOnly || (collabSession?.readOnly ?? false);
  const ready = Boolean(collabSession?.synced && collabUser);
  const refusalNote = collabRefusalNote(collabSession?.status, ready);
  const badge = collabSession
    ? collabBadge(
        collabSession.status,
        collabSession.pending || persisting,
        collabSession.durableSaved,
      )
    : null;
  const canPersist =
    ready &&
    !readOnly &&
    collabSession !== null &&
    collabSession.status === "connected" &&
    !persisting;

  async function saveTitle() {
    const next = title.trim();
    if (!next || next === meta.title) return;
    try {
      await patchMeta.mutateAsync({ title: next });
    } catch {
      setTitle(meta.title);
    }
  }

  async function saveIcon() {
    const current = meta.icon ?? "";
    if (icon === current) return;
    const nextIcon = icon.trim() === "" ? null : icon.trim();
    try {
      await patchMeta.mutateAsync({ icon: nextIcon });
    } catch {
      setIcon(current);
    }
  }

  async function saveStatus(next: string) {
    if (next === meta.status) return;
    const previous = meta.status;
    try {
      await patchMeta.mutateAsync({ status: next });
    } catch {
      setStatus(previous);
    }
  }

  async function persistBody() {
    if (!collabSession || !canPersist) return;
    setPersistError(null);
    setPersisting(true);
    try {
      await collabSession.persistNow();
    } catch (error) {
      const timedOut = error instanceof Error && error.message.includes("timed out");
      setPersistError(timedOut ? t("collab timeout — retry") : t("collab unavailable"));
      throw error;
    } finally {
      setPersisting(false);
    }
  }

  const awareness = collabSession?.provider.awareness;

  return (
    <article className="document-page" data-testid={`document-${displayRef}`}>
      <header className="document-page__head">
        <nav className="document-page__breadcrumb" aria-label={t("breadcrumb.ancestors")}>
          <Link to={projectPath(slug, project.key)}>{project.key}</Link>
          {crumbAncestors.map((item) => (
            <span key={item.id}>
              <span aria-hidden> / </span>
              <Link to={documentPath(slug, refOf(item.number))}>{item.title}</Link>
            </span>
          ))}
          <span aria-hidden> / </span>
          <span>{displayRef}</span>
        </nav>
        <div className="document-page__meta">
          <input
            className="document-page__title"
            value={title}
            aria-label={t("doc.title")}
            maxLength={TITLE_MAX}
            disabled={saving || readOnly}
            onChange={(event) => setTitle(event.target.value)}
            onFocus={() => {
              if (!readOnly && isBlockPresenceAwareness(awareness)) {
                setTitleEditing(awareness, true);
              }
            }}
            onBlur={() => {
              if (isBlockPresenceAwareness(awareness)) {
                setTitleEditing(awareness, false);
              }
              void saveTitle();
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.currentTarget.blur();
              }
            }}
          />
          <div className="document-page__fields">
            <div className="document-page__field">
              <Label htmlFor="document-icon">{t("project.icon")}</Label>
              <input
                id="document-icon"
                className="document-page__field-input"
                value={icon}
                maxLength={ICON_MAX}
                disabled={saving || readOnly}
                onChange={(event) => setIcon(event.target.value)}
                onBlur={() => {
                  void saveIcon();
                }}
              />
            </div>
            <div className="document-page__field">
              <Label htmlFor="document-status">{t("doc.status.a11y")}</Label>
              <select
                id="document-status"
                className="document-page__field-select"
                value={status}
                aria-label={t("doc.status.a11y")}
                disabled={saving || readOnly}
                onChange={(event) => {
                  const next = event.target.value;
                  setStatus(next);
                  void saveStatus(next);
                }}
              >
                {STATUSES.map((value) => (
                  <option key={value} value={value}>
                    {t(
                      value === "draft"
                        ? "doc.status.draft"
                        : value === "archived"
                          ? "doc.status.archived"
                          : "doc.status.published",
                    )}
                  </option>
                ))}
              </select>
            </div>
            <span className="document-page__badge">{displayRef}</span>
            {treeNode?.status === "draft" ? (
              <span className="document-page__badge">{t("doc.status.draft")}</span>
            ) : null}
            {readOnly ? (
              <span className="document-page__badge">{t("doc.readOnly")}</span>
            ) : null}
            <StarToggle workspaceId={workspaceId} type="document" targetId={documentId} />
            {!readOnly ? (
              <ShareDialog
                workspaceId={workspaceId}
                target={{ documentId, projectId }}
              />
            ) : null}
          </div>
          <DocumentTagsBar
            workspaceId={workspaceId}
            documentId={documentId}
            projectId={projectId}
            readOnly={readOnly}
          />
          <DocumentExportMenu
            workspaceId={workspaceId}
            documentId={documentId}
            title={title}
            projectId={projectId}
            persistNow={canPersist ? persistBody : undefined}
          />
          {!readOnly ? (
            <div className="document-page__lifecycle" aria-label={t("doc.move.title")}>
              <label className="document-page__field">
                <span className="sr-only">{t("doc.move.parentLabel")}</span>
                <select
                  className="document-page__field-select"
                  value={moveParentId}
                  aria-label={t("doc.move.parentLabel")}
                  disabled={moveDoc.isPending || trashDoc.isPending}
                  onChange={(event) => setMoveParentId(event.target.value)}
                >
                  <option value="">{t("doc.move.parentLabel")}</option>
                  {(tree.data?.items ?? [])
                    .filter(
                      (node) =>
                        node.id !== documentId &&
                        node.projectId === projectId &&
                        node.path !== docPath &&
                        !node.path.startsWith(docPathPrefix),
                    )
                    .map((node) => (
                      <option key={node.id} value={node.id}>
                        {node.title}
                      </option>
                    ))}
                </select>
              </label>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!moveParentId || moveDoc.isPending || trashDoc.isPending}
                onClick={() => {
                  if (!moveParentId) return;
                  moveDoc.mutate(moveParentId);
                }}
              >
                {moveDoc.isPending ? t("doc.move.pending") : t("doc.move.submit")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={trashDoc.isPending || moveDoc.isPending}
                aria-label={t("doc.trash.action")}
                onClick={() => {
                  if (!window.confirm(`${t("doc.trash.confirm.title")}\n${t("doc.trash.confirm.body")}`)) {
                    return;
                  }
                  trashDoc.mutate();
                }}
              >
                {trashDoc.isPending ? t("doc.trash.pending") : t("doc.trash.action")}
              </Button>
            </div>
          ) : null}
          {lifecycleError ? (
            <p role="alert" className="document-page__error">{lifecycleError}</p>
          ) : null}
          <div className="document-page__collab">
            {badge ? (
              <span
                className={`document-page__collab-status document-page__collab-status--${badge.tone}`}
                data-collab-status={collabSession?.status}
                data-collab-pending={collabSession?.pending ? "true" : "false"}
                data-collab-persisted={collabSession?.durableSaved ? "true" : "false"}
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
            {collabSession ? (
              <CollabPresence peers={collabSession.peers} onJump={flashBlock} />
            ) : null}
            <RevisionPanel
              workspaceId={workspaceId}
              documentId={documentId}
              projectId={projectId}
              readOnly={readOnly}
              persistNow={canPersist ? persistBody : undefined}
            />
          </div>
          {saveError ? <p role="alert" className="document-page__error">{saveError}</p> : null}
          {persistError ? <p role="alert" className="document-page__error">{persistError}</p> : null}
        </div>
      </header>
      <section
        className="document-page__body document-page__body--editor"
        aria-label={t("doc.body.a11y")}
      >
        {collabSession?.status === "unauthorized" ? (
          <p className="document-page__body-note" role="alert">
            {t("doc.collab.unauthorized")}
          </p>
        ) : null}
        {refusalNote ? (
          <p className="document-page__body-note" role="status">
            {t(refusalNote)}
          </p>
        ) : null}
        {!ready && collabSession?.status !== "unauthorized" && !refusalNote ? (
          <QueryLoading />
        ) : null}
        {ready && collabSession && collabUser ? (
          <AttachmentBlockContext.Provider value={attachmentBridge}>
            <UrlEmbedProvider workspaceId={workspaceId}>
              <FvociEditor
                ydoc={collabSession.doc}
                provider={collabSession.provider}
                user={collabUser}
                editable={!readOnly}
                ariaLabel={t("doc.body.a11y")}
                workspaceSlug={slug}
                gutterAddLabel={t("editor.gutter.add")}
                gutterMoveLabel={t("editor.gutter.move")}
                insertLabel={t("editor.mobile.insert")}
                onReady={setEditor}
              />
            </UrlEmbedProvider>
          </AttachmentBlockContext.Provider>
        ) : null}
      </section>
      <DocumentAiMenu
        workspaceId={workspaceId}
        slug={slug}
        documentId={documentId}
        project={{ id: project.id, canCreateTasks: project.canEdit && !project.archived }}
        editor={ready ? editor : null}
        insertBlockedReason={
          readOnly
            ? t("doc.readOnly")
            : collabSession?.status === "connected"
              ? null
              : t("ai.document.loading")
        }
      />
      <OriginPanel workspaceId={workspaceId} slug={slug} documentId={documentId} />
      {me.data ? (
        <CommentPanel
          workspaceId={workspaceId}
          kind="document"
          targetId={documentId}
          projectId={projectId}
          currentUserId={me.data.userId}
          readOnly={readOnly}
        />
      ) : null}
    </article>
  );
}
