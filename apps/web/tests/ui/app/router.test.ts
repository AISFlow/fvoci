/** Addresses: discriminated union `Route` parsing, formatting, and history navigation. */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as back from '$lib/layout/back.svelte.ts';

import {
  forgetFragmentToken,
  forgetInviteToken,
  readFragmentToken,
  takeInviteToken,
} from '$lib/fragment-token.ts';
import { localeOfSetting, matchLocale } from '../../../src/app/locale.ts';
import { goToPlace, routeOfPlace } from '../../../src/app/places.ts';
import {
  cardIdOf,
  isSharedRoute,
  parseRoute,
  placeRoute,
  readBasePath,
  routePath,
  Router,
  shownContainerId,
} from '../../../src/app/router.svelte.ts';

describe('reading an address', () => {
  it('reads workspace, board, view and card, each optional from the right', () => {
    expect(parseRoute('/', '')).toEqual({ kind: 'home' });
    expect(parseRoute('/w/t', '')).toEqual({ kind: 'workspace', workspaceId: 't' });
    expect(parseRoute('/w/t/boards', '')).toEqual({ kind: 'workspace', workspaceId: 't' });
    expect(parseRoute('/w/t/boards/b', '')).toEqual({
      kind: 'board',
      workspaceId: 't',
      boardId: 'b',
    });
    expect(parseRoute('/w/t/boards/b/v', '')).toEqual({
      kind: 'board',
      workspaceId: 't',
      boardId: 'b',
      viewId: 'v',
    });
    expect(parseRoute('/w/t/boards/b/v/c', '')).toEqual({
      kind: 'board',
      workspaceId: 't',
      boardId: 'b',
      viewId: 'v',
      cardId: 'c',
    });
    expect(parseRoute('/w/t/boards/b/v/', '')).toMatchObject({
      kind: 'board',
      boardId: 'b',
      viewId: 'v',
    });
  });

  it('reads a public link: the board, a view and a card, and the read token', () => {
    expect(parseRoute('/base/shared/b/v/c', 'base', '?r=tok')).toEqual({
      kind: 'shared',
      boardId: 'b',
      viewId: 'v',
      cardId: 'c',
      readToken: 'tok',
    });
    expect(parseRoute('/shared/b', '', '?r=a%2Fb')).toEqual({
      kind: 'shared',
      boardId: 'b',
      readToken: 'a/b',
    });
    expect(parseRoute('/shared/b', '').kind).toBe('unknown');
    expect(parseRoute('/shared/b', '', '?r=').kind).toBe('unknown');
    expect(parseRoute('/shared', '', '?r=t').kind).toBe('unknown');
    expect(parseRoute('/shared/b/v/c/x', '', '?r=t').kind).toBe('unknown');
    for (const [path, search] of [
      ['/shared/b', ''],
      ['/shared/b', '?r='],
      ['/shared', '?r=t'],
      ['/shared/b/v/c/x', '?r=t'],
    ] as const) {
      expect(isSharedRoute(parseRoute(path, '', search))).toBe(true);
    }
    expect(isSharedRoute(parseRoute('/w/t', ''))).toBe(false);
  });

  it('writes a public link under the base path, the token in the query, and compares readToken on go()', () => {
    const shared = { boardId: 'b', viewId: 'v', readToken: 't/1' };
    expect(routePath(shared, 'base')).toBe('/base/shared/b/v?r=t%2F1');
    expect(routePath({ ...shared, cardId: 'c' }, '')).toBe('/shared/b/v/c?r=t%2F1');
    expect(routePath({ ...shared, workspaceId: 'w' }, '')).toBe('/shared/b/v?r=t%2F1');

    history.replaceState(null, '', '/shared/b/v?r=tok1');
    const router = new Router('');
    router.go({ boardId: 'b', viewId: 'v', readToken: 'tok2' });
    expect(`${location.pathname}${location.search}`).toBe('/shared/b/v?r=tok2');
  });

  it("reads the organization's screen, and nothing below it", () => {
    expect(parseRoute('/org/o', '')).toEqual({ kind: 'org', orgId: 'o' });
    expect(parseRoute('/app/org/o', 'app')).toEqual({ kind: 'org', orgId: 'o' });
    expect(parseRoute('/org', '').kind).toBe('unknown');
    expect(parseRoute('/org/o/w', '').kind).toBe('unknown');
    expect(routePath({ orgId: 'o 1' }, 'app')).toBe('/app/org/o%201');
  });

  it('strips /invite/<token> from the URL immediately and restores from sessionStorage on reload', () => {
    sessionStorage.clear();
    history.replaceState(null, '', '/invite/secret-token');
    const initial = parseRoute(location.pathname, '');
    expect(initial).toEqual({ kind: 'invite', invitation: 'secret-token' });
    const taken = takeInviteToken(
      initial.kind === 'invite' ? initial.invitation : undefined,
      '/invite',
    );
    expect(taken).toBe('secret-token');
    expect(location.pathname).toBe('/invite');

    const reloaded = parseRoute(location.pathname, '');
    expect(reloaded).toEqual({ kind: 'invite' });
    expect(takeInviteToken(undefined, '/invite')).toBe('secret-token');
    forgetInviteToken();
    expect(takeInviteToken(undefined, '/invite')).toBe('');

    // Router itself strips both /invite/<token> and auth #hash tokens on construction (S10).
    history.replaceState(null, '', '/invite/router-secret');
    const inviteRouter = new Router('');
    expect(location.pathname).toBe('/invite');
    expect(inviteRouter.route).toEqual({ kind: 'invite' });
    expect(inviteRouter.invitation).toBe('router-secret');
    forgetInviteToken();

    history.replaceState(null, '', '/verify-email#verify-secret');
    const verifyRouter = new Router('');
    const stopVerify = verifyRouter.listen();
    const seenFragments: string[] = [];
    const offFragment = verifyRouter.onFragment(({ token }) => {
      seenFragments.push(token);
    });
    expect(verifyRouter.route).toEqual({ kind: 'emailVerify' });
    expect(verifyRouter.fragment).toEqual({ token: 'verify-secret' });
    expect(location.hash).toBe('');
    expect(readFragmentToken('verify-email')).toBe('verify-secret');

    // Same-document fragment navigation fires popstate BEFORE hashchange in HTML5:
    // Router captures & strips once, updates router.fragment to a fresh object, and notifies once.
    const firstOpened = verifyRouter.fragment;
    history.pushState(null, '', '/verify-email#second-secret');
    window.dispatchEvent(new PopStateEvent('popstate'));
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    expect(location.hash).toBe('');
    expect(verifyRouter.fragment).toEqual({ token: 'second-secret' });
    expect(verifyRouter.fragment).not.toBe(firstOpened);
    expect(seenFragments).toEqual(['second-secret']);

    // Re-opening the exact same fragment link again creates a new fragment object so {#key} remounts.
    const secondOpened = verifyRouter.fragment;
    history.pushState(null, '', '/verify-email#second-secret');
    window.dispatchEvent(new PopStateEvent('popstate'));
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    expect(verifyRouter.fragment).toEqual({ token: 'second-secret' });
    expect(verifyRouter.fragment).not.toBe(secondOpened);
    expect(seenFragments).toEqual(['second-secret', 'second-secret']);

    offFragment();
    stopVerify();
    forgetFragmentToken('verify-email');
  });

  it('does not know other addresses, the old ones among them', () => {
    expect(parseRoute('/login', '').kind).toBe('unknown');
    expect(parseRoute('/w', '').kind).toBe('unknown');
    expect(parseRoute('/w/t/bord/b', '').kind).toBe('unknown');
    expect(parseRoute('/w/t/boards/b/v/c/more', '').kind).toBe('unknown');
    expect(parseRoute(['', 'board', 'b', 'v', 'c'].join('/'), '').kind).toBe('unknown');
  });

  it('works under a base path, and knows nothing outside of it', () => {
    expect(parseRoute('/app', 'app')).toEqual({ kind: 'home' });
    expect(parseRoute('/app/w/t/boards/b', 'app')).toMatchObject({
      kind: 'board',
      workspaceId: 't',
      boardId: 'b',
    });
    expect(parseRoute('/elsewhere/w/t/boards/b', 'app').kind).toBe('unknown');
    expect(parseRoute('/appx/w/t/boards/b', 'app').kind).toBe('unknown');
  });

  it('decodes what the address encodes, and survives what does not decode', () => {
    expect(parseRoute('/w/a%2Fb/boards/a%20b', '')).toMatchObject({
      kind: 'board',
      workspaceId: 'a/b',
      boardId: 'a b',
    });
    const malformed = parseRoute('/w/t/boards/%E0%A4%A', '');
    expect(malformed).toMatchObject({ kind: 'board', boardId: '%E0%A4%A' });
  });
});

