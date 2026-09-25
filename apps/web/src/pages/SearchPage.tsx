import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchResultList, type SearchHit } from "@/features/workspace/search-results";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { searchPath } from "@/lib/href";
import { searchQuery, type SearchTab } from "@/lib/queries";

const TABS: SearchTab[] = ["all", "document", "task", "attachment", "comment"];

function parseTab(raw: string | null): SearchTab {
  return TABS.includes(raw as SearchTab) ? (raw as SearchTab) : "all";
}

export function SearchPage() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { slug, workspace } = useWorkspaceContext();
  const q = (params.get("q") ?? "").trim();
  const tab = parseTab(params.get("tab"));
  const projectId = params.get("projectId") ?? undefined;
  const [draft, setDraft] = useState(q);
  const [extra, setExtra] = useState<SearchHit[]>([]);
  const [nextCursor, setNextCursor] = useState<string | undefined>(undefined);
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);

  const page = useQuery(searchQuery(workspace?.id ?? "", q, tab, projectId));

  useEffect(() => {
    setDraft(q);
    setExtra([]);
    setNextCursor(page.data?.nextCursor ?? undefined);
    setMoreError(null);
  }, [q, tab, projectId, page.data?.nextCursor]);

  const items = [...((page.data?.items ?? []) as SearchHit[]), ...extra];

  if (!workspace) return null;

  function replaceQuery(next: { q?: string; tab?: SearchTab; projectId?: string }) {
    void navigate(
      searchPath(slug, {
        q: next.q ?? q,
        tab: next.tab ?? tab,
        projectId: next.projectId ?? projectId,
      }),
      { replace: true },
    );
  }

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="search"
    >
      <div className="search-page">
        <h1 id="search-page-title" className="search-page__title">
          {t("search.title")}
        </h1>
        <p className="search-page__hint">{t("search.hint")}</p>
        <form
          className="search-page__form"
          role="search"
          onSubmit={(event) => {
            event.preventDefault();
            replaceQuery({ q: draft.trim() });
          }}
        >
          <label className="sr-only" htmlFor="workspace-search-q">
            {t("search.query")}
          </label>
          <Input
            id="workspace-search-q"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder={t("search.queryPlaceholder")}
            autoComplete="off"
            enterKeyHint="search"
          />
          <Button type="submit">{t("nav.search")}</Button>
        </form>
        <div className="search-page__tabs" role="tablist" aria-label={t("search.resultType")}>
          {TABS.map((value) => (
            <button
              key={value}
              type="button"
              role="tab"
              aria-selected={tab === value}
              className={tab === value ? "search-page__tab is-active" : "search-page__tab"}
              onClick={() => replaceQuery({ tab: value })}
            >
              {t(
                value === "all"
                  ? "search.tab.all"
                  : value === "document"
                    ? "search.tab.document"
                    : value === "task"
                      ? "search.tab.task"
                      : value === "attachment"
                        ? "search.tab.attachment"
                        : "search.tab.comment",
              )}
            </button>
          ))}
        </div>
        {!q ? <p className="search-page__status">{t("search.hint")}</p> : null}
        {q && page.isLoading ? <QueryLoading /> : null}
        {q && page.isError ? (
          <QueryError
            message={loadErrorMessage(page.error)}
            onRetry={() => {
              void page.refetch();
            }}
          />
        ) : null}
        {q && !page.isLoading && !page.isError && items.length === 0 ? (
          <p className="search-page__status">{t("search.empty")}</p>
        ) : null}
        {items.length > 0 ? (
          <SearchResultList slug={slug} items={items} labelledBy="search-page-title" />
        ) : null}
        {moreError ? (
          <p role="alert" className="search-page__status">
            {moreError}
          </p>
        ) : null}
        {q && nextCursor ? (
          <Button
            type="button"
            variant="outline"
            disabled={loadingMore}
            onClick={async () => {
              if (!nextCursor) return;
              setLoadingMore(true);
              setMoreError(null);
              try {
                const fetched = await ensureOk(
                  await api.GET("/api/v1/workspaces/{workspace_id}/search", {
                    params: {
                      path: { workspace_id: workspace.id },
                      query: {
                        q,
                        type: tab,
                        cursor: nextCursor,
                        ...(projectId ? { projectId } : {}),
                      },
                    },
                  }),
                );
                setExtra((current) => [...current, ...((fetched.items ?? []) as SearchHit[])]);
                setNextCursor(fetched.nextCursor ?? undefined);
              } catch {
                setMoreError(t("search.loadMoreError"));
              } finally {
                setLoadingMore(false);
              }
            }}
          >
            {t("search.loadMore")}
          </Button>
        ) : null}
      </div>
    </WorkspaceShell>
  );
}
