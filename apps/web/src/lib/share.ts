/** Pure helpers for stars/recent rows and the public share page (no DOM, no app imports). */

/** Source share dialog expiry choices (instance setting catalog default 7, max 365). */
export const SHARE_EXPIRES_OPTIONS = [7, 30, 90, 365] as const;
export const SHARE_DEFAULT_EXPIRES_DAYS = 7;

export type ShareTreeNode = { id: string; parentId: string | null };

/**
 * A document share's `/tree` is the shared subtree: its root keeps the real
 * `parentId`, which is outside the share. Roots are nodes whose parent is not
 * in the list, in server order.
 */
export function shareTreeRoots<T extends ShareTreeNode>(nodes: readonly T[]): T[] {
  const ids = new Set(nodes.map((node) => node.id));
  return nodes.filter((node) => node.parentId === null || !ids.has(node.parentId));
}

export function shareTreeChildren<T extends ShareTreeNode>(
  nodes: readonly T[],
  parentId: string,
): T[] {
  return nodes.filter((node) => node.parentId === parentId);
}

/** Server fragment hrefs are limited to http/https/mailto/relative; re-check before rendering. */
export function isSafeShareHref(href: string): boolean {
  const value = href.trim();
  if (value === "") return false;
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(value);
  if (!scheme) return !value.startsWith("//") && !/[\u0000-\u001f]/.test(value);
  const name = scheme[1].toLowerCase();
  return name === "http" || name === "https" || name === "mailto";
}

/** `/s/:token` path of a created share URL (the origin comes from server config). */
export function sharePathFromUrl(url: string): string | null {
  try {
    const parsed = new URL(url, "https://share.invalid");
    const parts = parsed.pathname.split("/").filter((part) => part.length > 0);
    if (parts.length === 2 && parts[0] === "s" && parts[1]) return `/s/${parts[1]}`;
    return null;
  } catch {
    return null;
  }
}

/** Source `displayId(projectKeys, item)`: wiki items are `WIKI-n`, project items `KEY-n`. */
export function starItemDisplayId(
  item: { projectId: string | null; number: number },
  projectKeyById: ReadonlyMap<string, string>,
): string | null {
  if (item.projectId === null) return `WIKI-${item.number}`;
  const key = projectKeyById.get(item.projectId);
  return key ? `${key}-${item.number}` : null;
}

