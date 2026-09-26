/** Pure helpers for stars/recent rows and the public share page (no DOM, no app imports). */

/** Public `/instance` share policy (`values.share`). */
export type SharePolicy = { enabled: boolean; defaultExpiresDays: number; maxExpiresDays: number };

/** Source `SETTINGS_CATALOG.share.default`, used until `/instance` loads. */
export const SHARE_POLICY_DEFAULT: SharePolicy = {
  enabled: true,
  defaultExpiresDays: 7,
  maxExpiresDays: 365,
};

const SHARE_EXPIRES_PRESETS = [7, 30, 90, 365];

/** Source share dialog: presets plus the policy default, none above the policy max, ascending. */
export function shareExpiresOptions(policy: SharePolicy): number[] {
  return [...new Set([...SHARE_EXPIRES_PRESETS, policy.defaultExpiresDays])]
    .filter((days) => days <= policy.maxExpiresDays)
    .sort((a, b) => a - b);
}

/** The user's pick while it is still offered, otherwise the policy default. */
export function selectedShareExpires(chosen: number | null, policy: SharePolicy): number {
  return chosen !== null && shareExpiresOptions(policy).includes(chosen)
    ? chosen
    : policy.defaultExpiresDays;
}

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
  // Browsers read `\\host` and `/\host` as protocol-relative `//host`.
  if (!scheme) return !/^[/\\]{2}/.test(value) && !/[\u0000-\u001f]/.test(value);
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

