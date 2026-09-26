import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchResultList, type SearchHit } from "@/features/workspace/search-results";
import { searchPath } from "@/lib/href";
import { searchQuery } from "@/lib/queries";

export function SearchCommand({
  slug,
  workspaceId,
}: {
  slug: string;
  workspaceId: string;
}) {
  const navigate = useNavigate();
  const dialogId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [q, setQ] = useState("");

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen(true);
      }
      if (event.key === "Escape" && open) {
        event.preventDefault();
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    const handle = window.setTimeout(() => setQ(draft.trim()), 250);
    return () => window.clearTimeout(handle);
  }, [draft]);

  // Source command palette: workspace search asks for hybrid; the search page stays lexical.
  const results = useQuery(searchQuery(workspaceId, q, "all", undefined, undefined, "hybrid"));
  const items = (results.data?.items ?? []) as SearchHit[];

  return (
    <>
      <Button
        type="button"
        size="sm"
        variant="outline"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? dialogId : undefined}
        onClick={() => setOpen(true)}
      >
        {t("nav.search")}
      </Button>
      {open ? (
        <div className="search-command">
          <button
            type="button"
            className="search-command__backdrop"
            aria-label={t("search.close")}
            onClick={() => setOpen(false)}
          />
          <div
            id={dialogId}
            role="dialog"
            aria-modal="true"
            aria-labelledby={`${dialogId}-title`}
            className="search-command__dialog"
          >
            <h2 id={`${dialogId}-title`} className="search-command__title">
              {t("search.command")}
            </h2>
            <form
              className="search-command__form"
              onSubmit={(event) => {
                event.preventDefault();
                const next = draft.trim();
                if (!next) return;
                setOpen(false);
                void navigate(searchPath(slug, { q: next }));
              }}
            >
              <label className="sr-only" htmlFor={`${dialogId}-q`}>
                {t("search.query")}
              </label>
              <Input
                ref={inputRef}
                id={`${dialogId}-q`}
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder={t("search.queryPlaceholder")}
                autoComplete="off"
                enterKeyHint="search"
              />
            </form>
            {results.isFetching ? (
              <p role="status" className="search-command__status">
                {t("search.loading")}
              </p>
            ) : null}
            {results.isError ? (
              <p role="alert" className="search-command__status">
                {t("search.failed")}
              </p>
            ) : null}
            {!results.isFetching && !results.isError && q && items.length === 0 ? (
              <p className="search-command__status">{t("search.empty")}</p>
            ) : null}
            {items.length > 0 ? <SearchResultList slug={slug} items={items.slice(0, 8)} /> : null}
            {q ? (
              <Link
                className="search-command__all"
                to={searchPath(slug, { q })}
                onClick={() => setOpen(false)}
              >
                {t("search.seeAll")}
              </Link>
            ) : (
              <p className="search-command__hint">{t("search.hint")}</p>
            )}
          </div>
        </div>
      ) : null}
    </>
  );
}
