import { chunkPlainText } from "./chunk-plain-text.ts";

/**
 * Largest HWP/HWPX the browser viewer downloads (same budget as PDF); bigger
 * files stay download-only. This bounds the compressed bytes only; what they
 * may inflate to is bounded by `prepareHwpBytes` (HWPX) and rhwp's own HWP 5
 * stream caps, inside the per-document worker (`hwp-client.ts`).
 */
export const HWP_MAX_BYTES = 64 * 1024 * 1024;

/**
 * rhwp `getPageText` (0.8.6) returns the page text as a JSON string literal.
 * Decodes it and normalizes CRLF so paragraph boundaries chunk like the
 * server's plain text.
 */
export function decodePageText(raw: string): string {
  let text = raw;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "string") text = parsed;
  } catch {
    // Not JSON: already plain text.
  }
  return text.replace(/\r\n?/g, "\n");
}

/**
 * The 0-based page to open for search chunk `chunk` (source `pageOfChunk`):
 * the per-page layout text is joined with "\n" and chunked like the server.
 * The server chunks its own extract text, so this is a best-effort jump; an
 * unknown chunk opens the first page. Unlike the source, the page is taken
 * where the chunk's new text begins (the previous chunk's end), not at its
 * start, which lies inside the overlap copied from the previous page.
 */
export function pageOfChunk(pages: readonly string[], chunk: number): number {
  if (pages.length === 0) return 0;
  const chunks = chunkPlainText(pages.join("\n"));
  if (chunks[chunk] === undefined) return 0;
  const previous = chunks[chunk - 1];
  if (chunk !== 0 && previous === undefined) return 0;
  const at = previous === undefined ? 0 : previous.end;
  let seen = 0;
  for (const [index, text] of pages.entries()) {
    seen += text.length + 1;
    if (seen > at) return index;
  }
  return pages.length - 1;
}

/** Page count as shown to the user: a document always has at least one page. */
export function visiblePageCount(reported: number): number {
  return Number.isInteger(reported) && reported > 0 ? reported : 1;
}

export function clampPage(page: number, count: number): number {
  return Math.min(count - 1, Math.max(0, page));
}
