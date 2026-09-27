export function chunkSearch(search: URLSearchParams): { chunk?: number } {
  const raw = search.get("chunk");
  if (raw === null || raw === "") return {};
  const chunk = Number(raw);
  return Number.isInteger(chunk) && chunk >= 0 ? { chunk } : {};
}

export function isExtractableText(name: string, mime: string): boolean {
  const lower = mime.toLowerCase();
  if (lower.startsWith("text/") || lower === "application/json" || lower === "application/xml") {
    return true;
  }
  return /\.(txt|md|markdown|csv|json|xml|log)$/i.test(name);
}

export function isPdf(name: string, mime: string): boolean {
  return mime.toLowerCase() === "application/pdf" || /\.pdf$/i.test(name);
}

/** Source `officeKind` "docx" (by MIME or extension); other Office kinds stay download here. */
export function isDocx(name: string, mime: string): boolean {
  return mime.toLowerCase().includes("wordprocessingml.document") || /\.docx$/i.test(name);
}

export type ViewerKind = "image" | "pdf" | "docx" | "text" | "download";

export function viewerKind(att: { name: string; mime: string; image: boolean }): ViewerKind {
  if (att.image) return "image";
  if (isPdf(att.name, att.mime)) return "pdf";
  if (isDocx(att.name, att.mime)) return "docx";
  if (isExtractableText(att.name, att.mime)) return "text";
  return "download";
}

export function attachmentDownloadUrl(workspaceId: string, attachmentId: string): string {
  return `/api/v1/workspaces/${workspaceId}/attachments/${attachmentId}/download`;
}

/** Session-only extract text for the search-chunk supplement; share has no equivalent. */
export function attachmentPreviewHtmlUrl(workspaceId: string, attachmentId: string): string {
  return `/api/v1/workspaces/${workspaceId}/attachments/${attachmentId}/preview-html`;
}

export { attachmentViewPath } from "@/lib/href";
