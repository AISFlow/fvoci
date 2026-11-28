/**
 * Saving bytes the page has under a name: an attached file, an archive.
 *
 * The server offers every file as a download under its identifier, not
 * under the name it was uploaded with (`getFile` in `contracts/openapi.json`),
 * and that name wins over a link's `download` attribute. So the card store
 * reads the bytes, and they are saved from the page under the name the page
 * gives.
 */

/** What a file name may not hold on any system the file may be saved on: control characters among them. */
const NOT_IN_NAMES = /[\p{Cc}/\\:*?"<>|]+/gu;
/** Invisible marks of format: one that turns the text (U+202E) would show the name backwards. */
const INVISIBLE = /\p{Cf}/gu;

/**
 * `name` as the name of a file with `extension` (`.csv`, say), with nothing a
 * system refuses; `fallback` where nothing is left of it.
 */
export function fileName(name: string, extension: string, fallback: string): string {
  const kept = name.replace(INVISIBLE, '').replace(NOT_IN_NAMES, '_').trim();
  return `${kept || fallback}${extension}`;
}

/** Has the browser save `bytes` as `name`. */
export function saveBlob(bytes: Blob, name: string): void {
  const local = URL.createObjectURL(bytes);
  const link = document.createElement('a');
  link.href = local;
  link.download = name;
  link.click();
  // The browser has begun saving by now; the address is let go a little later.
  setTimeout(() => {
    URL.revokeObjectURL(local);
  }, 60_000);
}