describe('writing an address', () => {
  it('is the reverse of reading one', () => {
    for (const path of [
      '/',
      '/w/t',
      '/w/t/boards/b',
      '/w/t/boards/b/v',
      '/w/t/boards/b/v/c',
      '/w/a%2Fb/boards/a%20b%3F/v%23',
    ]) {
      expect(routePath(parseRoute(path, ''), '')).toBe(path);
    }
    expect(routePath({ workspaceId: 't', boardId: 'b', viewId: 'v' }, 'app')).toBe(
      '/app/w/t/boards/b/v',
    );
    expect(routePath({}, 'app')).toBe('/app/');
  });

  it('drops what has nothing to stand under: a card without a view, a board without a workspace', () => {
    expect(routePath({ workspaceId: 't', boardId: 'b', cardId: 'c' }, '')).toBe('/w/t/boards/b');
    expect(routePath({ boardId: 'b', viewId: 'v' }, '')).toBe('/');
  });
});

describe('the address of a diary', () => {
  it('reads the diary and the day of the year, and writes them back', () => {
    expect(parseRoute('/w/me/diaries/d/11-21', '')).toEqual({
      kind: 'diary',
      workspaceId: 'me',
      diaryId: 'd',
      monthDay: '11-21',
    });
    expect(parseRoute('/w/me/diaries/d', '')).toEqual({
      kind: 'diary',
      workspaceId: 'me',
      diaryId: 'd',
    });
    for (const path of ['/w/me/diaries/d', '/w/me/diaries/d/02-29']) {
      expect(routePath(parseRoute(path, ''), '')).toBe(path);
    }
  });

  it('reads and writes ?year=YYYY on a diary day, ignoring invalid years', () => {
    expect(parseRoute('/w/me/diaries/d/11-21', '', '?year=2024')).toEqual({
      kind: 'diary',
      workspaceId: 'me',
      diaryId: 'd',
      monthDay: '11-21',
      year: '2024',
    });
    expect(routePath(parseRoute('/w/me/diaries/d/11-21', '', '?year=2024'), '')).toBe(
      '/w/me/diaries/d/11-21?year=2024',
    );
    expect(routePath({ workspaceId: 'me', diaryId: 'd', year: '2024' }, '')).toBe(
      '/w/me/diaries/d',
    );
    for (const bad of ['', '?year=', '?year=abc', '?year=0000', '?year=20250', '?year=-1']) {
      const r = parseRoute('/w/me/diaries/d/11-21', '', bad);
      expect(r.kind === 'diary' ? r.year : undefined).toBeUndefined();
    }
    const withoutDay = parseRoute('/w/me/diaries/d', '', '?year=2024');
    expect(withoutDay.kind === 'diary' ? withoutDay.year : undefined).toBeUndefined();
  });

  it('builds entry search routes with ?year=YYYY and follows popstate across years', () => {
    history.replaceState(null, '', '/w/me/diaries/d/11-21');
    const router = new Router('');
    const stop = router.listen();
    const hit = {
      kind: 'entry' as const,
      id: 'e1',
      containerId: 'd',
      workspaceId: 'me',
      title: '2024-11-21',
    };
    expect(routeOfPlace(router, hit)).toEqual({
      workspaceId: 'me',
      diaryId: 'd',
      monthDay: '11-21',
      year: '2024',
    });
    expect(router.href(routeOfPlace(router, hit))).toBe('/w/me/diaries/d/11-21?year=2024');
    goToPlace(router, hit);
    expect(`${location.pathname}${location.search}`).toBe('/w/me/diaries/d/11-21?year=2024');
    expect(router.route).toMatchObject({ kind: 'diary', year: '2024' });
    router.openDay('11-21');
    expect(`${location.pathname}${location.search}`).toBe('/w/me/diaries/d/11-21');
    expect(router.route.kind === 'diary' ? router.route.year : undefined).toBeUndefined();
    history.replaceState(null, '', '/w/me/diaries/d/11-21?year=2023');
    window.dispatchEvent(new PopStateEvent('popstate'));
    expect(router.route).toMatchObject({ kind: 'diary', year: '2023' });
    stop();
  });

  it('keeps a day no year has, for the page to say it is not found', () => {
    expect(parseRoute('/w/me/diaries/d/13-40', '')).toMatchObject({
      kind: 'diary',
      monthDay: '13-40',
    });
  });

  it('is the address of a diary in the sidebar, and its diary the container shown', () => {
    const diary = { id: 'd', workspaceId: 'me', kind: 'diary' as const };
    expect(placeRoute(diary)).toEqual({ kind: 'diary', workspaceId: 'me', diaryId: 'd' });
    expect(shownContainerId(parseRoute('/w/me/diaries/d/11-21', ''))).toBe('d');
  });

  it('goes from day to day, the diary kept, and back to the day before', () => {
    history.replaceState(null, '', '/w/me/diaries/d/11-21');
    const router = new Router('');
    const stop = router.listen();
    const before = history.length;
    router.openDay('11-22');
    expect(router.route).toMatchObject({ kind: 'diary', diaryId: 'd', monthDay: '11-22' });
    expect(location.pathname).toBe('/w/me/diaries/d/11-22');
    expect(history.length).toBe(before + 1);
    router.openDay('11-23', { replace: true });
    expect(history.length).toBe(before + 1);
    history.replaceState(null, '', '/w/me/diaries/d/11-21');
    window.dispatchEvent(new PopStateEvent('popstate'));
    expect(router.route).toMatchObject({ kind: 'diary', monthDay: '11-21' });
    stop();
  });
});

