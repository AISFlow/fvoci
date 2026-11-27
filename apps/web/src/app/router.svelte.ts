/**
 * The address bar: which workspace, board, view and card are shown.
 *
 * Pure route parsing and formatting live in `route-path.ts`; this module holds
 * the reactive `Router` synced with `history` and `popstate`.
 */
import { takeFragmentToken, takeInviteToken } from '$lib/fragment-token.ts';
import { afterOwnSteps, releaseOverlayEntries, stepBack } from '$lib/layout/back.svelte.ts';
import { buildUrl, type ContainerPlace } from '$lib/logic/urls.ts';

import {
  ANY_VIEW,
  cardIdOf,
  isSharedRoute,
  NOWHERE,
  parseRoute,
  placeRoute,
  readBasePath,
  routePath,
  sameRoute,
  settled,
  shownContainerId,
  workspaceIdOf,
  type CourseTab,
  type Route,
  type RouteFields,
  type RouteTarget,
} from './route-path.ts';

export {
  ANY_VIEW,
  cardIdOf,
  isSharedRoute,
  parseRoute,
  placeRoute,
  readBasePath,
  routePath,
  sameRoute,
  shownContainerId,
  workspaceIdOf,
  type CourseTab,
  type Route,
  type RouteFields,
  type RouteTarget,
};

function fragmentScopeOf(route: Route): string | undefined {
  if (route.kind === 'passwordReset') return 'reset-password';
  if (route.kind === 'emailVerify') return 'verify-email';
  if (route.kind === 'emailConfirm') return 'confirm-email';
  if (route.kind === 'emailRevert') return 'revert-email';
  return undefined;
}

export class Router {
  readonly basePath: string;
  route = $state.raw<Route>(NOWHERE);
  /** Invitation token captured and stripped from `/invite/<token>` once on route settlement. */
  invitation = $state('');
  /**
   * One-time `#token` captured and stripped on auth screens (`passwordReset`,
   * `emailVerify`, `emailConfirm`, `emailRevert`). Wrapped in a fresh object
   * per link opening so `{#key router.fragment}` remounts even when the same
   * link is opened a second time in the same tab.
   */
  fragment = $state.raw<{ readonly token: string }>({ token: '' });
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- callback list, not rendered
  readonly #fragmentListeners = new Set<(opened: { readonly token: string }) => void>();
  /** Whether the card on screen was opened from the board in this visit: then "back" closes it. */
  #cardEntered = false;
  /** Addresses that `go` has yet to write to the history, and the last of them. */
  #writes = 0;
  #heading: string | undefined;

  constructor(basePath: string) {
    this.basePath = basePath;
    const initial = parseRoute(location.pathname, basePath, location.search);
    this.route = this.#captureTokens(initial, true);
  }

  /** Subscribes to new `#token` arrivals on the active auth screen (`popstate` / `hashchange`). */
  onFragment(listener: (opened: { readonly token: string }) => void): () => void {
    this.#fragmentListeners.add(listener);
    return () => {
      this.#fragmentListeners.delete(listener);
    };
  }

  #takeRouteFragment(scope: string): void {
    const opened = { token: takeFragmentToken(scope) };
    this.fragment = opened;
    for (const listener of this.#fragmentListeners) listener(opened);
  }

