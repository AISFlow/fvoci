/**
 * Pure URL route parsing and formatting for the app (`router.svelte.ts`),
 * modeled as a `kind`-discriminated union (`Route`).
 */
import { readBasePath } from '$lib/chunk-reload.svelte.ts';
import { buildUrl, type ContainerPlace } from '$lib/logic/urls.ts';

export { readBasePath };

export type CourseTab = 'units' | 'meetings' | 'exams';

export type Route =
  | { readonly kind: 'home' }
  | { readonly kind: 'unknown'; readonly shared?: true }
  | { readonly kind: 'passwordForgot' }
  | { readonly kind: 'passwordReset' }
  | { readonly kind: 'emailVerify' }
  | { readonly kind: 'emailConfirm' }
  | { readonly kind: 'emailRevert' }
  | { readonly kind: 'invite'; readonly invitation?: string }
  | { readonly kind: 'org'; readonly orgId: string }
  | { readonly kind: 'meTime' }
  | { readonly kind: 'todayHub' }
  | {
      readonly kind: 'shared';
      readonly boardId: string;
      readonly viewId?: string;
      readonly cardId?: string;
      readonly readToken: string;
    }
  | { readonly kind: 'workspace'; readonly workspaceId: string }
  | {
      readonly kind: 'board';
      readonly workspaceId: string;
      readonly boardId: string;
      readonly viewId?: string;
      readonly cardId?: string;
    }
  | {
      readonly kind: 'wiki';
      readonly workspaceId: string;
      readonly wikiId: string;
      readonly pageId?: string;
    }
  | {
      readonly kind: 'diary';
      readonly workspaceId: string;
      readonly diaryId: string;
      readonly monthDay?: string;
      readonly year?: string;
    }
  | {
      readonly kind: 'course';
      readonly workspaceId: string;
      readonly courseId: string;
      readonly unitId?: string;
      readonly courseTab?: Exclude<CourseTab, 'units'>;
      readonly meetingId?: string;
    }
  | { readonly kind: 'learningToday'; readonly workspaceId: string }
  | { readonly kind: 'reviewsToday'; readonly workspaceId: string };

/**
 * Shorthand accepted by `settled`, `routePath`, and `Router` navigation methods
 * alongside any concrete `Route` variant.
 */
export interface RouteFields {
  readonly kind?: Route['kind'] | undefined;
  readonly passwordReset?: boolean | undefined;
  readonly passwordForgot?: boolean | undefined;
  readonly emailVerify?: boolean | undefined;
  readonly emailConfirm?: boolean | undefined;
  readonly emailRevert?: boolean | undefined;
  readonly orgId?: string | undefined;
  readonly meTime?: boolean | undefined;
  readonly todayHub?: boolean | undefined;
  readonly workspaceId?: string | undefined;
  readonly boardId?: string | undefined;
  readonly viewId?: string | undefined;
  readonly cardId?: string | undefined;
  readonly wikiId?: string | undefined;
  readonly pageId?: string | undefined;
  readonly diaryId?: string | undefined;
  readonly monthDay?: string | undefined;
  readonly year?: string | undefined;
  readonly courseId?: string | undefined;
  readonly unitId?: string | undefined;
  readonly courseTab?: Exclude<CourseTab, 'units'> | undefined;
  readonly meetingId?: string | undefined;
  readonly reviewsToday?: boolean | undefined;
  readonly learningToday?: boolean | undefined;
  readonly invitation?: string | undefined;
  readonly readToken?: string | undefined;
  readonly shared?: boolean | undefined;
}

export type RouteTarget = Route | RouteFields;

export const NOWHERE: Route = { kind: 'home' };

/**
 * In the place of a view in an address, where the view is not known (a card
 * of a board that is not open): the page shows the board's first view, as
 * for any view that is not there, and keeps the card.
 */
export const ANY_VIEW = '-';

/** Whether `route` is under `/shared` (valid or malformed), shown by `SharedApp`. */
export function isSharedRoute(route: Route): boolean {
  return route.kind === 'shared' || (route.kind === 'unknown' && route.shared === true);
}

/** The workspace `route` is scoped to, if any. */
export function workspaceIdOf(route: Route): string | undefined {
  switch (route.kind) {
    case 'workspace':
    case 'board':
    case 'wiki':
    case 'diary':
    case 'course':
    case 'learningToday':
    case 'reviewsToday':
      return route.workspaceId;
    default:
      return undefined;
  }
}