describe('the address of a course', () => {
  it('reads the course and its unit, writes them back, and rejects extra segments', () => {
    const parsed = parseRoute('/w/ws/courses/c1/u1', '');
    expect(parsed).toEqual({
      kind: 'course',
      workspaceId: 'ws',
      courseId: 'c1',
      unitId: 'u1',
    });
    expect(parseRoute('/w/ws/courses/c1', '')).toEqual({
      kind: 'course',
      workspaceId: 'ws',
      courseId: 'c1',
    });
    expect(parseRoute('/w/ws/courses/c1/u1/extra', '').kind).toBe('unknown');
    for (const path of ['/w/ws/courses/c1', '/w/ws/courses/c1/u1']) {
      expect(routePath(parseRoute(path, ''), '')).toBe(path);
    }
    expect(routePath({ workspaceId: 'ws', unitId: 'u1' }, '')).toBe('/w/ws');
  });

  it('places courses in the sidebar, resolves shownContainerId, and navigates units and search hits', () => {
    const course = { id: 'c1', workspaceId: 'ws', kind: 'course' as const };
    expect(placeRoute(course)).toEqual({ kind: 'course', workspaceId: 'ws', courseId: 'c1' });
    expect(shownContainerId(parseRoute('/w/ws/courses/c1/u1', ''))).toBe('c1');

    history.replaceState(null, '', '/w/ws/courses/c1');
    const router = new Router('');
    const stop = router.listen();
    const before = history.length;
    router.openUnit('u1');
    expect(router.route).toMatchObject({ kind: 'course', courseId: 'c1', unitId: 'u1' });
    expect(location.pathname).toBe('/w/ws/courses/c1/u1');
    expect(history.length).toBe(before + 1);

    router.openUnit('u2', { replace: true });
    expect(router.route).toMatchObject({ kind: 'course', unitId: 'u2' });
    expect(history.length).toBe(before + 1);

    const hit = {
      kind: 'unit' as const,
      id: 'u9',
      containerId: 'c2',
      workspaceId: 'ws',
      title: 'Lesson 9',
    };
    expect(routeOfPlace(router, hit)).toEqual({
      workspaceId: 'ws',
      courseId: 'c2',
      unitId: 'u9',
    });
    goToPlace(router, hit);
    expect(location.pathname).toBe('/w/ws/courses/c2/u9');
    expect(router.route).toMatchObject({ kind: 'course', courseId: 'c2', unitId: 'u9' });

    expect(parseRoute('/w/ws/courses/c2', '', '?tab=meetings&meeting=m1')).toEqual({
      kind: 'course',
      workspaceId: 'ws',
      courseId: 'c2',
      courseTab: 'meetings',
      meetingId: 'm1',
    });
    expect(routePath(parseRoute('/w/ws/courses/c2', '', '?tab=meetings&meeting=m1'), '')).toBe(
      '/w/ws/courses/c2?tab=meetings&meeting=m1',
    );
    const examsParsed = parseRoute('/w/ws/courses/c2', '', '?tab=exams&meeting=m1');
    expect(examsParsed).toEqual({
      kind: 'course',
      workspaceId: 'ws',
      courseId: 'c2',
      courseTab: 'exams',
    });
    const otherTab = parseRoute('/w/ws/courses/c2', '', '?tab=other');
    expect(otherTab.kind === 'course' ? otherTab.courseTab : undefined).toBeUndefined();

    const meetingHit = {
      kind: 'meeting' as const,
      id: 'm42',
      containerId: 'c2',
      workspaceId: 'ws',
      title: 'Weekly Sync',
    };
    expect(routeOfPlace(router, meetingHit)).toEqual({
      workspaceId: 'ws',
      courseId: 'c2',
      courseTab: 'meetings',
      meetingId: 'm42',
    });
    goToPlace(router, meetingHit);
    expect(`${location.pathname}${location.search}`).toBe(
      '/w/ws/courses/c2/u9?tab=meetings&meeting=m42',
    );
    router.openCourseTab('exams');
    expect(`${location.pathname}${location.search}`).toBe('/w/ws/courses/c2/u9?tab=exams');
    router.openCourseTab('units');
    expect(`${location.pathname}${location.search}`).toBe('/w/ws/courses/c2/u9');
    stop();
  });
});

