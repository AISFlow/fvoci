/**
 * Navigation caches (nav-speed t2):
 * - Bounded LRU workspace sidebar cache with stale-while-revalidate & WS dedup
 * - Wiki fast first-page open without waiting for the full page list + fallback
 * - Bounded LRU `BoardStore` cache (cap 2) with zero leaked listeners/rooms/timers
 * - Strict security, isolation & invalidation on 401/403/404, role change,
 *   member removal, org leaving, SSO/password lock, logout and account switch
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  BackendError,
  BoardCache,
  WikiCache,
  WikiStore,
  Workspace,
  WorkspaceCache,
  leaveOrganization,
  whileHere,
  type WorkspaceInfo,
} from '$lib/state/index.ts';
import type { BodyDoc, ChangeListener } from '$lib/state/backend.ts';
import { OrgAdmin } from '$lib/state/org-admin.svelte.ts';
import type { Organizations } from '$lib/state/organization.ts';

import { settle, setup, SMALL, text, type Setup } from './support.ts';

const closers: (() => void)[] = [];

afterEach(() => {
  while (closers.length > 0) closers.pop()?.();
});

function track<T extends { close(): void }>(store: T): T {
  closers.push(() => {
    store.close();
  });
  return store;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function multiWorkspaceSetup(
  extra: WorkspaceInfo[],
  options: { cacheLimits?: { workspaces?: number; boards?: number; wikis?: number } } = {},
) {
  const made = setup([SMALL, { ...SMALL, key: 'small-2' }, { ...SMALL, key: 'small-3' }]);
  const { backend, world } = made;
  const first = world.workspace;
  const all = [first, ...extra];
  const extras = new Set(extra.map((one) => one.id));

  vi.spyOn(backend, 'workspaces').mockResolvedValue(all);
  const realUsers = backend.users.bind(backend);
  const realBoards = backend.boards.bind(backend);
  const realCategories = backend.categories.bind(backend);
  const realWikis = backend.wiki.wikis.bind(backend.wiki);
  const realCourses = backend.course.courses.bind(backend.course);

  const usersSpy = vi
    .spyOn(backend, 'users')
    .mockImplementation((id) => (extras.has(id) ? Promise.resolve([]) : realUsers(id)));
  const boardsSpy = vi
    .spyOn(backend, 'boards')
    .mockImplementation((id) => (extras.has(id) ? Promise.resolve([]) : realBoards(id)));
  const categoriesSpy = vi
    .spyOn(backend, 'categories')
    .mockImplementation((id) => (extras.has(id) ? Promise.resolve([]) : realCategories(id)));
  const wikisSpy = vi
    .spyOn(backend.wiki, 'wikis')
    .mockImplementation((id) => (extras.has(id) ? Promise.resolve([]) : realWikis(id)));
  vi.spyOn(backend.course, 'courses').mockImplementation((id) =>
    extras.has(id) ? Promise.resolve([]) : realCourses(id),
  );

  const workspace = track(
    new Workspace({
      backend,
      locale: () => 'en',
      cacheLimits: options.cacheLimits,
    }),
  );

  return {
    ...made,
    workspace,
    first,
    all,
    usersSpy,
    boardsSpy,
    categoriesSpy,
    wikisSpy,
  };
}

describe('workspace sidebar cache (stale-while-revalidate + LRU)', () => {
  it('shows cached sidebar immediately on return and merges fresh data in the background', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load();
    const initialIds = workspace.containers.map((one) => one.id);
    expect(initialIds.length).toBeGreaterThan(0);

    await workspace.enter(ws2.id);
    expect(workspace.info?.id).toBe(ws2.id);
    expect(workspace.containers).toEqual([]);

    // Make a new board in `first` while the reader is in `ws2`.
    const added = await backend.createBoard(
      first.id,
      { title: 'Added While Away', cardProperties: [] },
      { title: 'Board', fields: { viewType: 'board' } as never },
    );

    // Hold the background revalidation so we can inspect synchronous frame-0 state.
    const gate = deferred<Awaited<ReturnType<typeof backend.boards>>>();
    const realBoards = backend.boards.bind(backend);
    boardsSpy.mockImplementationOnce((id) => (id === first.id ? gate.promise : realBoards(id)));

    const entering = workspace.enter(first.id);
    // Synchronously ready with cached data (zero loading state!).
    expect(workspace.entered).toBe('ready');
    expect(workspace.info?.id).toBe(first.id);
    expect(workspace.containers.map((one) => one.id)).toEqual(initialIds);

    // Complete background revalidation: fresh board merges in.
    const fresh = await backend.boards(first.id);
    expect(fresh.map((one) => one.id)).toContain(added.board.id);
    gate.resolve(fresh);
    await entering;
    expect(workspace.containers.map((one) => one.id)).toContain(added.board.id);
    workspace.close();
  });

  it('evicts the least-recently-used workspace snapshot when the cap is exceeded', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const ws3: WorkspaceInfo = { id: 'ws-3', title: 'Three' };
    const { workspace, first, boardsSpy } = multiWorkspaceSetup([ws2, ws3], {
      cacheLimits: { workspaces: 2 },
    });
    await workspace.load(); // caches first
    await workspace.enter(ws2.id); // caches ws2 -> [first, ws2]
    await workspace.enter(ws3.id); // caches ws3 -> evicts first -> [ws2, ws3]

    // Returning to ws2 is a cache hit (synchronously ready).
    const hitGate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(hitGate.promise);
    const enterWs2 = workspace.enter(ws2.id);
    expect(workspace.entered).toBe('ready');
    hitGate.resolve([]);
    await enterWs2;

    // Returning to evicted `first` is a cache miss (starts in 'loading').
    const missGate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(missGate.promise);
    const enterFirst = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    missGate.resolve([]);
    await enterFirst;
    expect(workspace.entered).toBe('ready');
    workspace.close();
  });

  it('keeps stale cached sidebar on transient 500 during background revalidation', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load();
    const expectedCount = workspace.containers.length;
    await workspace.enter(ws2.id);

    boardsSpy.mockRejectedValueOnce(new BackendError(500));
    await workspace.enter(first.id);
    expect(workspace.entered).toBe('ready');
    expect(workspace.containers).toHaveLength(expectedCount);
    workspace.close();
  });

  it('coalesces resync while enter is in flight and runs one follow-up refresh after enter finishes', async () => {
    const { workspace, backend, world, first, boardsSpy } = multiWorkspaceSetup([]);
    await workspace.load();
    boardsSpy.mockClear();

    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const reentering = workspace.enter(first.id, true);
    world.tellUsers([backend.userId], { kind: 'resync' });
    world.tellUsers([backend.userId], { kind: 'resync' });
    await settle();
    expect(boardsSpy).toHaveBeenCalledTimes(1);

    const followUp = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(followUp.promise);
    gate.resolve([]);
    await reentering;
    await vi.waitFor(() => {
      expect(boardsSpy).toHaveBeenCalledTimes(2);
    });
    followUp.resolve([]);
    await settle();
  });

  it('preserves user updates received in-flight during cached workspace revalidation', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, world, first, usersSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load();
    const initialUser = workspace.users[0];
    if (!initialUser) throw new Error('expected workspace user');
    await workspace.enter(ws2.id);

    const staleUsers = [{ ...initialUser, username: 'stale-copy' }];
    const updatedUser = { ...initialUser, username: 'live-updated' };
    const usersGate = deferred<typeof staleUsers>();
    usersSpy.mockReturnValueOnce(usersGate.promise);
    vi.spyOn(backend, 'usersById').mockResolvedValueOnce([updatedUser]);

    const entering = workspace.enter(first.id);
    expect(workspace.entered).toBe('ready');
    // Live user update arrives while cached revalidation is still in flight.
    world.tellUsers([backend.userId], { kind: 'user', userId: initialUser.id });
    await vi.waitFor(() => {
      expect(workspace.user(initialUser.id)?.username).toBe('live-updated');
    });

    usersGate.resolve(staleUsers);
    await entering;
    expect(workspace.user(initialUser.id)?.username).toBe('live-updated');
    expect(workspace.workspaceUsers.find((u) => u.id === initialUser.id)?.username).toBe(
      'live-updated',
    );
  });
});

describe('wiki fast first-page open and fallback', () => {
  async function setupWiki() {
    const made = setup();
    await made.workspace.load();
    const wiki = await made.backend.wiki.createWiki(made.world.workspace.id, {
      title: 'Docs',
      visibility: 'private',
    });
    const p1 = await made.backend.wiki.createPage(wiki.id, { title: 'First Page' });
    await made.backend.wiki.patchPage(wiki.id, p1.id, {
      body: text('First body'),
      readAt: p1.updatedAt,
    });
    const p2 = await made.backend.wiki.createPage(wiki.id, { title: 'Second Page' });
    await made.backend.wiki.patchPage(wiki.id, p2.id, {
      body: text('Second body'),
      readAt: p2.updatedAt,
    });
    return { ...made, wiki, p1, p2 };
  }

  it('requests and displays the remembered last page before the full page list finishes', async () => {
    const { workspace, backend, wiki, p2 } = await setupWiki();

    // First visit opens p2 and remembers it.
    const firstVisit = track(new WikiStore(workspace, wiki.id));
    await firstVisit.load();
    firstVisit.showPage(p2.id);
    await settle();
    expect(workspace.rememberedWikiPage(wiki.id)).toBe(p2.id);
    firstVisit.close();

    // Second visit: hold the full page list while page(p2) responds immediately.
    const allPages = await backend.wiki.pages(wiki.id);
    const pagesGate = deferred<typeof allPages>();
    vi.spyOn(backend.wiki, 'pages').mockReturnValueOnce(pagesGate.promise);

    const secondVisit = track(new WikiStore(workspace, wiki.id));
    const remembered = workspace.rememberedWikiPage(wiki.id);
    const loading = secondVisit.load({ pageId: remembered, speculative: true });
    await vi.waitFor(() => {
      expect(secondVisit.status).toBe('ready');
      expect(secondVisit.openPage?.status).toBe('ready');
    });

    // Page content and metadata are ready BEFORE the full page list resolves!
    expect(secondVisit.pagesReady).toBe(false);
    expect(secondVisit.pages).toEqual([]);
    expect(secondVisit.openPage?.pageId).toBe(p2.id);
    expect(secondVisit.openPage?.meta?.title).toBe('Second Page');
    expect(secondVisit.openPage?.body).toEqual(text('Second body'));

    pagesGate.resolve(allPages);
    await loading;
    expect(secondVisit.pagesReady).toBe(true);
    expect(secondVisit.pages).toHaveLength(2);
  });

  it('falls back cleanly when the remembered page was deleted (404) before the list arrives', async () => {
    const { workspace, backend, wiki, p1, p2 } = await setupWiki();
    workspace.rememberWikiPage(wiki.id, p2.id);
    await backend.wiki.deletePage(wiki.id, p2.id);

    const remaining = await backend.wiki.pages(wiki.id);
    const pagesGate = deferred<typeof remaining>();
    vi.spyOn(backend.wiki, 'pages').mockReturnValueOnce(pagesGate.promise);

    const store = track(new WikiStore(workspace, wiki.id));
    const loading = store.load({ pageId: p2.id, speculative: true });

    await vi.waitFor(() => {
      expect(store.speculativePageId).toBeUndefined();
      expect(store.openPage).toBeUndefined();
    });
    // Forgotten from cache and no NotFound flash while waiting for the list.
    expect(workspace.rememberedWikiPage(wiki.id)).toBeUndefined();

    pagesGate.resolve(remaining);
    await loading;
    expect(store.pagesReady).toBe(true);
    expect(store.pages.map((page) => page.id)).toEqual([p1.id]);
  });

  it('cancels speculative page if the list resolves first without it', async () => {
    const { workspace, backend, wiki, p1, p2 } = await setupWiki();
    workspace.rememberWikiPage(wiki.id, p2.id);
    await backend.wiki.deletePage(wiki.id, p2.id);

    const pageGate = deferred<never>();
    vi.spyOn(backend.wiki, 'page').mockReturnValueOnce(pageGate.promise);

    const store = track(new WikiStore(workspace, wiki.id));
    await store.load({ pageId: p2.id, speculative: true });
    expect(store.pagesReady).toBe(true);
    expect(store.speculativePageId).toBeUndefined();
    expect(store.openPage).toBeUndefined();
    expect(workspace.rememberedWikiPage(wiki.id)).toBeUndefined();
    expect(store.pages.map((page) => page.id)).toEqual([p1.id]);
  });

  it('closes speculative page and evicts wiki cache when wiki access is refused (403)', async () => {
    const { workspace, backend, wiki, p1 } = await setupWiki();
    workspace.rememberWikiPage(wiki.id, p1.id);
    vi.spyOn(backend.wiki, 'pages').mockRejectedValueOnce(new BackendError(403));

    const store = track(new WikiStore(workspace, wiki.id));
    await store.load({ pageId: p1.id, speculative: true });
    expect(store.missing).toBe(true);
    expect(store.status).toBe('failed');
    expect(store.openPage).toBeUndefined();
    expect(workspace.rememberedWikiPage(wiki.id)).toBeUndefined();
  });
});

describe('recent BoardStore cache (cap 2, instant resume, zero leaks)', () => {
  it('resumes cached BoardStore immediately on board<->board and board<->wiki round-trips without leaking listeners or rooms', async () => {
    const made = setup([SMALL, { ...SMALL, key: 'small-2' }, { ...SMALL, key: 'small-3' }]);
    const { workspace, backend } = made;
    let activeWatchers = 0;
    const realWatch = backend.watchContainer.bind(backend);
    vi.spyOn(backend, 'watchContainer').mockImplementation((id, listener) => {
      activeWatchers += 1;
      const stop = realWatch(id, listener);
      return () => {
        activeWatchers -= 1;
        stop();
      };
    });

    const closedRooms: string[] = [];
    backend.openDoc = (_ws, _board, cardId): BodyDoc => ({
      state: {
        status: 'joining',
        doc: undefined,
        carets: undefined,
        composing: () => undefined,
        readsFirst: () => () => undefined,
        mode: 'write',
        reason: undefined,
        writable: false,
        pending: false,
        retrying: false,
      },
      close: () => {
        closedRooms.push(cardId);
      },
      retry: () => undefined,
      settle: () => Promise.resolve(),
    });

    await workspace.load();
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected at least 2 boards');

    const store1 = await workspace.openBoard(b1.id);
    expect(activeWatchers).toBe(1);
    const card = store1.cards[0];
    if (!card) throw new Error('expected card');
    await store1.showCard(card.id);
    expect(store1.openCard).toBeDefined();

    // Switch b1 -> b2: b1 parks (unsubscribes WS, closes openCard + doc room).
    const store2 = await workspace.openBoard(b2.id);
    expect(activeWatchers).toBe(1);
    expect(store1.openCard).toBeUndefined();
    expect(closedRooms).toEqual([card.id]);

    // Edit b1 while parked; parked store does NOT receive live updates while away.
    const renamed = await backend.patchCard(b1.id, card.id, { title: 'Updated While Parked' });
    await settle();
    expect(store1.card(card.id)?.title).not.toBe(renamed.title);

    // Return b2 -> b1: synchronously sets cached store1 (ready!), then refreshes in background.
    const cardsGate = deferred<Awaited<ReturnType<typeof backend.cards>>>();
    const realCards = backend.cards.bind(backend);
    vi.spyOn(backend, 'cards').mockImplementationOnce((id) =>
      id === b1.id ? cardsGate.promise : realCards(id),
    );
    const reopening = workspace.openBoard(b1.id);
    expect(workspace.board).toBe(store1);
    expect(store1.status).toBe('ready');
    expect(store1.revalidating).toBe(true);
    expect(activeWatchers).toBe(1);

    cardsGate.resolve(await realCards(b1.id));
    expect(await reopening).toBe(store1);
    expect(store1.revalidating).toBe(false);
    expect(store1.card(card.id)?.title).toBe('Updated While Parked');

    // Board -> Wiki (`closeBoard`) -> Board: parks with 0 watchers, then resumes instantly.
    workspace.closeBoard();
    expect(workspace.board).toBeUndefined();
    expect(activeWatchers).toBe(0);

    const resumed = await workspace.openBoard(b1.id);
    expect(resumed).toBe(store1);
    expect(activeWatchers).toBe(1);
    expect(store2.status).toBe('ready');

    workspace.close();
    expect(activeWatchers).toBe(0);
  });

  it('evicts and closes the oldest BoardStore when exceeding the cap of 2', async () => {
    const { workspace } = multiWorkspaceSetup([]);
    await workspace.load();
    const [b1, b2, b3] = workspace.containers;
    if (!b1 || !b2 || !b3) throw new Error('expected 3 boards');

    const store1 = await workspace.openBoard(b1.id);
    const close1 = vi.spyOn(store1, 'close');
    const store2 = await workspace.openBoard(b2.id);
    expect(close1).not.toHaveBeenCalled();

    // Opening b3 exceeds cap of 2 -> evicts and closes b1.
    await workspace.openBoard(b3.id);
    expect(close1).toHaveBeenCalledOnce();

    // b2 is still cached, b1 is a fresh store.
    expect(await workspace.openBoard(b2.id)).toBe(store2);
    expect(await workspace.openBoard(b1.id)).not.toBe(store1);
    workspace.close();
  });

  it('preserves whileHere isolation across cached board round-trips (b1 -> b2 -> b1)', async () => {
    const { workspace, backend } = multiWorkspaceSetup([]);
    await workspace.load();
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected 2 boards');

    const store1 = await workspace.openBoard(b1.id);
    const gate = deferred<Awaited<ReturnType<typeof backend.createCard>>>();
    const realCreate = backend.createCard.bind(backend);
    vi.spyOn(backend, 'createCard').mockReturnValueOnce(gate.promise);
    const creating = whileHere(store1, () => store1.createCard({ title: 'Stale' }));

    await workspace.openBoard(b2.id);
    const returned = await workspace.openBoard(b1.id);
    expect(returned).toBe(store1);
    expect(workspace.board).toBe(store1);

    // Release the card creation only AFTER returning to the same store1 instance.
    gate.resolve(await realCreate(b1.id, { title: 'Stale' }));
    expect(await creating).toBeUndefined();
  });
});

describe('strict security, isolation and cache invalidation', () => {
  it('clears all caches immediately on 401 before load() and keeps User B isolated', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load();
    const b1 = workspace.containers[0];
    if (!b1) throw new Error('expected board');
    const store1 = await workspace.openBoard(b1.id);
    const close1 = vi.spyOn(store1, 'close');
    workspace.rememberWikiPage('wiki-1', 'page-1');
    expect(workspace.rememberedWikiPage('wiki-1')).toBe('page-1');
    await workspace.enter(ws2.id);

    // 401 session expired wipes ALL caches and closes parked boards BEFORE load() runs.
    workspace.report('save', new BackendError(401, { detail: 'expired' }));
    expect(workspace.status).toBe('signedOut');
    expect(close1).toHaveBeenCalled();

    // Verify User A's cached sidebar and wiki page were already evicted by 401 (without calling load).
    workspace.workspaces = [first, ws2];
    const preLoadGate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(preLoadGate.promise);
    const preLoadEnter = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    expect(workspace.containers).toEqual([]);
    expect(workspace.rememberedWikiPage('wiki-1')).toBeUndefined();
    preLoadGate.resolve([]);
    await preLoadEnter;
    expect(workspace.rememberedWikiPage('wiki-1')).toBeUndefined();

    // Another user signs in on the same Workspace instance: zero cached data from User A!
    if (!workspace.me) throw new Error('expected signed-in user');
    const userB = { ...workspace.me, id: 'user-b', username: 'bob' };
    vi.spyOn(backend, 'me').mockResolvedValue(userB);
    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const loadingB = workspace.load(first.id);
    await vi.waitFor(() => {
      expect(workspace.entered).toBe('loading');
    });
    expect(workspace.containers).toEqual([]);
    expect(workspace.rememberedWikiPage('wiki-1')).toBeUndefined();
    gate.resolve([]);
    await loadingB;
  });

  it('evicts cached workspace, boards and wiki pages immediately when 403/404 arrives on return or resync', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, world, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load();
    const b1 = workspace.containers[0];
    if (!b1) throw new Error('expected board');
    const store1 = await workspace.openBoard(b1.id);
    const close1 = vi.spyOn(store1, 'close');
    workspace.rememberWikiPage('wiki-1', 'page-1');
    await workspace.enter(ws2.id);

    // Return to `first`, but background revalidation gets 403 (removed from workspace).
    boardsSpy.mockRejectedValueOnce(new BackendError(403));
    await workspace.enter(first.id);
    expect(workspace.entered).toBe('missing');
    expect(workspace.containers).toEqual([]);
    expect(close1).toHaveBeenCalled();
    expect(workspace.rememberedWikiPage('wiki-1')).toBeUndefined();

    // Normal (non-`again`) return from ws2 -> first must be a cache miss (no stale snapshot shown).
    await workspace.enter(ws2.id);
    const retryGate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(retryGate.promise);
    const retry = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    expect(workspace.containers).toEqual([]);
    retryGate.resolve([]);
    await retry;

    // Also on resync (#refresh) getting 403 while inside a workspace: evict and mark missing.
    await workspace.enter(ws2.id);
    boardsSpy.mockRejectedValueOnce(new BackendError(403));
    world.tellUsers([backend.userId], { kind: 'resync' });
    await vi.waitFor(() => {
      expect(workspace.entered).toBe('missing');
    });
  });

  it('evicts cached board and clears its content immediately when refresh returns 403 or 404', async () => {
    const { workspace, backend } = multiWorkspaceSetup([]);
    await workspace.load();
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected 2 boards');

    const store1 = await workspace.openBoard(b1.id);
    expect(store1.cards.length).toBeGreaterThan(0);
    await workspace.openBoard(b2.id);

    // Revoked access (403 on private board) when resuming b1.
    vi.spyOn(backend, 'views').mockRejectedValueOnce(new BackendError(403));
    const resumed = await workspace.openBoard(b1.id);
    expect(resumed).toBe(store1);
    expect(resumed.status).toBe('failed');
    expect(resumed.cards).toEqual([]);
    expect(resumed.views).toEqual([]);
    expect(resumed.members).toEqual([]);

    // Next openBoard creates a fresh store (b1 was evicted from cache).
    const fresh = await workspace.openBoard(b1.id);
    expect(fresh).not.toBe(store1);
    expect(fresh.status).toBe('ready');
    workspace.close();
  });

  it('evicts org caches on SSO requirement, leaving an organization, and OrgAdmin role/workspace changes', async () => {
    const orgA: WorkspaceInfo = { id: 'ws-a', title: 'Org A', orgId: 'org-a', kind: 'shared' };
    const orgB: WorkspaceInfo = { id: 'ws-b', title: 'Org B', orgId: 'org-b', kind: 'shared' };
    const { workspace, backend, boardsSpy } = multiWorkspaceSetup([orgA, orgB]);
    await workspace.load(orgA.id);
    workspace.rememberWikiPage('wiki-a', 'page-a');
    await workspace.enter(orgB.id);

    // Forced SSO for org-a evicts org-a's cached sidebar and wiki memory immediately.
    workspace.report('load', new BackendError(401, { reason: 'sso_required', orgId: 'org-a' }));
    expect(workspace.workspaces.find((w) => w.id === orgA.id)?.ssoRequired).toBe(true);

    // Re-open org-a after SSO -> must be a cache miss (not stale pre-SSO snapshot).
    workspace.workspaces = workspace.workspaces.map((w) =>
      w.id === orgA.id ? { ...w, ssoRequired: false } : w,
    );
    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const enteringA = workspace.enter(orgA.id);
    expect(workspace.entered).toBe('loading');
    gate.resolve([]);
    await enteringA;
    expect(workspace.rememberedWikiPage('wiki-a')).toBeUndefined();

    // Leaving org-a evicts org-a's cached snapshot directly (even if org-a is re-listed).
    await workspace.enter(orgB.id);
    await leaveOrganization(workspace, () => Promise.resolve(), 'org-a');
    workspace.workspaces = [...workspace.workspaces, orgA];
    const gateAfterLeaveOrg = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gateAfterLeaveOrg.promise);
    const enterAfterLeaveOrg = workspace.enter(orgA.id);
    expect(workspace.entered).toBe('loading');
    expect(workspace.containers).toEqual([]);
    gateAfterLeaveOrg.resolve([]);
    await enterAfterLeaveOrg;

    // OrgAdmin.leaveWorkspace evicts org-b; normal enter (without `again`) must be a cache miss.
    await workspace.enter(orgB.id);
    const orgApi = {
      organization: () => Promise.resolve({ id: 'org-b', name: 'B', publicLinks: false }),
      workspaces: () => Promise.resolve([]),
      members: () => Promise.resolve([]),
      records: () => Promise.resolve([]),
      leaveWorkspace: () => Promise.resolve({} as never),
      setRole: () => Promise.resolve({} as never),
    } as unknown as Organizations;
    const admin = new OrgAdmin(orgApi, 'org-b', workspace);
    await admin.leaveWorkspace(orgB.id);
    await workspace.enter(orgA.id);
    const gateB = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gateB.promise);
    const reenterB = workspace.enter(orgB.id);
    expect(workspace.entered).toBe('loading');
    expect(workspace.containers).toEqual([]);
    gateB.resolve([]);
    await reenterB;

    // Direct cache classes strictly enforce single-user isolation.
    const wsCache = new WorkspaceCache(2);
    wsCache.set({
      userId: 'u1',
      workspaceId: 'w1',
      users: [],
      workspaceUsers: [],
      containers: [],
      categories: [],
    });
    expect(wsCache.get('u2', 'w1')).toBeUndefined();
    expect(wsCache.size).toBe(0);

    const closed = vi.fn();
    const boardCache = new BoardCache(2);
    boardCache.track('u1', 'w1', {
      boardId: 'b1',
      status: 'ready',
      missing: false,
      joinable: false,
      role: 'admin',
      evicted: false,
      isClosed: () => false,
      markEvicted: () => undefined,
      close: closed,
    });
    expect(boardCache.take('u2', 'w1', 'b1')).toBeUndefined();
    expect(closed).toHaveBeenCalledOnce();
    expect(boardCache.size).toBe(0);

    const wikiCache = new WikiCache(2);
    wikiCache.set('u1', 'w1', 'k1', 'p1');
    expect(wikiCache.get('u2', 'w1', 'k1')).toBeUndefined();
    expect(wikiCache.size).toBe(0);
    // Keep unused backend reference satisfied cleanly.
    expect(backend).toBeDefined();
  });

  it('evicts all sibling workspaces of an org on OrgAdmin.setRole()', async () => {
    const orgA1: WorkspaceInfo = { id: 'ws-a1', title: 'Org A1', orgId: 'org-a', kind: 'shared' };
    const orgA2: WorkspaceInfo = { id: 'ws-a2', title: 'Org A2', orgId: 'org-a', kind: 'shared' };
    const orgB: WorkspaceInfo = { id: 'ws-b', title: 'Org B', orgId: 'org-b', kind: 'shared' };
    const { workspace, boardsSpy } = multiWorkspaceSetup([orgA1, orgA2, orgB]);
    await workspace.load(orgA1.id);
    await workspace.enter(orgA2.id);
    await workspace.enter(orgB.id);

    const orgApi = {
      organization: () => Promise.resolve({ id: 'org-a', name: 'A', publicLinks: false }),
      workspaces: () => Promise.resolve([]),
      members: () => Promise.resolve([]),
      records: () => Promise.resolve([]),
      setRole: () => Promise.resolve({} as never),
    } as unknown as Organizations;
    const admin = new OrgAdmin(orgApi, 'org-a', workspace);
    await admin.setRole('user-1', 'member');

    // Both org-a workspaces were evicted -> normal enters hit 'loading'.
    const gateA1 = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gateA1.promise);
    const enterA1 = workspace.enter(orgA1.id);
    expect(workspace.entered).toBe('loading');
    gateA1.resolve([]);
    await enterA1;

    const gateA2 = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gateA2.promise);
    const enterA2 = workspace.enter(orgA2.id);
    expect(workspace.entered).toBe('loading');
    gateA2.resolve([]);
    await enterA2;
  });

  it('evicts cached workspace on evictWorkspace and discards an in-flight #refresh()', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, world, first, boardsSpy, categoriesSpy } = multiWorkspaceSetup([
      ws2,
    ]);
    await workspace.load();
    await workspace.enter(ws2.id);

    // Evict `first` while parked (e.g. after removing a member or downgrading a role).
    workspace.evictWorkspace(first.id);
    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const entering = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    gate.resolve([]);
    await entering;

    // Now trigger an in-flight #refresh() on active `first` and evict mid-flight.
    const initialContainers = workspace.containers;
    const initialCategories = workspace.categories;
    const boardsGate = deferred<Awaited<ReturnType<typeof backend.boards>>>();
    const catsGate = deferred<Awaited<ReturnType<typeof backend.categories>>>();
    boardsSpy.mockReturnValueOnce(boardsGate.promise);
    categoriesSpy.mockReturnValueOnce(catsGate.promise);
    world.tellUsers([backend.userId], { kind: 'resync' });
    await settle();

    workspace.evictWorkspace(first.id);
    boardsGate.resolve([
      {
        id: 'stale-board',
        workspaceId: first.id,
        title: 'Stale',
        visibility: 'open',
        module: 'boards',
        createdAt: 1,
        updatedAt: 1,
      } as never,
    ]);
    catsGate.resolve([{ id: 'stale-cat', name: 'Stale', containerIds: [] } as never]);
    await settle();

    // In-flight pre-eviction #refresh() was discarded and did NOT re-cache `first`.
    expect(workspace.containers).toBe(initialContainers);
    expect(workspace.categories).toBe(initialCategories);
    await workspace.enter(ws2.id);
    const reenterGate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(reenterGate.promise);
    const reentering = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    reenterGate.resolve([]);
    await reentering;
  });

  it('does not re-cache the active workspace on leave after pruneWorkspaces sees it removed or locked', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load(first.id);
    workspace.workspaces = workspace.workspaces.map((w) =>
      w.id === first.id ? { ...w, ssoRequired: true } : w,
    );
    workspace.pruneWorkspaces();

    await workspace.enter(ws2.id);
    workspace.workspaces = workspace.workspaces.map((w) =>
      w.id === first.id ? { ...w, ssoRequired: false } : w,
    );
    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const returning = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    gate.resolve([]);
    await returning;
  });

  it('signs out and clears caches when WikiStore or PageStore encounters 401', async () => {
    const made: Setup = setup();
    await made.workspace.load();
    const wiki = await made.backend.wiki.createWiki(made.world.workspace.id, {
      title: 'Docs',
      visibility: 'private',
    });
    const page = await made.backend.wiki.createPage(wiki.id, { title: 'P1' });
    const store = track(new WikiStore(made.workspace, wiki.id));
    await store.load();
    const shown = store.showPage(page.id);
    await settle();
    expect(shown?.status).toBe('ready');

    // 401 on page refresh signs out and wipes caches.
    vi.spyOn(made.backend.wiki, 'page').mockRejectedValueOnce(new BackendError(401));
    await shown?.refresh();
    expect(made.workspace.status).toBe('signedOut');
    expect(made.workspace.rememberedWikiPage(wiki.id)).toBeUndefined();
    store.close();
  });

  it('updates board cache immediately when reader is removed via WS members change', async () => {
    const { workspace, backend } = multiWorkspaceSetup([]);
    const listeners = new Map<string, ChangeListener>();
    const realWatch = backend.watchContainer.bind(backend);
    vi.spyOn(backend, 'watchContainer').mockImplementation((id, listener) => {
      listeners.set(id, listener);
      return realWatch(id, listener);
    });
    await workspace.load();
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected 2 boards');
    const store1 = await workspace.openBoard(b1.id);
    const close1 = vi.spyOn(store1, 'close');
    // Reader removed from members via WS -> board is evicted and not re-cached on switch.
    listeners.get(b1.id)?.({ kind: 'members', containerId: b1.id, members: [] });
    expect(store1.role).toBe('');
    expect(store1.canEdit).toBe(false);
    await workspace.openBoard(b2.id);
    expect(close1).toHaveBeenCalledOnce();
    const reopened = await workspace.openBoard(b1.id);
    expect(reopened).not.toBe(store1);
    workspace.close();
  });

  it('does not poison WorkspaceCache with User A data when User B signs in on the same Workspace', async () => {
    const ws2: WorkspaceInfo = { id: 'ws-2', title: 'Two' };
    const { workspace, backend, first, boardsSpy } = multiWorkspaceSetup([ws2]);
    await workspace.load(first.id);
    expect(workspace.containers.length).toBeGreaterThan(0);

    // Session expires while User A is actively inside `first` (with private boards loaded).
    workspace.report('save', new BackendError(401, { detail: 'expired' }));
    expect(workspace.status).toBe('signedOut');
    expect(workspace.containers).toEqual([]);
    expect(workspace.info).toBeUndefined();

    // User B signs in at ws2, then navigates to `first`: MUST NOT see User A's cached boards!
    if (!workspace.me) throw new Error('expected signed-in user');
    vi.spyOn(backend, 'me').mockResolvedValue({ ...workspace.me, id: 'user-b', username: 'bob' });
    await workspace.load(ws2.id);

    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const entering = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    expect(workspace.containers).toEqual([]);
    gate.resolve([]);
    await entering;
    workspace.close();
  });

  it('does not re-cache an evicted active workspace or board on leave, and avoids zombie boards on evictOrg', async () => {
    const orgA: WorkspaceInfo = { id: 'ws-a', title: 'Org A', orgId: 'org-a', kind: 'shared' };
    const orgB: WorkspaceInfo = { id: 'ws-b', title: 'Org B', orgId: 'org-b', kind: 'shared' };
    const { workspace, first, boardsSpy } = multiWorkspaceSetup([orgA, orgB]);
    await workspace.load(first.id);
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected 2 boards');
    const store1 = await workspace.openBoard(b1.id);

    // Evict currently active workspace -> switching away must NOT re-cache `first` or `store1`.
    workspace.evictWorkspace(first.id);
    expect(store1.isClosed()).toBe(false);
    expect(store1.evicted).toBe(true);
    await workspace.openBoard(b2.id);
    expect(store1.isClosed()).toBe(true);

    await workspace.enter(orgA.id);
    const gate = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gate.promise);
    const returning = workspace.enter(first.id);
    expect(workspace.entered).toBe('loading');
    gate.resolve([]);
    await returning;

    // OrgAdmin hitting 401 sso_required evicts org-a caches and marks workspace ssoRequired.
    await workspace.enter(orgA.id);
    await workspace.enter(orgB.id);
    const ssoError = new BackendError(401, { reason: 'sso_required', orgId: 'org-a' });
    const ssoOrgApi = {
      organization: () => Promise.reject(ssoError),
      workspaces: () => Promise.reject(ssoError),
      members: () => Promise.reject(ssoError),
      records: () => Promise.reject(ssoError),
    } as unknown as Organizations;
    const admin = new OrgAdmin(ssoOrgApi, 'org-a', workspace);
    await admin.load();
    expect(admin.status).toBe('sso');
    // org-a's cached sidebar was evicted so returning must fetch fresh rather than serving stale cache.
    const gateA = deferred<never[]>();
    boardsSpy.mockReturnValueOnce(gateA.promise);
    const reenterA = workspace.enter(orgA.id);
    expect(workspace.entered).toBe('loading');
    gateA.resolve([]);
    await reenterA;
    workspace.close();
  });

  it('evicts the failing parked board (not the active board) on an in-flight 403 write', async () => {
    const { workspace, backend } = multiWorkspaceSetup([]);
    await workspace.load();
    const [b1, b2] = workspace.containers;
    if (!b1 || !b2) throw new Error('expected 2 boards');
    const store1 = await workspace.openBoard(b1.id);
    const close1 = vi.spyOn(store1, 'close');

    const failWrite = deferred<never>();
    vi.spyOn(backend, 'patchContainer').mockReturnValueOnce(failWrite.promise);
    const writing = store1.changeBoard((cur) => ({ ...cur, title: 'Refused' }));
    const store2 = await workspace.openBoard(b2.id);

    failWrite.reject(new BackendError(403));
    await writing;
    expect(close1).toHaveBeenCalled();
    expect(store2.isClosed()).toBe(false);
    expect(await workspace.openBoard(b1.id)).not.toBe(store1);
    workspace.close();
  });

  it('marks confirmed speculative wiki page gone if absent from pages(), and allows early title edits', async () => {
    const made = setup();
    await made.workspace.load();
    const wiki = await made.backend.wiki.createWiki(made.world.workspace.id, {
      title: 'Docs',
      visibility: 'private',
    });
    const p1 = await made.backend.wiki.createPage(wiki.id, { title: 'Keep' });
    const p2 = await made.backend.wiki.createPage(wiki.id, { title: 'Soon Gone' });

    // Early title edit before pages() resolves updates meta and survives a pre-rename pages() snapshot.
    const realPages = made.backend.wiki.pages.bind(made.backend.wiki);
    const preRenamePages = await realPages(wiki.id);
    const pagesGate1 = deferred<typeof preRenamePages>();
    vi.spyOn(made.backend.wiki, 'pages').mockReturnValueOnce(pagesGate1.promise);
    const store1 = track(new WikiStore(made.workspace, wiki.id));
    const load1 = store1.load({ pageId: p1.id, speculative: false });
    await vi.waitFor(() => {
      expect(store1.status).toBe('ready');
      expect(store1.openPage?.status).toBe('ready');
    });
    await store1.changePage(p1.id, { title: 'Renamed Early' });
    expect(store1.openPage?.meta?.title).toBe('Renamed Early');
    pagesGate1.resolve(preRenamePages);
    await load1;
    expect(store1.page(p1.id)?.title).toBe('Renamed Early');
    store1.close();

    // Speculative page confirmed before pages() finishes, then absent from pages() -> marked removed.
    const pagesGate2 = deferred<typeof preRenamePages>();
    vi.spyOn(made.backend.wiki, 'pages').mockReturnValueOnce(pagesGate2.promise);
    const store2 = track(new WikiStore(made.workspace, wiki.id));
    const load2 = store2.load({ pageId: p2.id, speculative: true });
    await vi.waitFor(() => {
      expect(store2.openPage?.status).toBe('ready');
    });
    // Simulate URL confirmation calling showPage(p2.id) non-speculatively before pages() resolves.
    store2.showPage(p2.id);
    expect(store2.speculativePageId).toBeUndefined();
    await made.backend.wiki.deletePage(wiki.id, p2.id);
    pagesGate2.resolve(await realPages(wiki.id));
    await load2;
    expect(store2.openPage?.missing).toBe(true);
    expect(store2.openPage?.removed).toEqual({ by: 'others', title: 'Soon Gone' });
    expect(made.workspace.rememberedWikiPage(wiki.id)).toBeUndefined();
    store2.close();
    made.workspace.close();
  });
});
