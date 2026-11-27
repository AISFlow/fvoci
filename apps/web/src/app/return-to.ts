/**
 * Where the reader was going when a sign-in through a provider took them
 * away: kept for the tab while the browser is at the provider, and taken
 * when it comes back to the root, which is where the server sends it.
 *
 * Only a path of this site is kept and taken, and only for a while: a
 * sign-in that never came back does not send a later visit somewhere old.
 */
import { buildUrl } from '$lib/logic/urls.ts';

import { parseRoute, type Router } from './router.svelte.ts';

const KEY = 'fvoci:return-to';

/** How long a sign-in may take to come back: longer than the server lets it (ten minutes). */
const KEPT_FOR = 15 * 60 * 1000;

/** A path of this site: not another site's address, however written. */
function isOwnPath(path: unknown): path is string {
  return typeof path === 'string' && /^\/(?![/\\])/.test(path) && !path.includes('\\');
}

export function keepReturn(path: string, now: number = Date.now()): void {
  if (!isOwnPath(path)) return;
  try {
    sessionStorage.setItem(KEY, JSON.stringify({ path, at: now }));
  } catch {
    // Without storage the reader comes back to the root.
  }
}

/** The path kept, once: it is forgotten as it is read. */
export function takeReturn(now: number = Date.now()): string | undefined {
  try {
    const kept: unknown = JSON.parse(sessionStorage.getItem(KEY) ?? 'null');
    sessionStorage.removeItem(KEY);
    if (typeof kept !== 'object' || kept === null) return undefined;
    const { path, at } = kept as Record<string, unknown>;
    if (!isOwnPath(path) || typeof at !== 'number') return undefined;
    return now - at >= 0 && now - at < KEPT_FOR ? path : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Back from a provider, the browser is at the root (`cameBack` its query): the
 * reader goes on to where they were when they left for it, if that is a page of
 * the app.
 */
export function resume(router: Router, cameBack: URLSearchParams): void {
  const kept = takeReturn();
  // What the page was told once (a provider connected, the account asked to be deleted) is not told again.
  if (cameBack.has('oidc_linked') || cameBack.has('account_leaving')) {
    history.replaceState(null, '', location.pathname);
  }
  if (kept === undefined || router.route.kind !== 'home') return;
  const route = parseRoute(kept, router.basePath);
  if (route.kind === 'invite') {
    location.replace(
      route.invitation ? router.inviteHref(route.invitation) : buildUrl('/invite', router.basePath),
    );
  } else if (route.kind !== 'unknown') {
    router.go(route, { replace: true });
  }
}
