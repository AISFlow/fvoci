import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Label } from "@/components/ui/label";
import { documentPath, wikiDisplayId, wikiPath } from "@/lib/href";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { components } from "@/generated/api";
import {
  ancestorsQuery,
  documentBodyQuery,
  documentMetaQuery,
  treeQuery,
} from "@/lib/queries/documents";
import "./document-shell.css";

type PatchDocumentBody = components["schemas"]["PatchDocumentBody"];

const STATUSES = ["draft", "published", "archived"] as const;
const TITLE_MAX = 300;
const ICON_MAX = 50;

function isEmptyBody(content: unknown): boolean {
  if (!content || typeof content !== "object") return true;
  const doc = content as { type?: string; content?: unknown[] };
  if (doc.type !== "doc" || !Array.isArray(doc.content)) return false;
  if (doc.content.length === 0) return true;
  if (doc.content.length === 1) {
    const block = doc.content[0] as { type?: string; content?: unknown[] };
    return block.type === "paragraph" && (!block.content || block.content.length === 0);
  }
  return false;
}

interface DocumentViewProps {
  workspaceId: string;
  slug: string;
  documentId: string;
}

export function DocumentView({ workspaceId, slug, documentId }: DocumentViewProps) {
  const queryClient = useQueryClient();
  const metaQuery = useQuery(documentMetaQuery(workspaceId, documentId));
  const bodyQuery = useQuery(documentBodyQuery(workspaceId, documentId));
  const ancestors = useQuery(ancestorsQuery(workspaceId, documentId));
  const tree = useQuery(treeQuery(workspaceId));

  const [title, setTitle] = useState("");
  const [icon, setIcon] = useState("");
  const [status, setStatus] = useState<string>("draft");
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    if (!metaQuery.data) return;
    setTitle(metaQuery.data.title);
    setIcon(metaQuery.data.icon ?? "");
    setStatus(metaQuery.data.status);
  }, [metaQuery.data]);

  const patchMeta = useMutation({
    mutationFn: async (body: PatchDocumentBody) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/documents/{document_id}", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
          body,
        }),
      ),
    onSuccess: async () => {
      setSaveError(null);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["document", workspaceId, documentId] }),
        queryClient.invalidateQueries({ queryKey: ["tree", workspaceId] }),
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
        <Link to={wikiPath(slug)}>{t("nav.toWiki")}</Link>
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

  const displayRef = wikiDisplayId(metaQuery.data.number);
  const treeNode = tree.data?.items.find((node) => node.id === documentId);
  const crumbAncestors = ancestors.data?.items ?? [];
  const meta = metaQuery.data;
  const saving = patchMeta.isPending;

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

  const bodyNote = bodyQuery.data
    ? isEmptyBody(bodyQuery.data.contentJson)
      ? t("doc.empty")
      : t("doc.body.unavailable")
    : null;

  return (
    <article className="document-page" data-testid={`document-${displayRef}`}>
      <header className="document-page__head">
        <nav className="document-page__breadcrumb" aria-label={t("breadcrumb.ancestors")}>
          <Link to={wikiPath(slug)}>{t("nav.wiki")}</Link>
          {crumbAncestors.map((item) => (
            <span key={item.id}>
              <span aria-hidden> / </span>
              <Link to={documentPath(slug, wikiDisplayId(item.number))}>{item.title}</Link>
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
            disabled={saving}
            onChange={(event) => setTitle(event.target.value)}
            onBlur={() => {
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
                disabled={saving}
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
                disabled={saving}
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
          </div>
          {saveError ? <p role="alert" className="document-page__error">{saveError}</p> : null}
        </div>
      </header>
      <section className="document-page__body" aria-label={t("doc.body.a11y")}>
        <h2 className="document-page__body-title">{t("doc.readOnly")}</h2>
        {bodyQuery.isLoading ? <QueryLoading /> : null}
        {bodyQuery.isError ? (
          <QueryError
            message={loadErrorMessage(bodyQuery.error)}
            onRetry={() => {
              void bodyQuery.refetch();
            }}
          />
        ) : null}
        {bodyNote ? <p className="document-page__body-note">{bodyNote}</p> : null}
      </section>
    </article>
  );
}