/** The board, wiki, diary or course `route` shows. */
export function shownContainerId(route: Route): string | undefined {
  switch (route.kind) {
    case 'board':
    case 'shared':
      return route.boardId;
    case 'wiki':
      return route.wikiId;
    case 'diary':
      return route.diaryId;
    case 'course':
      return route.courseId;
    default:
      return undefined;
  }
}

/** The card `route` opens over a board, if any. */
export function cardIdOf(route: Route): string | undefined {
  return route.kind === 'board' || route.kind === 'shared' ? route.cardId : undefined;
}

/** A four-digit year (`0001`–`9999`) for a diary page's `?year=YYYY` query; undefined for anything else. */
function diaryYear(raw: string | null | undefined): string | undefined {
  return raw && /^\d{4}$/.test(raw) && raw !== '0000' ? raw : undefined;
}

/** The route of a public link: a board, then a view and a card, and the read token. */
function sharedRoute(segments: string[], search: string): Route {
  const [boardId, viewId, cardId, ...rest] = segments;
  const readToken = new URLSearchParams(search).get('r');
  if (boardId === undefined || readToken === null || readToken === '' || rest.length > 0) {
    return { kind: 'unknown', shared: true };
  }
  return {
    kind: 'shared',
    boardId,
    ...(viewId !== undefined ? { viewId } : {}),
    ...(cardId !== undefined ? { cardId } : {}),
    readToken,
  };
}

/**
 * The route an address stands for. `pathname` is the whole path, base path
 * included; `search` the query, where a public link has its read token.
 */
export function parseRoute(pathname: string, basePath: string, search = ''): Route {
  const prefix = basePath === '' ? '' : `/${basePath}`;
  if (prefix !== '' && pathname !== prefix && !pathname.startsWith(`${prefix}/`)) {
    return { kind: 'unknown' };
  }
  const segments = pathname
    .slice(prefix.length)
    .split('/')
    .filter((segment) => segment !== '')
    .map((segment) => {
      try {
        return decodeURIComponent(segment);
      } catch {
        return segment;
      }
    });
  if (segments.length === 0) return NOWHERE;
  if (segments[0] === 'forgot-password' && segments.length === 1) return { kind: 'passwordForgot' };
  if (segments[0] === 'reset-password' && segments.length === 1) return { kind: 'passwordReset' };
  if (segments[0] === 'verify-email' && segments.length === 1) return { kind: 'emailVerify' };
  if (segments[0] === 'confirm-email' && segments.length === 1) return { kind: 'emailConfirm' };
  if (segments[0] === 'revert-email' && segments.length === 1) return { kind: 'emailRevert' };
  if (segments[0] === 'invite') {
    if (segments.length === 1) return { kind: 'invite' };
    if (segments.length === 2) return { kind: 'invite', invitation: segments[1] };
    return { kind: 'unknown' };
  }
  if (segments[0] === 'shared') return sharedRoute(segments.slice(1), search);
  if (segments[0] === 'org') {
    return segments.length === 2 ? { kind: 'org', orgId: segments[1] ?? '' } : { kind: 'unknown' };
  }
  if (segments[0] === 'me' && segments[1] === 'time' && segments.length === 2) {
    return { kind: 'meTime' };
  }
  if (segments[0] === 'today' && segments.length === 1) {
    return { kind: 'todayHub' };
  }
  const [w, workspaceId, section, boardId, viewId, cardId, ...rest] = segments;
  if (w !== 'w' || workspaceId === undefined) return { kind: 'unknown' };
  if (section === 'wikis' && boardId !== undefined && cardId === undefined) {
    return {
      kind: 'wiki',
      workspaceId,
      wikiId: boardId,
      ...(viewId !== undefined ? { pageId: viewId } : {}),
    };
  }
  if (section === 'diaries' && boardId !== undefined && cardId === undefined) {
    const rawYear = viewId !== undefined ? new URLSearchParams(search).get('year') : null;
    const year = diaryYear(rawYear);
    return {
      kind: 'diary',
      workspaceId,
      diaryId: boardId,
      ...(viewId !== undefined ? { monthDay: viewId } : {}),
      ...(year ? { year } : {}),
    };
  }
  if (section === 'courses' && boardId !== undefined && cardId === undefined) {
    const params = new URLSearchParams(search);
    const rawTab = params.get('tab');
    const courseTab = rawTab === 'meetings' || rawTab === 'exams' ? rawTab : undefined;
    const rawMeeting = courseTab === 'meetings' ? params.get('meeting') : null;
    const meetingId = rawMeeting ? rawMeeting : undefined;
    return {
      kind: 'course',
      workspaceId,
      courseId: boardId,
      ...(viewId !== undefined ? { unitId: viewId } : {}),
      ...(courseTab ? { courseTab } : {}),
      ...(meetingId ? { meetingId } : {}),
    };
  }
  if (section === 'learning' && boardId === 'today' && viewId === undefined) {
    return { kind: 'learningToday', workspaceId };
  }
  if (
    section === 'learning' &&
    boardId === 'reviews' &&
    viewId === 'today' &&
    cardId === undefined
  ) {
    return { kind: 'reviewsToday', workspaceId };
  }
  if ((section !== undefined && section !== 'boards') || rest.length > 0) {
    return { kind: 'unknown' };
  }
  if (boardId === undefined) return { kind: 'workspace', workspaceId };
  return {
    kind: 'board',
    workspaceId,
    boardId,
    ...(viewId !== undefined ? { viewId } : {}),
    ...(cardId !== undefined ? { cardId } : {}),
  };
}

