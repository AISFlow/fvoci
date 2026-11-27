/**
 * Where a reference in a body leads, by the address bar: a card of the board
 * that is shown opens over the view that is shown, a card of another board
 * on that board's first view, and a page in its wiki. A reader by a link
 * reads one board, and nothing else has an address there.
 */
import { referenceAt } from '$lib/logic/reference-address.ts';
import type { LinkedBlock, ReferencePlaces } from '$lib/state/index.ts';

import {
  ANY_VIEW,
  isSharedRoute,
  workspaceIdOf,
  type RouteTarget,
  type Router,
} from './router.svelte.ts';

/** The route that shows what is named; none for what has no screen. */
function routeOf(router: Router, linked: LinkedBlock): RouteTarget | undefined {
  const { route } = router;
  if (
    linked.kind === 'card' &&
    (route.kind === 'board' || route.kind === 'shared') &&
    linked.containerId === route.boardId
  ) {
    return { ...route, cardId: linked.id };
  }
  if (isSharedRoute(route)) return undefined;
  const workspaceId = workspaceIdOf(route);
  if (linked.kind === 'page') return { workspaceId, wikiId: linked.containerId, pageId: linked.id };
  if (linked.kind === 'unit') {
    return { workspaceId, courseId: linked.containerId, unitId: linked.id };
  }
  return { workspaceId, boardId: linked.containerId, viewId: ANY_VIEW, cardId: linked.id };
}

export function referencePlaces(router: Router): ReferencePlaces {
  return {
    href: (linked) => {
      const route = routeOf(router, linked);
      return route ? router.href(route) : undefined;
    },
    open: (linked) => {
      const route = routeOf(router, linked);
      if (!route) return;
      const now = router.route;
      if (
        linked.kind === 'card' &&
        (now.kind === 'board' || now.kind === 'shared') &&
        linked.containerId === now.boardId
      ) {
        router.openCard(linked.id);
      } else if (
        linked.kind === 'page' &&
        now.kind === 'wiki' &&
        linked.containerId === now.wikiId
      ) {
        router.openPage(linked.id);
      } else if (
        linked.kind === 'unit' &&
        now.kind === 'course' &&
        linked.containerId === now.courseId
      ) {
        router.openUnit(linked.id);
      } else router.go(route);
    },
    named: (address) =>
      referenceAt(address, {
        origin: location.origin,
        basePath: router.basePath,
        workspaceId: workspaceIdOf(router.route),
      }),
  };
}
