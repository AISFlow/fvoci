/** Source zoom contract: 50%–300% in 25% steps, reset to 100%. */
export const PDF_ZOOM_MIN = 0.5;
export const PDF_ZOOM_MAX = 3;
export const PDF_ZOOM_STEP = 0.25;

/** Largest PDF the browser viewer parses; bigger files stay download-only. */
export const PDF_MAX_BYTES = 64 * 1024 * 1024;

/** Canvas backing-store cap (4096²), within every mainstream browser's limit. */
export const PDF_MAX_CANVAS_PIXELS = 4096 * 4096;

/** Per-image decode cap handed to pdf.js (`maxImageSize`, total pixels). */
export const PDF_MAX_IMAGE_PIXELS = 50_000_000;

export function zoomIn(zoom: number): number {
  return Math.min(PDF_ZOOM_MAX, zoom + PDF_ZOOM_STEP);
}

export function zoomOut(zoom: number): number {
  return Math.max(PDF_ZOOM_MIN, zoom - PDF_ZOOM_STEP);
}

/**
 * Backing-store scale for a page whose 100% size is `width`×`height` CSS px:
 * zoom × device pixel ratio, lowered so the canvas never exceeds `maxPixels`.
 */
export function renderScale(
  width: number,
  height: number,
  zoom: number,
  dpr: number,
  maxPixels: number = PDF_MAX_CANVAS_PIXELS,
): number {
  const ratio = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  const wanted = zoom * ratio;
  const area = width * height * wanted * wanted;
  if (!(area > maxPixels)) return wanted;
  return Math.sqrt(maxPixels / (width * height));
}

export type CappedBytes = { status: "bytes"; bytes: Uint8Array } | { status: "tooLarge" };

/**
 * Reads a response body without buffering more than `max` bytes. A declared
 * `Content-Length` over the cap stops before reading; otherwise the stream is
 * cancelled as soon as it passes the cap.
 */
export async function readCapped(response: Response, max: number): Promise<CappedBytes> {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > max) {
    await response.body?.cancel();
    return { status: "tooLarge" };
  }
  if (!response.body) return { status: "bytes", bytes: new Uint8Array(0) };
  const reader = response.body.getReader();
  const parts: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel();
      return { status: "tooLarge" };
    }
    parts.push(value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.byteLength;
  }
  return { status: "bytes", bytes };
}
