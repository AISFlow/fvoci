import { HWP_MAX_BYTES } from "./hwp-page.ts";
import { PDF_MAX_BYTES, readCapped, type CappedBytes } from "./pdf-limits.ts";
import { XLSX_MAX_BYTES } from "./xlsx-limits.ts";

/** Largest DOCX the browser viewer downloads; bigger files stay download-only. */
export const DOCX_MAX_BYTES = 32 * 1024 * 1024;

/** Largest PPTX the browser viewer downloads; bigger files stay download-only. */
export const PPTX_MAX_BYTES = 64 * 1024 * 1024;

export type LayoutKind = "pdf" | "docx" | "hwp" | "pptx" | "xlsx";

/** Download cap per layout viewer, readable without loading the viewer chunk. */
export const VIEWER_MAX_BYTES: Readonly<Record<LayoutKind, number>> = {
  pdf: PDF_MAX_BYTES,
  docx: DOCX_MAX_BYTES,
  hwp: HWP_MAX_BYTES,
  pptx: PPTX_MAX_BYTES,
  xlsx: XLSX_MAX_BYTES,
};

/** A capped body, or `failed` for a non-2xx response (its body is cancelled). */
export type ViewerBytes = CappedBytes | { status: "failed" };

/**
 * Credentials for fetching an original through the API's download route: the
 * session cookie goes to the app origin, and nothing goes to the storage
 * origin a presigned `302` leads to (the signed URL is the whole
 * authorization), so bucket CORS needs no `Access-Control-Allow-Credentials`.
 */
export const ORIGINAL_FETCH_CREDENTIALS: RequestCredentials = "same-origin";

/**
 * Downloads a viewer's file with the session cookie, never buffering more
 * than `max` bytes. Network errors and aborts reject.
 */
export async function downloadCapped(
  url: string,
  max: number,
  signal: AbortSignal,
): Promise<ViewerBytes> {
  const response = await fetch(url, { credentials: ORIGINAL_FETCH_CREDENTIALS, signal });
  if (!response.ok) {
    await response.body?.cancel();
    return { status: "failed" };
  }
  return readCapped(response, max);
}

/**
 * A download the parent starts as soon as the attachment metadata names the
 * viewer, in parallel with loading the viewer chunk. The viewer takes it once
 * for its first load; a retry, or a second mount, downloads again. Aborting the
 * signal passed to `take` (the viewer's own lifetime) aborts the download too.
 */
export type ViewerPrefetch = {
  take(signal: AbortSignal): Promise<ViewerBytes> | null;
};

export function startViewerPrefetch(
  url: string,
  max: number,
  signal: AbortSignal,
  download: typeof downloadCapped = downloadCapped,
): ViewerPrefetch {
  const controller = new AbortController();
  const abort = () => controller.abort();
  if (signal.aborted) abort();
  else signal.addEventListener("abort", abort, { once: true });
  let pending: Promise<ViewerBytes> | null = download(url, max, controller.signal);
  // Nobody may take it (the chunk failed, or the page moved on): never an unhandled rejection.
  pending.catch(() => {});
  return {
    take(taker) {
      const taken = pending;
      pending = null;
      if (taken === null) return null;
      if (taker.aborted) abort();
      else taker.addEventListener("abort", abort, { once: true });
      return taken;
    },
  };
}
