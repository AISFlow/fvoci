import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { components } from "@/generated/api";
import { membersQuery, meQuery } from "@/lib/queries";
import { persistThenCreate } from "./revision-persist";
import "./document-shell.css";

type RevisionMeta = components["schemas"]["RevisionMetaResponse"];
type RevisionDetail = components["schemas"]["RevisionDetailResponse"];

const REASON_LABEL: Record<string, string> = {
  manual: t("version.reason.manual"),
  session: t("version.reason.session"),
  scheduled: t("version.reason.scheduled"),
};

function formatAt(iso: string, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat("ko", {
      timeZone,
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(iso));
  } catch {
    return iso;
  }
}

function extractPreviewText(node: unknown): string {
  if (!node || typeof node !== "object") return "";
  const record = node as { text?: unknown; content?: unknown[] };
  if (typeof record.text === "string") return record.text;
  return (record.content ?? []).map(extractPreviewText).join("");
}

function authorLabel(
  item: RevisionMeta,
  authorById: Readonly<Record<string, string>>,
): string {
  if (item.createdBy === null || item.createdBy === undefined) {
    return t("version.author.system");
  }
  const name = authorById[item.createdBy];
  return name && name.trim().length > 0 ? name : t("version.author.member");
}

export function RevisionPanel({
  workspaceId,
  documentId,
  readOnly,
  persistNow,
}: {
  workspaceId: string;
  documentId: string;
  readOnly: boolean;
  persistNow?: () => Promise<void>;
}) {
  const queryClient = useQueryClient();
  const me = useQuery(meQuery);
  const timeZone = me.data?.timezone || "Asia/Seoul";
  const [open, setOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [pendingRestoreId, setPendingRestoreId] = useState<string | null>(null);
  const [preview, setPreview] = useState<RevisionDetail | null>(null);
  const correlations = useRef(new Map<string, string>());
  const openerRef = useRef<HTMLElement | null>(null);
  const cancelRestoreRef = useRef<HTMLButtonElement>(null);
  const confirmRestoreRef = useRef<HTMLButtonElement>(null);
  const queryKey = ["revisions", workspaceId, documentId] as const;

  useEffect(() => {
    if (!pendingRestoreId) {
      openerRef.current?.focus();
      openerRef.current = null;
      return;
    }
    if (!openerRef.current && document.activeElement instanceof HTMLElement) {
      openerRef.current = document.activeElement;
    }
    cancelRestoreRef.current?.focus();
  }, [pendingRestoreId]);

  const listQuery = useQuery({
    queryKey,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
            query: { limit: 20 },
          },
        }),
      ),
    enabled: open,
  });

  const members = useQuery({
    ...membersQuery(workspaceId),
    enabled: open,
  });
  const authorById = Object.fromEntries(
    (members.data?.items ?? []).map((member) => [
      member.userId,
      formatPersonName(member, me.data?.locale),
    ]),
  );

  const saveRevision = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
        }),
      ),
    onSuccess: async () => {
      setNotice(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: () => setNotice(t("version.save.failed")),
  });

  const restore = useMutation({
    mutationFn: async (revId: string) => {
      let correlationId = correlations.current.get(revId);
      if (!correlationId) {
        correlationId = crypto.randomUUID();
        correlations.current.set(revId, correlationId);
      }
      return ensureOk(
        await api.POST(
          "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                document_id: documentId,
                revision_id: revId,
              },
            },
            body: { correlationId },
          },
        ),
      );
    },
    onSuccess: async (_data, revId) => {
      correlations.current.delete(revId);
      setPendingRestoreId(null);
      setNotice(t("version.restore.done"));
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (err) => {
      const timedOut = err instanceof ProblemError && err.status === 504;
      if (timedOut) {
        void queryClient.invalidateQueries({ queryKey });
        void queryClient.invalidateQueries({
          queryKey: ["document", workspaceId, documentId],
        });
        void queryClient.invalidateQueries({
          queryKey: ["document-body", workspaceId, documentId],
        });
      }
      setNotice(timedOut ? t("version.restore.timeout") : t("version.restore.failed"));
    },
  });

  async function showPreview(id: string) {
    try {
      const detail = await ensureOk(
        await api.GET(
          "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                document_id: documentId,
                revision_id: id,
              },
            },
          },
        ),
      );
      setPreview(detail);
    } catch {
      setNotice(t("version.list.failed"));
    }
  }

  return (
    <div className="document-revision-host">
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-expanded={open}
        aria-controls="document-revision-panel"
        data-testid="revision-history"
        onClick={() => setOpen((value) => !value)}
      >
        {t("version.history")}
      </Button>
      {open ? (
        <aside
          id="document-revision-panel"
          className="document-revision-panel"
          aria-label={t("version.historyTitle")}
        >
          <header className="document-revision-panel__head">
            <h2>{t("version.historyTitle")}</h2>
            {!readOnly ? (
              <Button
                type="button"
                size="sm"
                disabled={saveRevision.isPending}
                data-testid="revision-save"
                onClick={() => {
                  void persistThenCreate(persistNow, () => saveRevision.mutate()).catch(() =>
                    setNotice(t("version.save.failed")),
                  );
                }}
              >
                {saveRevision.isPending ? t("version.saving") : t("version.save")}
              </Button>
            ) : null}
          </header>
          {notice ? (
            <p role="status" className="document-revision-panel__notice">
              {notice}
            </p>
          ) : null}
          {listQuery.isLoading ? (
            <p className="document-revision-panel__notice">{t("load.loading")}</p>
          ) : null}
          {listQuery.isError ? (
            <p role="alert" className="document-page__error">
              {t("version.list.failed")}
            </p>
          ) : null}
          {!listQuery.isLoading && !listQuery.isError && (listQuery.data?.items.length ?? 0) === 0 ? (
            <p className="document-revision-panel__notice">{t("version.empty")}</p>
          ) : null}
          <ul className="document-revision">
            {(listQuery.data?.items ?? []).map((item) => {
              const when = formatAt(item.createdAt, timeZone);
              const who = authorLabel(item, authorById);
              const reason = REASON_LABEL[item.reason] ?? item.reason;
              return (
                <li key={item.id} className="document-revision__item" data-testid="revision-item">
                  <button
                    type="button"
                    className="document-revision__meta"
                    onClick={() => void showPreview(item.id)}
                  >
                    <span className="document-revision__when">{when}</span>
                    <span className="document-revision__who">
                      <span>{reason}</span> <span>{who}</span>
                    </span>
                  </button>
                  {!readOnly ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      className="min-h-11"
                      disabled={restore.isPending}
                      data-testid="revision-restore"
                      onClick={() => setPendingRestoreId(item.id)}
                    >
                      {t("version.restore")}
                    </Button>
                  ) : null}
                </li>
              );
            })}
          </ul>
          {preview ? (
            <section className="document-revision-preview" aria-label={t("version.preview")}>
              <p data-testid="revision-preview">{extractPreviewText(preview.contentJson) || "…"}</p>
            </section>
          ) : null}
          {pendingRestoreId ? (
            <div
              className="document-revision-dialog"
              role="dialog"
              aria-modal="true"
              aria-labelledby="revision-restore-title"
              aria-describedby="revision-restore-body"
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setPendingRestoreId(null);
                  return;
                }
                if (event.key !== "Tab") {
                  return;
                }
                const first = cancelRestoreRef.current;
                const last = confirmRestoreRef.current;
                if (!first || !last) {
                  return;
                }
                if (event.shiftKey && document.activeElement === first) {
                  event.preventDefault();
                  last.focus();
                } else if (!event.shiftKey && document.activeElement === last) {
                  event.preventDefault();
                  first.focus();
                }
              }}
            >
              <h3 id="revision-restore-title">{t("version.dialog.title")}</h3>
              <p id="revision-restore-body" className="document-revision-dialog__body">
                {t("version.dialog.body")}
              </p>
              <div className="document-revision-dialog__actions">
                <Button
                  ref={cancelRestoreRef}
                  type="button"
                  variant="outline"
                  onClick={() => setPendingRestoreId(null)}
                >
                  {t("version.dialog.cancel")}
                </Button>
                <Button
                  ref={confirmRestoreRef}
                  type="button"
                  data-testid="revision-restore-confirm"
                  disabled={restore.isPending}
                  onClick={() => restore.mutate(pendingRestoreId)}
                >
                  {t("version.restore")}
                </Button>
              </div>
            </div>
          ) : null}
        </aside>
      ) : null}
    </div>
  );
}
