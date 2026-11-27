/**
 * Maps a personal `TimeEntry`'s target snapshot to an app route and navigates
 * there safely (card, course unit, board, course, or `/me/time` when targetless).
 */
import type { TimeEntry } from '$lib/state/index.ts';

import { goToPlace, routeOfPlace, type Place } from './places.ts';
import { placeRoute, type Router } from './router.svelte.ts';

function placeOfEntry(entry: TimeEntry): Place | undefined {
  if (entry.blockId && entry.containerId) {
    if (entry.targetKind === 'card') {
      return {
        kind: 'card',
        id: entry.blockId,
        containerId: entry.containerId,
        workspaceId: entry.workspaceId ?? undefined,
      };
    }
    if (entry.targetKind === 'unit') {
      return {
        kind: 'unit',
        id: entry.blockId,
        containerId: entry.containerId,
        workspaceId: entry.workspaceId ?? undefined,
      };
    }
  }
  return undefined;
}

/** Returns a clickable href for a time entry's target, or `undefined` when targetless. */
export function timeEntryTargetHref(router: Router, entry: TimeEntry): string | undefined {
  const place = placeOfEntry(entry);
  if (place) return router.href(routeOfPlace(router, place));
  if (
    entry.workspaceId &&
    entry.containerId &&
    (entry.targetKind === 'board' || entry.targetKind === 'course')
  ) {
    return router.href(
      placeRoute({
        id: entry.containerId,
        workspaceId: entry.workspaceId,
        kind: entry.targetKind,
      }),
    );
  }
  return undefined;
}

/** Navigates to the entry's target (or `/me/time` when targetless). */
export function openTimeEntryTarget(router: Router, entry: TimeEntry): void {
  const place = placeOfEntry(entry);
  if (place) {
    goToPlace(router, place);
    return;
  }
  if (
    entry.workspaceId &&
    entry.containerId &&
    (entry.targetKind === 'board' || entry.targetKind === 'course')
  ) {
    router.openContainer({
      id: entry.containerId,
      workspaceId: entry.workspaceId,
      kind: entry.targetKind,
    });
    return;
  }
  router.go({ meTime: true });
}