  /**
   * Moves one-time URL tokens (`#token` on auth screens, `/invite/<token>`) out
   * of the address bar immediately when a route settles, before any lazy chunk
   * renders (`S10`), so components only read reactive state without `$derived` side effects.
   */
  #captureTokens(route: Route, initial = false): Route {
    if (route.kind === 'invite') {
      this.invitation = takeInviteToken(
        route.invitation,
        routePath({ kind: 'invite' }, this.basePath),
      );
      return { kind: 'invite' };
    }
    const scope = fragmentScopeOf(route);
    if (scope !== undefined && (initial || location.hash.length > 1)) {
      this.#takeRouteFragment(scope);
    }
    return route;
  }

  /** Starts following the back, forward, and same-document fragment navigations. */
  listen(): () => void {
    const onPopState = (): void => {
      if (this.#writes > 0) return;
      const parsed = parseRoute(location.pathname, this.basePath, location.search);
      const next = this.#captureTokens(parsed);
      const now = this.route;
      if (sameRoute(next, now)) return;
      if (cardIdOf(next) !== cardIdOf(now)) this.#cardEntered = false;
      this.route = next;
    };
    const onHashChange = (): void => {
      const scope = fragmentScopeOf(this.route);
      if (scope !== undefined && location.hash.length > 1) {
        this.#takeRouteFragment(scope);
      }
    };
    window.addEventListener('popstate', onPopState);
    window.addEventListener('hashchange', onHashChange);
    return () => {
      window.removeEventListener('popstate', onPopState);
      window.removeEventListener('hashchange', onHashChange);
    };
  }

  href(route: RouteTarget): string {
    return routePath(route, this.basePath);
  }

  /** The address of the page of an invitation, by its token. */
  inviteHref(token: string): string {
    return buildUrl(`/invite/${encodeURIComponent(token)}`, this.basePath);
  }

  /** Loads the page anew at a route (signing out, another account signing in). */
  restart(route: RouteTarget, options: { replace?: boolean; query?: string } = {}): void {
    const released = releaseOverlayEntries();
    const address = this.href(route) + (options.query ? `?${options.query}` : '');
    if (released > 1) {
      stepBack(released - 1);
      afterOwnSteps(() => {
        window.location.replace(address);
      });
      return;
    }
    if (released > 0 || options.replace) window.location.replace(address);
    else window.location.assign(address);
  }

  /** Goes to a route. `replace` puts it in place of the current entry instead of after it. */
  go(route: RouteTarget, options: { replace?: boolean } = {}): void {
    const next = this.#captureTokens(settled(route));
    const path = routePath(next, this.basePath);
    if (isSharedRoute(next) !== isSharedRoute(this.route)) {
      window.location.assign(path);
      return;
    }
    const here = `${location.pathname}${location.search}`;
    const shown = this.#writes > 0 ? this.#heading : here;
    if (path !== shown) {
      this.#writes += 1;
      this.#heading = path;
      afterOwnSteps(() => {
        const released = releaseOverlayEntries();
        if (released > 1) {
          stepBack(released - 1);
          afterOwnSteps(() => {
            this.#writes -= 1;
            history.replaceState(null, '', path);
          });
          return;
        }
        this.#writes -= 1;
        if (options.replace || released > 0) history.replaceState(null, '', path);
        else history.pushState(null, '', path);
      });
    }
    this.route = next;
  }

  openContainer(container: ContainerPlace): void {
    this.#cardEntered = false;
    this.go(placeRoute(container));
  }

  openPage(pageId: string | undefined, options: { replace?: boolean } = {}): void {
    if (this.route.kind !== 'wiki') return;
    this.go({ workspaceId: this.route.workspaceId, wikiId: this.route.wikiId, pageId }, options);
  }

  openDay(monthDay: string, options: { replace?: boolean } = {}): void {
    if (this.route.kind !== 'diary') return;
    this.go(
      { workspaceId: this.route.workspaceId, diaryId: this.route.diaryId, monthDay },
      options,
    );
  }

  openUnit(unitId: string | undefined, options: { replace?: boolean } = {}): void {
    if (this.route.kind !== 'course') return;
    this.go(
      { workspaceId: this.route.workspaceId, courseId: this.route.courseId, unitId },
      options,
    );
  }

  /** Switches the active tab of the course shown (`units` clears `?tab=`). */
  openCourseTab(tab: CourseTab, options: { meetingId?: string; replace?: boolean } = {}): void {
    if (this.route.kind !== 'course') return;
    this.go(
      {
        workspaceId: this.route.workspaceId,
        courseId: this.route.courseId,
        unitId: this.route.unitId,
        ...(tab === 'units' ? {} : { courseTab: tab }),
        ...(tab === 'meetings' && options.meetingId ? { meetingId: options.meetingId } : {}),
      },
      options,
    );
  }

  openView(viewId: string): void {
    this.#cardEntered = false;
    if (this.route.kind === 'shared') {
      this.go({ boardId: this.route.boardId, viewId, readToken: this.route.readToken });
    } else if (this.route.kind === 'board') {
      this.go({ workspaceId: this.route.workspaceId, boardId: this.route.boardId, viewId });
    }
  }

  openCard(cardId: string): void {
    if (this.route.kind !== 'board' && this.route.kind !== 'shared') return;
    const replacing = this.route.cardId !== undefined;
    this.go({ ...this.route, cardId }, { replace: replacing });
    if (!replacing) this.#cardEntered = true;
  }

  closeCard(): void {
    if (this.route.kind !== 'board' && this.route.kind !== 'shared') return;
    if (this.route.cardId === undefined) return;
    const withoutCard = settled({ ...this.route, cardId: undefined });
    if (this.#cardEntered) {
      this.#cardEntered = false;
      this.route = withoutCard;
      this.#writes += 1;
      this.#heading = routePath(withoutCard, this.basePath);
      afterOwnSteps(() => {
        this.#writes -= 1;
        stepBack();
      });
    } else {
      this.go(withoutCard, { replace: true });
    }
  }
}
