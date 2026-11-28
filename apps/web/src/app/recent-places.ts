/**
 * What each reader opened last in a workspace on this browser, for the start
 * of the search ("최근 연 항목"): the identifiers alone and what they are, the
 * latest first, at most `RECENT_KEPT`. What they are called is read when they
 * are shown, as the reader may read them then: what they may not, or what is
 * gone, is left out. No diary, and no entry of one, is kept: what a diary
 * holds is never shown at the head of the search, and nothing that is no
 * identifier (from an address made by hand, say) is kept or read: it would
 * have the server refuse what is asked with it. Signing out, or a session
 * that is over, forgets them all at once (`forgetUnscoped`, `App`).
 */
import type { Favorite, FavoriteKind, Workspace } from '$lib/state/index.ts';

import type { Route } from './route-path.ts';

/** How many are kept, and shown. */
export const RECENT_KEPT = 8;

const KEY = 'fvoci:recent:';

/** A place opened: what it is, by its identifier. */
export interface RecentPlace {
  readonly id: string;
  readonly kind: FavoriteKind;
}

const KINDS: ReadonlySet<string> = new Set<FavoriteKind>(['container', 'card', 'page', 'unit']);

/** An identifier as the server takes one: a UUID in small letters, not the nil one. */
const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NIL = '00000000-0000-0000-0000-000000000000';

function isRecent(value: unknown): value is RecentPlace {
  if (typeof value !== 'object' || value === null) return false;
  const { id, kind } = value as Record<string, unknown>;
  return (
    typeof id === 'string' &&
    ID.test(id) &&
    id !== NIL &&
    typeof kind === 'string' &&
    KINDS.has(kind)
  );
}

const keyOf = (userId: string, workspaceId: string): string => `${KEY}${userId}:${workspaceId}`;

/** What the reader opened last in the workspace on this browser, the latest first. */
export function recentPlaces(userId: string, workspaceId: string): RecentPlace[] {
  try {
    const kept: unknown = JSON.parse(localStorage.getItem(keyOf(userId, workspaceId)) ?? '[]');
    return Array.isArray(kept) ? kept.filter(isRecent).slice(0, RECENT_KEPT) : [];
  } catch {
    // No storage here, or nothing of ours in it: nothing was kept.
    return [];
  }
}

/** Keeps the place as the latest opened; what is no place is not kept. */
export function rememberRecent(userId: string, workspaceId: string, place: RecentPlace): void {
  if (!isRecent(place)) return;
  const kept = recentPlaces(userId, workspaceId).filter((one) => one.id !== place.id);
  const latest = [{ id: place.id, kind: place.kind }, ...kept].slice(0, RECENT_KEPT);
  try {
    localStorage.setItem(keyOf(userId, workspaceId), JSON.stringify(latest));
  } catch {
    // Without storage the search starts without them.
  }
}

/** Forgets what every reader opened on this browser. */
export function forgetRecentPlaces(): void {
  try {
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith(KEY)) localStorage.removeItem(key);
    }
  } catch {
    // Without storage nothing was kept.
  }
}

/** The place the route opens, to keep: a board, wiki or course, or its card, page or unit. A diary's is none. */
export function recentOfRoute(route: Route): RecentPlace | undefined {
  if (route.kind === 'board') {
    return route.cardId
      ? { id: route.cardId, kind: 'card' }
      : { id: route.boardId, kind: 'container' };
  }
  if (route.kind === 'wiki') {
    return route.pageId
      ? { id: route.pageId, kind: 'page' }
      : { id: route.wikiId, kind: 'container' };
  }
  if (route.kind === 'course') {
    return route.unitId
      ? { id: route.unitId, kind: 'unit' }
      : { id: route.courseId, kind: 'container' };
  }
  return undefined;
}

/**
 * What the reader opened last in the workspace, as they may read it now, in
 * the shape of a star: a board, wiki or course as the sidebar has it, a card,
 * page or unit as a reference resolves it.
 */
export async function shownRecents(workspace: Workspace): Promise<Favorite[]> {
  const workspaceId = workspace.info?.id;
  const meId = workspace.me?.id;
  if (!workspaceId || !meId) return [];
  const kept = recentPlaces(meId, workspaceId);
  const blockIds = kept.filter((place) => place.kind !== 'container').map((place) => place.id);
  const linked =
    blockIds.length === 0
      ? []
      : await workspace.backend.references.resolve(workspaceId, blockIds).catch(() => []);
  const blocks = new Map(linked.map((block) => [block.id, block]));
  return kept.flatMap((place): Favorite[] => {
    if (place.kind === 'container') {
      const container = workspace.containers.find((one) => one.id === place.id);
      if (!container || container.kind === 'diary' || container.isTemplate) return [];
      const { id, title } = container;
      const [icon, containerKind] = [container.icon ?? '', container.kind ?? 'board'];
      return [{ targetId: id, kind: 'container', title, icon, containerId: id, containerKind }];
    }
    const block = blocks.get(place.id);
    if (!block) return [];
    const { id, kind, title, icon, number, containerId, containerKind } = block;
    if (kind === 'view') return [];
    return [
      {
        targetId: id,
        kind,
        title,
        icon,
        ...(number === undefined ? {} : { number }),
        containerId,
        containerKind,
      },
    ];
  });
}
