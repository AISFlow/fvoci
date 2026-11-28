/**
 * The workspace each user was last in on this browser, so that the root
 * address takes them back to it. Kept per user: someone else signing in
 * here is not taken to (nor told of) a workspace of the one before.
 *
 * Only the identifier is kept. Whether it is still theirs is the backend's
 * say (`Workspace.load` enters it only when it is among their workspaces).
 */
import { QUICK_FILTERS_KEY, quickFilters } from '$lib/state/board-quick.svelte.ts';

import { forgetRecentPlaces } from './recent-places.ts';

const KEY = 'fvoci:last-workspace:';

export function lastWorkspace(userId: string): string | undefined {
  try {
    return localStorage.getItem(`${KEY}${userId}`) ?? undefined;
  } catch {
    // No storage here (blocked, or a private window): the first workspace then.
    return undefined;
  }
}

export function rememberWorkspace(userId: string, workspaceId: string): void {
  try {
    localStorage.setItem(`${KEY}${userId}`, workspaceId);
  } catch {
    // Without storage the choice lasts for the visit.
  }
}

const PLACE_KEY = 'fvoci:last-place:';

/** The place remembered for the account's hub of today, opened last from that workspace. */
export const TODAY_PLACE = 'today';

/**
 * The board or the wiki each user was last on in a workspace on this
 * browser, or the hub of today (`TODAY_PLACE`): the workspace's address
 * alone takes them back to it, if the sidebar still has it
 * (`WorkspacePane.svelte`).
 */
export function lastPlace(userId: string, workspaceId: string): string | undefined {
  try {
    return localStorage.getItem(`${PLACE_KEY}${userId}:${workspaceId}`) ?? undefined;
  } catch {
    // No storage here: the first of the sidebar then.
    return undefined;
  }
}

export function rememberPlace(userId: string, workspaceId: string, boardId: string): void {
  try {
    localStorage.setItem(`${PLACE_KEY}${userId}:${workspaceId}`, boardId);
  } catch {
    // Without storage the place lasts for the visit.
  }
}

/**
 * Clears unscoped UI state in `localStorage` on sign-out while preserving per-user workspace memory.
 * The quick filters go too, from storage and from the page: they narrow what the next reader sees;
 * and what each reader opened last, which the search starts with (`recent-places.ts`).
 */
export function forgetUnscoped(): void {
  quickFilters.forget();
  forgetRecentPlaces();
  try {
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith('fvoci.calendar.scale.') || key.startsWith(QUICK_FILTERS_KEY)) {
        localStorage.removeItem(key);
      }
    }
  } catch {
    // Without storage nothing was kept.
  }
}

/** Forgets every account-scoped identifier kept on this browser for `userId` (on account deletion). */
export function forgetUser(userId: string): void {
  if (!userId) return;
  forgetRecentPlaces();
  try {
    for (const key of Object.keys(localStorage)) {
      if (
        key === `${KEY}${userId}` ||
        key.startsWith(`${PLACE_KEY}${userId}:`) ||
        key.startsWith(`fvoci:wiki-open:${userId}:`) ||
        key.startsWith(`fvoci:course-open:${userId}:`) ||
        key.startsWith(`${QUICK_FILTERS_KEY}${userId}:`) ||
        key.startsWith('fvoci.calendar.scale.')
      ) {
        localStorage.removeItem(key);
      }
    }
  } catch {
    // Without storage nothing was kept.
  }
}
