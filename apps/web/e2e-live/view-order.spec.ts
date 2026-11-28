/**
 * The order of a board's views on the real server: a view moved by the keys
 * alone in the list of views, seen in the list of another member who has the
 * board open, there again after a reload, and kept by a copy of the board
 * and by a board made from it as a template. A member who may only look has
 * nothing to move them with.
 */
import { type APIRequestContext, expect, test, type Page } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US', viewport: { width: 1400, height: 900 } });

const viewsButton = (page: Page) => page.locator('[data-tool="views"]');
const listed = (page: Page) =>
  page.getByRole('menu', { name: 'Views' }).locator('a[role="menuitem"]');

/**
 * The titles of views in the board's order: by place (`fields.order`, as
 * strings compare), the identifier settling two of one place; those without
 * a place after them, by title. A space sorts before every digit of a place.
 */
function inOrder(views: Schemas['Block'][]): string[] {
  const key = (view: Schemas['Block']): string => {
    const place = (view.fields as { order?: unknown }).order;
    return typeof place === 'string' ? `0${place} ${view.id}` : `1${view.title}`;
  };
  return [...views]
    .sort((one, other) => (key(one) < key(other) ? -1 : 1))
    .map((view) => view.title);
}

/** Views titled `titles` added to the board, each like `like`. */
async function addViews(
  api: APIRequestContext,
  ws: string,
  boardId: string,
  like: Schemas['Block'] | undefined,
  titles: string[],
): Promise<void> {
  for (const title of titles) {
    const added = await api.post(`/api/v1/w/${ws}/boards/${boardId}/views`, {
      data: {
        title,
        viewType: like?.fields.viewType,
        fields: { ...like?.fields, order: undefined },
      },
    });
    expect(added.ok(), await added.text()).toBe(true);
  }
}

/** `views` left without a place, as views are that nobody has moved since the server placed none. */
async function unplace(
  api: APIRequestContext,
  ws: string,
  boardId: string,
  views: Schemas['Block'][],
): Promise<void> {
  for (const view of views) {
    const patched = await api.patch(`/api/v1/w/${ws}/boards/${boardId}/views/${view.id}`, {
      data: { deletedFields: ['order'] },
    });
    expect(patched.ok(), await patched.text()).toBe(true);
  }
}

test('a view moved by the keys is the board’s order for another member, after a reload and in copies', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('view-mover');
  const helper = person('view-watcher');
  const reader = person('view-reader');
  for (const who of [owner, helper, reader]) await register(base, who);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Views in order',
    members: [
      { who: helper, role: 'editor' },
      { who: reader, role: 'viewer' },
    ],
  });
  const ws = board.workspaceId;
  const viewsOf = async (boardId: string): Promise<Schemas['Block'][]> =>
    (await (await api.get(`/api/v1/w/${ws}/boards/${boardId}/views`)).json()) as Schemas['Block'][];
  // Three more views beside "By status", like it, none of them placed: they go by title.
  const [first] = await viewsOf(board.boardId);
  await addViews(api, ws, board.boardId, first, ['Timeline', 'Backlog', 'Roadmap']);
  await unplace(api, ws, board.boardId, await viewsOf(board.boardId));
  const before = ['Backlog', 'By status', 'Roadmap', 'Timeline'];

  const pages: Page[] = [];
  const watched: string[][] = [];
  for (const who of [owner, helper, reader]) {
    const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
    pages.push(page);
    watched.push(watchErrors(page));
    await page.goto(boardPath(board, board.viewId));
    await signInOnPage(page, who);
    await expect(viewsButton(page)).toHaveText('By status');
  }
  const [mine, theirs, looking] = pages as [Page, Page, Page];

  // The member who may only look has the list, and nothing that moves a view.
  await viewsButton(looking).click();
  await expect(listed(looking)).toHaveText(before);
  await expect(looking.locator('[data-line-tool]')).toHaveCount(0);
  await looking.keyboard.press('Escape');

  // Two places down by the keys, Alt and an arrow on its line: the list open, the focus there.
  await viewsButton(mine).focus();
  await mine.keyboard.press('Enter');
  await expect(listed(mine)).toHaveText(before);
  const backlog = listed(mine).filter({ hasText: 'Backlog' });
  await expect(backlog).toBeFocused();
  await mine.keyboard.press('Alt+ArrowDown');
  await expect(listed(mine).nth(1)).toHaveText('Backlog');
  await mine.keyboard.press('Alt+ArrowDown');
  const moved = ['By status', 'Roadmap', 'Backlog', 'Timeline'];
  await expect(listed(mine)).toHaveText(moved);
  await expect(backlog).toBeFocused();
  await expect(mine.getByText('Backlog is now 3 of 4')).toBeAttached();
  await mine.keyboard.press('Escape');
  await expect(viewsButton(mine)).toBeFocused();

  // The other member's list follows, and so does the reader's.
  await viewsButton(theirs).click();
  await expect(listed(theirs)).toHaveText(moved);
  await theirs.keyboard.press('Escape');
  await viewsButton(looking).click();
  await expect(listed(looking)).toHaveText(moved);
  await looking.keyboard.press('Escape');

  // Every view has its place on the server, and the page read anew lists them so.
  await expect.poll(async () => inOrder(await viewsOf(board.boardId))).toEqual(moved);
  await mine.reload();
  await viewsButton(mine).click();
  await expect(listed(mine)).toHaveText(moved);
  await mine.keyboard.press('Escape');

  // A copy of the board keeps the order, and so does a board made from it as a template.
  const copyOf = async (containerId: string, params: Record<string, string | boolean>) => {
    const copied = await api.post(`/api/v1/w/${ws}/containers/${containerId}/duplicate`, {
      params,
    });
    expect(copied.ok(), await copied.text()).toBe(true);
    return ((await copied.json()) as Schemas['ContainersAndBlocks']).containers[0]?.id ?? '';
  };
  const copyId = await copyOf(board.boardId, {});
  expect(inOrder(await viewsOf(copyId))).toEqual(moved);
  const templateId = await copyOf(board.boardId, { asTemplate: true });
  const fromTemplate = await copyOf(templateId, { toWorkspace: ws });
  expect(inOrder(await viewsOf(fromTemplate))).toEqual(moved);
  await mine.goto(boardPath({ workspaceId: ws, boardId: fromTemplate }));
  await viewsButton(mine).click();
  await expect(listed(mine)).toHaveText(moved);

  for (const errors of watched) expect(errors).toEqual([]);
  for (const page of pages) await page.context().close();
  await api.dispose();
});

