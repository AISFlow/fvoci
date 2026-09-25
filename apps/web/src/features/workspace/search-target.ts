import { COMMENTS_ANCHOR_ID, attachmentViewPath, documentPath } from "@/lib/href";

export type SearchHit = {
  type: string;
  id: string;
  documentId?: string | null;
  taskId?: string | null;
  displayId?: string | null;
  chunkNo?: number | null;
};

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
