/**
 * Saving bytes the page has under a name: an attached file, an archive.
 *
 * The server offers every file as a download under its identifier, not
 * under the name it was uploaded with (`getFile` in `contracts/openapi.json`),
 * and that name wins over a link's `download` attribute. So the card store
 * reads the bytes, and they are saved from the page under the name the page
 * gives.
 */

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