test('two concurrent first moves on an unplaced board keep both moves', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const alice = person('view-first-a');
  const bob = person('view-first-b');
  for (const who of [alice, bob]) await register(base, who);
  const api = await signedIn(base, alice);
  const board = await makeBoard(api, {
    title: 'Concurrent first moves',
    members: [{ who: bob, role: 'editor' }],
  });
  const ws = board.workspaceId;
  const viewsOf = async (): Promise<Schemas['Block'][]> =>
    (await (
      await api.get(`/api/v1/w/${ws}/boards/${board.boardId}/views`)
    ).json()) as Schemas['Block'][];
  const [first] = await viewsOf();
  await addViews(api, ws, board.boardId, first, ['Timeline', 'Backlog', 'Roadmap']);
  await unplace(api, ws, board.boardId, await viewsOf());

  const alicePage = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const bobPage = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const watched = [watchErrors(alicePage), watchErrors(bobPage)];
  for (const [page, who] of [
    [alicePage, alice],
    [bobPage, bob],
  ] as const) {
    await page.goto(boardPath(board, board.viewId));
    await signInOnPage(page, who);
    await expect(viewsButton(page)).toHaveText('By status');
  }

  // Hold Bob's move until Alice's has landed on the server: a move names the view it goes
  // after, and the server places it there when it comes.
  let holding = true;
  let releaseBob = (): void => undefined;
  const holdBob = new Promise<void>((resolve) => {
    releaseBob = resolve;
  });
  let bobMoveHeld = (): void => undefined;
  const moveHeld = new Promise<void>((resolve) => {
    bobMoveHeld = resolve;
  });
  const movePattern = `**/api/v1/w/${ws}/boards/${board.boardId}/views/*/move`;
  await bobPage.route(movePattern, async (route) => {
    if (holding) {
      bobMoveHeld();
      await holdBob;
    }
    await route.continue();
  });

  // Bob starts moving Backlog down, after "By status"; his move is held.
  await viewsButton(bobPage).click();
  await expect(listed(bobPage)).toHaveText(['Backlog', 'By status', 'Roadmap', 'Timeline']);
  await listed(bobPage).filter({ hasText: 'Backlog' }).focus();
  await bobPage.keyboard.press('Alt+ArrowDown');
  await moveHeld;

  // Alice moves Timeline up; her move lands on the server before Bob's writes arrive.
  await viewsButton(alicePage).click();
  await listed(alicePage).filter({ hasText: 'Timeline' }).focus();
  await alicePage.keyboard.press('Alt+ArrowUp');
  await expect
    .poll(async () => inOrder(await viewsOf()))
    .toEqual(['Backlog', 'By status', 'Timeline', 'Roadmap']);

  // Release Bob's move: Backlog goes after "By status", before Timeline where Alice put it.
  holding = false;
  releaseBob();

  const both = ['By status', 'Backlog', 'Timeline', 'Roadmap'];
  await expect.poll(async () => inOrder(await viewsOf())).toEqual(both);
  await expect(listed(alicePage)).toHaveText(both);
  await expect(listed(bobPage)).toHaveText(both);
  await expect(alicePage.locator('[data-problem]')).toHaveCount(0);
  await expect(bobPage.locator('[data-problem]')).toHaveCount(0);
  for (const errors of watched) expect(errors).toEqual([]);
  await alicePage.context().close();
  await bobPage.context().close();
  await api.dispose();
});

