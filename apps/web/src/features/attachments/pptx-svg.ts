/**
 * The slide as served to the page: a fixed outer SVG, written entirely here,
 * whose only content is one `<image>` of the renderer's SVG as a
 * `data:image/svg+xml;base64,` URL.
 *
 * `@office-kit/pptx-preview` interpolates some deck strings into its markup
 * unescaped (chart number-format prefixes, for one), so its output is treated
 * as hostile markup and never filtered or rewritten here. The one change
 * made to it before this wrapper, in the worker, is `pptx-fallback.ts`: a
 * standard XML tokenizer finds the renderer's placeholder labels, and each
 * is enclosed in a viewport of its own box. That is layout, not a security
 * measure; the boundary is what follows. SVG that an
 * `<image>` element references is processed as an image (SVG Integration):
 * no script, no external loads, no links or other interaction. That holds for
 * the inner slide wherever the outer blob is shown: through the viewer's
 * `<img>`, which is itself image mode, or opened on its own as a document
 * ("open image in new tab"), where the outer markup — ours — has nothing
 * active and the slide inside is still an image.
 *
 * The renderer inlines pictures as `data:` URLs, which image mode displays.
 * No DOM parse happens in the page, so the app CSP (`style-src`) reports
 * nothing for the slide's style attributes.
 */

export const SLIDE_IMAGE_TYPE = "image/svg+xml";

/** A slide dimension, in CSS px, as the outer template writes it. */
const MAX_DIMENSION = 1_000_000;

/** Bytes per `btoa` call: a multiple of 3, so the pieces join into one base64 string. */
const BASE64_CHUNK = 3 * 8 * 1024;

/** Standard base64 of `bytes`, without a per-byte string of the whole input. */
export function toBase64(bytes: Uint8Array): string {
  const parts: string[] = [];
  for (let at = 0; at < bytes.byteLength; at += BASE64_CHUNK) {
    const chunk = bytes.subarray(at, Math.min(bytes.byteLength, at + BASE64_CHUNK));
    parts.push(btoa(String.fromCharCode(...chunk)));
  }
  return parts.join("");
}

function dimension(value: number): string | null {
  if (!(Number.isFinite(value) && value <= MAX_DIMENSION)) return null;
  const rounded = Math.round(value * 100) / 100;
  // Also rejects a positive value that rounds to "0": a zero-size image.
  return rounded > 0 ? String(rounded) : null;
}

export type SlideImageSvg =
  { status: "ok"; svg: string } | { status: "tooLarge" } | { status: "failed" };

/**
 * Wraps renderer SVG `inner` for a `width` × `height` px slide. `inner` is
 * encoded as UTF-8 (`TextEncoder`, which replaces any lone surrogate with
 * U+FFFD) and must start with the renderer's `<svg` root; more than
 * `maxBytes` of it is `tooLarge`.
 */
export function slideImageSvg(
  inner: string,
  width: number,
  height: number,
  maxBytes: number,
): SlideImageSvg {
  const w = dimension(width);
  const h = dimension(height);
  if (w === null || h === null || !/^<svg[\s>]/.test(inner)) return { status: "failed" };
  // UTF-8 is never shorter than the UTF-16 length, so this skips encoding a string that cannot fit.
  if (inner.length > maxBytes) return { status: "tooLarge" };
  const bytes = new TextEncoder().encode(inner);
  if (bytes.byteLength > maxBytes) return { status: "tooLarge" };
  return {
    status: "ok",
    svg:
      `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
      `<image width="${w}" height="${h}" href="data:${SLIDE_IMAGE_TYPE};base64,${toBase64(bytes)}"/></svg>`,
  };
}

const OUTER =
  /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg" width="([0-9.]+)" height="([0-9.]+)" viewBox="0 0 \1 \2"><image width="\1" height="\2" href="data:image\/svg\+xml;base64,([A-Za-z0-9+/]*={0,2})"\/><\/svg>$/;

/**
 * The inner slide SVG of a served blob, or `null` unless `outer` is exactly
 * the template `slideImageSvg` writes. For tests and the browser spec.
 */
export function innerSlideSvg(outer: string): string | null {
  const match = OUTER.exec(outer);
  const encoded = match?.[3];
  if (encoded === undefined || encoded.length % 4 !== 0) return null;
  const binary = atob(encoded);
  const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}