describe('the base path', () => {
  const metaWith = (content: string): Document => {
    const other = document.implementation.createHTMLDocument('');
    const meta = other.createElement('meta');
    meta.setAttribute('name', 'fvoci:base');
    meta.setAttribute('content', content);
    other.head.append(meta);
    return other;
  };

  it('is what the server wrote into the page', () => {
    expect(readBasePath(metaWith('/boards'))).toBe('boards');
    expect(readBasePath(metaWith('/boards/'))).toBe('boards');
    expect(readBasePath(metaWith(''))).toBe('');
  });

  it('is empty when nobody filled the placeholder in, or the element is missing', () => {
    expect(readBasePath(metaWith('{{.BaseURL}}'))).toBe('');
    expect(readBasePath(document.implementation.createHTMLDocument(''))).toBe('');
  });
});

describe('moving about', () => {
  beforeEach(() => {
    history.replaceState(null, '', '/w/t/boards/b/v');
  });

  it('starts from the address the page was loaded on', () => {
    expect(new Router('').route).toEqual({
      kind: 'board',
      workspaceId: 't',
      boardId: 'b',
      viewId: 'v',
    });
  });

  it('opens a card as a new step and another card in its place', () => {
    const router = new Router('');
    const before = history.length;
    router.openCard('c1');
    expect(location.pathname).toBe('/w/t/boards/b/v/c1');
    expect(history.length).toBe(before + 1);
    router.openCard('c2');
    expect(location.pathname).toBe('/w/t/boards/b/v/c2');
    expect(history.length).toBe(before + 1);
  });

  it('closes a card it was loaded on by replacing the address', () => {
    history.replaceState(null, '', '/w/t/boards/b/v/c');
    const router = new Router('');
    const before = history.length;
    router.closeCard();
    expect(location.pathname).toBe('/w/t/boards/b/v');
    expect(history.length).toBe(before);
    expect(cardIdOf(router.route)).toBeUndefined();
  });

  it('drops the view and the card when another board is opened', () => {
    const router = new Router('');
    router.openCard('c');
    router.openContainer({ id: 'other', workspaceId: 't' });
    expect(location.pathname).toBe('/w/t/boards/other');
    expect(router.route).toEqual({ kind: 'board', workspaceId: 't', boardId: 'other' });
  });

  it('opens a board at the address of its own workspace', () => {
    const router = new Router('');
    router.openContainer({ id: 'theirs', workspaceId: 't2' });
    expect(location.pathname).toBe('/w/t2/boards/theirs');
    expect(router.route).toEqual({ kind: 'board', workspaceId: 't2', boardId: 'theirs' });
  });

  it('keeps the workspace when another view is opened', () => {
    const router = new Router('');
    router.openView('v2');
    expect(location.pathname).toBe('/w/t/boards/b/v2');
  });

  it('follows back and forward to another workspace', () => {
    const router = new Router('');
    const stop = router.listen();
    history.pushState(null, '', '/w/t2/boards/x');
    window.dispatchEvent(new PopStateEvent('popstate'));
    expect(router.route).toEqual({ kind: 'board', workspaceId: 't2', boardId: 'x' });
    stop();
  });

  it('loads the page anew at a route, the entries of what is open let go first', () => {
    const router = new Router('app');
    const calls: string[] = [];
    const released = vi.spyOn(back, 'releaseOverlayEntries');
    vi.stubGlobal('location', {
      assign: (to: string) => calls.push(`assign ${to}`),
      replace: (to: string) => calls.push(`replace ${to}`),
    });
    try {
      released.mockImplementationOnce(() => {
        calls.push('release');
        return 0;
      });
      router.restart({});
      released.mockImplementationOnce(() => {
        calls.push('release');
        return 1;
      });
      router.restart({});
      expect(calls).toEqual(['release', 'assign /app/', 'release', 'replace /app/']);
    } finally {
      vi.unstubAllGlobals();
      released.mockRestore();
    }
  });

  it('makes links that keep the base path', () => {
    expect(
      new Router('app').href({ workspaceId: 't', boardId: 'b', viewId: 'v', cardId: 'c' }),
    ).toBe('/app/w/t/boards/b/v/c');
  });
});

describe('the language', () => {
  it('finds the closest set of messages for what the browser asks', () => {
    expect(matchLocale('ko-KR')).toBe('ko');
    expect(matchLocale('en-GB')).toBe('en');
    expect(matchLocale('ja-JP')).toBeUndefined();
    expect(matchLocale('xx-YY')).toBeUndefined();
    expect(matchLocale('KO-kr')).toBe('ko');
    expect(matchLocale('EN-us')).toBe('en');
  });

  it('shows the account setting in its closest set, and auto or one it has none for as the browser asks', () => {
    const browser = matchLocale(navigator.languages[0] ?? '') ?? 'en';
    expect(localeOfSetting('ko')).toBe('ko');
    expect(localeOfSetting('ko-KR')).toBe('ko');
    expect(localeOfSetting('en-GB')).toBe('en');
    expect(localeOfSetting('de-AT')).toBe(browser);
    expect(localeOfSetting('auto')).toBe(browser);
    expect(localeOfSetting('xx')).toBe(browser);
  });
});