/**
 * Normalizes a `RouteTarget` so each part stands only under the one before it
 * (a card only on a view, a view only on a board, a board only in a workspace),
 * and standalone screens stand alone.
 */
export function settled(target: RouteTarget): Route {
  const route = target as RouteFields;
  if (route.kind === 'passwordForgot' || route.passwordForgot) return { kind: 'passwordForgot' };
  if (route.kind === 'passwordReset' || route.passwordReset) return { kind: 'passwordReset' };
  if (route.kind === 'emailVerify' || route.emailVerify) return { kind: 'emailVerify' };
  if (route.kind === 'emailConfirm' || route.emailConfirm) return { kind: 'emailConfirm' };
  if (route.kind === 'emailRevert' || route.emailRevert) return { kind: 'emailRevert' };
  if (route.kind === 'invite' || route.invitation !== undefined) {
    return route.invitation ? { kind: 'invite', invitation: route.invitation } : { kind: 'invite' };
  }
  if (route.orgId) return { kind: 'org', orgId: route.orgId };
  if (route.kind === 'meTime' || route.meTime) return { kind: 'meTime' };
  if (route.kind === 'todayHub' || route.todayHub) return { kind: 'todayHub' };
  const readToken = route.readToken || undefined;
  if (readToken) {
    if (!route.boardId) return NOWHERE;
    const viewId = route.viewId || undefined;
    const cardId = viewId ? route.cardId || undefined : undefined;
    return {
      kind: 'shared',
      boardId: route.boardId,
      ...(viewId ? { viewId } : {}),
      ...(cardId ? { cardId } : {}),
      readToken,
    };
  }
  const workspaceId = route.workspaceId || undefined;
  if (!workspaceId) return NOWHERE;
  if (route.wikiId) {
    const pageId = route.pageId || undefined;
    return { kind: 'wiki', workspaceId, wikiId: route.wikiId, ...(pageId ? { pageId } : {}) };
  }
  if (route.diaryId) {
    const monthDay = route.monthDay || undefined;
    const year = monthDay ? diaryYear(route.year) : undefined;
    return {
      kind: 'diary',
      workspaceId,
      diaryId: route.diaryId,
      ...(monthDay ? { monthDay } : {}),
      ...(year ? { year } : {}),
    };
  }
  if (route.courseId) {
    const courseTab =
      route.courseTab === 'meetings' || route.courseTab === 'exams' ? route.courseTab : undefined;
    const meetingId = courseTab === 'meetings' ? route.meetingId || undefined : undefined;
    const unitId = route.unitId || undefined;
    return {
      kind: 'course',
      workspaceId,
      courseId: route.courseId,
      ...(unitId ? { unitId } : {}),
      ...(courseTab ? { courseTab } : {}),
      ...(meetingId ? { meetingId } : {}),
    };
  }
  if (route.kind === 'reviewsToday' || route.reviewsToday) {
    return { kind: 'reviewsToday', workspaceId };
  }
  if (route.kind === 'learningToday' || route.learningToday) {
    return { kind: 'learningToday', workspaceId };
  }
  if (route.boardId) {
    const viewId = route.viewId || undefined;
    const cardId = viewId ? route.cardId || undefined : undefined;
    return {
      kind: 'board',
      workspaceId,
      boardId: route.boardId,
      ...(viewId ? { viewId } : {}),
      ...(cardId ? { cardId } : {}),
    };
  }
  return { kind: 'workspace', workspaceId };
}

