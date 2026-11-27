/**
 * Going to a card or a page from outside its board or wiki: from the search,
 * from a notification. A card opens over the view of its board that is
 * shown, or else over the board's first view; a page at its wiki's address.
 */
import { parseCalendarDate } from '$lib/logic/dates.ts';
import type { ContainerKind } from '$lib/logic/wire.ts';

import type { RouteFields, Router } from './router.svelte.ts';

/**
 * The view segment of the address of a card on a board not shown: it names
 * no view, so the board page puts its first view in its place and keeps the
 * card (`BoardPage.svelte`).
 */
export const ANY_VIEW = '-';

/** A card, a page, a diary entry, a course unit, a course meeting, or a container itself, and where it is. */
export interface Place {
  kind: 'card' | 'page' | 'entry' | 'unit' | 'meeting' | 'container';
  id: string;
  containerId: string;
  containerKind?: ContainerKind;
  workspaceId: string | undefined;
  /** The entry's day (`YYYY-MM-DD`), for a diary entry. */
  title?: string;
}

/** The route of a card (over the view shown where its board is shown, else over any), a page, a diary entry, a course unit, a course meeting, or a container. */
export function routeOfPlace(router: Router, place: Place): RouteFields {
  const { workspaceId, containerId, id } = place;
  if (place.kind === 'container') {
    if (place.containerKind === 'wiki') return { workspaceId, wikiId: containerId };
    if (place.containerKind === 'course') return { workspaceId, courseId: containerId };
    return { workspaceId, boardId: containerId };
  }
  if (place.kind === 'meeting') {
    return { workspaceId, courseId: containerId, courseTab: 'meetings', meetingId: id };
  }
  if (place.kind === 'unit') return { workspaceId, courseId: containerId, unitId: id };
  if (place.kind === 'entry') {
    const date = parseCalendarDate(place.title ?? '');
    return {
      workspaceId,
      diaryId: containerId,
      monthDay: date ? date.slice(5) : (place.title ?? '').slice(5),
      ...(date ? { year: date.slice(0, 4) } : {}),
    };
  }
  if (place.kind === 'page') return { workspaceId, wikiId: containerId, pageId: id };
  const route = router.route;
  const viewId =
    route.kind === 'board' &&
    route.boardId === containerId &&
    route.viewId !== undefined &&
    route.viewId !== ANY_VIEW
      ? route.viewId
      : ANY_VIEW;
  return { workspaceId, boardId: containerId, viewId, cardId: id };
}

/**
 * Goes to a card, a page, a diary entry, or a course unit.
 */
export function goToPlace(router: Router, place: Place): void {
  const route = routeOfPlace(router, place);
  const now = router.route;
  if (place.kind === 'container') {
    router.openContainer({
      id: place.containerId,
      workspaceId: place.workspaceId ?? '',
      kind: place.containerKind ?? 'board',
    });
  } else if (place.kind === 'page' && now.kind === 'wiki' && now.wikiId === place.containerId) {
    router.openPage(place.id);
  } else if (
    place.kind === 'meeting' &&
    now.kind === 'course' &&
    now.courseId === place.containerId
  ) {
    router.openCourseTab('meetings', { meetingId: place.id });
  } else if (place.kind === 'unit' && now.kind === 'course' && now.courseId === place.containerId) {
    router.openUnit(place.id);
  } else if (place.kind === 'card' && route.viewId !== ANY_VIEW) {
    router.openCard(place.id);
  } else if (place.kind === 'entry' && now.kind === 'diary' && now.diaryId === place.containerId) {
    router.go(route);
  } else {
    const kind = (
      { card: 'board', page: 'wiki', entry: 'diary', unit: 'course', meeting: 'course' } as const
    )[place.kind];
    router.openContainer({ id: place.containerId, workspaceId: place.workspaceId ?? '', kind });
    router.go(route, { replace: true });
  }
}

export function focusThePlace(): void {
  const card = document.querySelector<HTMLElement>('[data-card-dialog]');
  (card ?? document.querySelector<HTMLElement>('main'))?.focus();
}
