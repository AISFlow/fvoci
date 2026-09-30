import { t } from "@fvoci/i18n";
import { COMMENTS_ANCHOR_ID, attachmentViewPath, documentPath } from "@/lib/href";

export type SearchHit = {
  type: string;
  id: string;
  documentId?: string | null;
  taskId?: string | null;
  displayId?: string | null;
  chunkNo?: number | null;
};

/** A search result row as both web apps list it: the hit plus its title, snippet and extraction state. */
export type SearchResult = SearchHit & {
  title: string;
  snippet?: Array<{ text: string; match: boolean }> | null;
  extractStatus?: string | null;
};

/** The result row's kind label (the search tab names); an unknown type shows as is. */
export function searchHitTypeLabel(type: string): string {
  return type === "document"
    ? t("search.tab.document")
    : type === "task"
      ? t("search.tab.task")
      : type === "attachment"
        ? t("search.tab.attachment")
        : type === "comment"
          ? t("search.tab.comment")
          : type;
}

export function searchItemHref(slug: string, item: SearchHit): string | null {
  if (item.type === "attachment") {
    return attachmentViewPath(slug, item.id, item.chunkNo);
  }
  const ref = item.displayId;
  if (!ref) {
    return null;
  }
  const base = documentPath(slug, ref);
  return item.type === "comment" ? `${base}#${COMMENTS_ANCHOR_ID}` : base;
}
