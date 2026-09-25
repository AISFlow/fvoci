import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { searchItemHref, type SearchHit as SearchTargetHit } from "@/features/workspace/search-target";

export type SearchHit = SearchTargetHit & {
  title: string;
  snippet?: Array<{ text: string; match: boolean }> | null;
  extractStatus?: string | null;
};

export function searchHitHref(slug: string, item: SearchHit): string | null {
  return searchItemHref(slug, item);
}

export function SearchResultList({
  slug,
  items,
  labelledBy,
}: {
  slug: string;
  items: SearchHit[];
  labelledBy?: string;
}) {
  return (
    <ul className="search-results" role="region" aria-labelledby={labelledBy}>
      {items.map((item) => {
        const href = searchHitHref(slug, item);
        const typeLabel =
          item.type === "document"
            ? t("search.tab.document")
            : item.type === "task"
              ? t("search.tab.task")
              : item.type === "attachment"
                ? t("search.tab.attachment")
                : item.type === "comment"
                  ? t("search.tab.comment")
                  : item.type;
        return (
          <li key={`${item.type}:${item.id}`} className="search-results__row">
            {href ? (
              <Link to={href} className="search-results__link">
                <SearchResultBody item={item} typeLabel={typeLabel} />
              </Link>
            ) : (
              <div className="search-results__link">
                <SearchResultBody item={item} typeLabel={typeLabel} />
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function SearchResultBody({ item, typeLabel }: { item: SearchHit; typeLabel: string }) {
  return (
    <>
      <span className="search-results__meta">
        <span className="search-results__kind">{typeLabel}</span>
        {item.displayId ? <span className="search-results__id">{item.displayId}</span> : null}
      </span>
      <span className="search-results__title">{item.title}</span>
      {item.snippet && item.snippet.length > 0 ? (
        <span className="search-results__snippet">
          {item.snippet.map((piece, index) =>
            piece.match ? (
              <mark key={`${item.id}-${index}`}>{piece.text}</mark>
            ) : (
              <span key={`${item.id}-${index}`}>{piece.text}</span>
            ),
          )}
        </span>
      ) : null}
      {item.extractStatus && item.extractStatus !== "ok" ? (
        <span className="search-results__badge">{t("search.badge.notIndexed")}</span>
      ) : null}
    </>
  );
}