test('a tie raise and a concurrent move of the raised view keep both moves', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const alice = person('view-tie-a');
  const bob = person('view-tie-b');
  for (const who of [alice, bob]) await register(base, who);
  const api = await signedIn(base, alice);
  const board = await makeBoard(api, {
    title: 'Concurrent tie raise',
    members: [{ who: bob, role: 'editor' }],
  });
  const ws = board.workspaceId;
  const viewsOf = async (): Promise<Schemas['Block'][]> =>
    (await (
      await api.get(`/api/v1/w/${ws}/boards/${board.boardId}/views`)
    ).json()) as Schemas['Block'][];
  const [first] = await viewsOf();
  await addViews(api, ws, board.boardId, first, ['Backlog', 'Roadmap', 'Timeline', 'Zeta']);
  const views = await viewsOf();
  const byTitle = (title: string): Schemas['Block'] => {
    const found = views.find((view) => view.title === title);
    if (!found) throw new Error(`missing view ${title}`);
    return found;
  };
  const tied = [byTitle('By status'), byTitle('Roadmap')].sort((a, b) => (a.id < b.id ? -1 : 1));
  const [firstTied, secondTied] = tied as [Schemas['Block'], Schemas['Block']];
  for (const [view, order] of [
    [byTitle('Backlog'), 'V'],
    [firstTied, 'W'],
    [secondTied, 'W'],
    [byTitle('Timeline'), 'Y'],
    [byTitle('Zeta'), 'Z'],
  ] as const) {
    const patched = await api.patch(`/api/v1/w/${ws}/boards/${board.boardId}/views/${view.id}`, {
      data: { updatedFields: { order } },
    });
    expect(patched.ok(), await patched.text()).toBe(true);
  }
  const initial = ['Backlog', firstTied.title, secondTied.title, 'Timeline', 'Zeta'];
  expect(inOrder(await viewsOf())).toEqual(initial);

  const alicePage = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const bobPage = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const watched = [watchErrors(alicePage), watchErrors(bobPage)];
  for (const [page, who] of [
    [alicePage, alice],
    [bobPage, bob],
  ] as const) {
    await page.goto(boardPath(board, board.viewId));
    await signInOnPage(page, who);
    await expect(viewsButton(page)).toHaveText('By status');
  }

  // Hold Alice's move (Backlog after firstTied, which the server places by raising
  // secondTied out of the tie) until Bob's move lands.
  let holding = true;
  let releaseRaise = (): void => undefined;
  const holdRaise = new Promise<void>((resolve) => {
    releaseRaise = resolve;
  });
  let raiseHit = (): void => undefined;
  const raiseHeld = new Promise<void>((resolve) => {
    raiseHit = resolve;
  });
  const movePattern = `**/api/v1/w/${ws}/boards/${board.boardId}/views/*/move`;
  await alicePage.route(movePattern, async (route) => {
    if (holding) {
      raiseHit();
      await holdRaise;
    }
    await route.continue();
  });

  // Alice moves Backlog down into the tie between firstTied and secondTied.
  await viewsButton(alicePage).click();
  await expect(listed(alicePage)).toHaveText(initial);
  await listed(alicePage).filter({ hasText: 'Backlog' }).focus();
  await alicePage.keyboard.press('Alt+ArrowDown');
  await raiseHeld;

  // Bob moves secondTied down past Timeline while Alice's tie-raise is held.
  await viewsButton(bobPage).click();
  await expect(listed(bobPage)).toHaveText(initial);
  await listed(bobPage).filter({ hasText: secondTied.title }).focus();
  await bobPage.keyboard.press('Alt+ArrowDown');
  await expect
    .poll(async () => inOrder(await viewsOf()))
    .toEqual(['Backlog', firstTied.title, 'Timeline', secondTied.title, 'Zeta']);

  // Release Alice's move: Backlog goes after firstTied, before Timeline now.
  holding = false;
  releaseRaise();

  // Wait for Alice's held move to settle and confirm Bob's move was not overwritten.
  const both = [firstTied.title, 'Backlog', 'Timeline', secondTied.title, 'Zeta'];
  await expect(listed(alicePage)).toHaveText(both);
  await expect(listed(bobPage)).toHaveText(both);
  await expect.poll(async () => inOrder(await viewsOf())).toEqual(both);
  await alicePage.reload();
  await viewsButton(alicePage).click();
  await expect(listed(alicePage)).toHaveText(both);
  await expect(alicePage.locator('[data-problem]')).toHaveCount(0);
  await expect(bobPage.locator('[data-problem]')).toHaveCount(0);
  for (const errors of watched) expect(errors).toEqual([]);
  await alicePage.context().close();
  await bobPage.context().close();
  await api.dispose();
});
