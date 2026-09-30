/** A plain local anchor can leave Vue without consulting its route guards. */
export function guardedViewerLink(
  event: Pick<MouseEvent, "button" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "defaultPrevented">,
  link: { href: string; target: string; download: boolean },
  current: string,
): string | null {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return null;
  if (link.download || (link.target && link.target.toLowerCase() !== "_self")) return null;
  const url = new URL(link.href, current);
  const here = new URL(current);
  if (!/^https?:$/.test(url.protocol) || url.origin !== here.origin) return null;
  // An anchor within this document does not discard edits.
  if (url.pathname === here.pathname && url.search === here.search && url.hash) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}