/** The route of a board, wiki, diary or course (`kind`). */
export function placeRoute(container: ContainerPlace): Route {
  const { workspaceId, id } = container;
  if (container.kind === 'wiki') return { kind: 'wiki', workspaceId, wikiId: id };
  if (container.kind === 'diary') return { kind: 'diary', workspaceId, diaryId: id };
  if (container.kind === 'course') return { kind: 'course', workspaceId, courseId: id };
  return { kind: 'board', workspaceId, boardId: id };
}

function encodeParts(parts: readonly (string | undefined)[]): string[] {
  return parts.filter((part): part is string => part !== undefined).map(encodeURIComponent);
}

/** The address of a route, from the root of the site. */
export function routePath(target: RouteTarget, basePath: string): string {
  const next = settled(target);
  switch (next.kind) {
    case 'home':
    case 'unknown':
      return buildUrl('/', basePath);
    case 'passwordForgot':
      return buildUrl('/forgot-password', basePath);
    case 'passwordReset':
      return buildUrl('/reset-password', basePath);
    case 'emailVerify':
      return buildUrl('/verify-email', basePath);
    case 'emailConfirm':
      return buildUrl('/confirm-email', basePath);
    case 'emailRevert':
      return buildUrl('/revert-email', basePath);
    case 'invite':
      return buildUrl(
        next.invitation ? `/invite/${encodeURIComponent(next.invitation)}` : '/invite',
        basePath,
      );
    case 'org':
      return buildUrl(`/org/${encodeURIComponent(next.orgId)}`, basePath);
    case 'meTime':
      return buildUrl('/me/time', basePath);
    case 'todayHub':
      return buildUrl('/today', basePath);
    case 'shared': {
      const path = ['shared', ...encodeParts([next.boardId, next.viewId, next.cardId])].join('/');
      return `${buildUrl(`/${path}`, basePath)}?r=${encodeURIComponent(next.readToken)}`;
    }
    case 'workspace':
      return buildUrl(`/w/${encodeURIComponent(next.workspaceId)}`, basePath);
    case 'reviewsToday':
      return buildUrl(
        `/w/${encodeURIComponent(next.workspaceId)}/learning/reviews/today`,
        basePath,
      );
    case 'learningToday':
      return buildUrl(`/w/${encodeURIComponent(next.workspaceId)}/learning/today`, basePath);
    case 'board': {
      const rest = encodeParts([next.boardId, next.viewId, next.cardId]);
      return buildUrl(
        `/w/${encodeURIComponent(next.workspaceId)}/boards/${rest.join('/')}`,
        basePath,
      );
    }
    case 'wiki': {
      const rest = encodeParts([next.wikiId, next.pageId]);
      return buildUrl(
        `/w/${encodeURIComponent(next.workspaceId)}/wikis/${rest.join('/')}`,
        basePath,
      );
    }
    case 'diary': {
      const rest = encodeParts([next.diaryId, next.monthDay]);
      const url = buildUrl(
        `/w/${encodeURIComponent(next.workspaceId)}/diaries/${rest.join('/')}`,
        basePath,
      );
      return next.monthDay && next.year ? `${url}?year=${encodeURIComponent(next.year)}` : url;
    }
    case 'course': {
      const rest = encodeParts([next.courseId, next.unitId]);
      const url = buildUrl(
        `/w/${encodeURIComponent(next.workspaceId)}/courses/${rest.join('/')}`,
        basePath,
      );
      if (!next.courseTab) return url;
      const focus =
        next.courseTab === 'meetings' && next.meetingId
          ? `&meeting=${encodeURIComponent(next.meetingId)}`
          : '';
      return `${url}?tab=${encodeURIComponent(next.courseTab)}${focus}`;
    }
  }
}

/** Structural equality over discriminated `Route` variants via normalized path+query. */
export function sameRoute(a: Route, b: Route): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'unknown' && b.kind === 'unknown') return a.shared === b.shared;
  return routePath(a, '') === routePath(b, '');
}
