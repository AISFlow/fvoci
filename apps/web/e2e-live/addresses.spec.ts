/**
 * The addresses on the real server: every one starts at a workspace
 * (`/w/<workspace>/boards/<board>/<view>/<card>`), the root takes the reader
 * to one of theirs, and an address that does not fit is not found, the same
 * whatever does not fit.
 */
import { expect, test, type Page } from '@playwright/test';

import { addressOf, ON_A_VIEW } from '../e2e/address.ts';
import { boardLink, sidebar } from '../e2e/shell.support.ts';

import {
  addWorkspace,
  boardPath,
  joinWorkspace,
  madeABoard,
  makeBoard,
  person,
  placeOf,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
} from './support.ts';

test.use({ locale: 'en-US' });

const NOT_THERE = '01930000-0000-7000-8000-00000000ffff';

/**
 * Signs out, and waits for the page to start again at the root: the sign-in
 * form may show a moment before that, still at the address of the one before.
 */
async function signOut(page: Page): Promise<void> {
  await page.getByTestId('open-settings').click();
  await page.getByTestId('settings-dialog').getByRole('button', { name: 'Sign out' }).click();
  await page.waitForURL((url) => url.pathname === '/');
  await expect(page.getByTestId('sign-in-form')).toBeVisible();
}

async function readerWithBoard(base: string): Promise<{
  who: Person;
  board: Awaited<ReturnType<typeof makeBoard>>;
}> {
  const who = person('addresses');
  await register(base, who);
  const api = await signedIn(base, who);
  const board = await makeBoard(api, {
    title: 'Home board',
    cards: [{ title: 'Paint the fence', status: 'To do' }],
  });
  await api.dispose();
  return { who, board };
}

test('signing in lands in a workspace of the reader; a card’s address opens it again after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { who, board } = await readerWithBoard(base);
  const errors = watchErrors(page);

  await page.goto('/');
  await signInOnPage(page, who);
  // The workspace of the reader (shared with the others of this server), on its first board.
  await expect(page).toHaveURL(ON_A_VIEW);
  expect(placeOf(page.url()).workspaceId).toBe(board.workspaceId);
  await expect(page.getByTestId('board-page')).toBeVisible();

  await boardLink(page, 'Home board').click();
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, board.viewId)}$`));
  await page.getByTestId('board-page').getByText('Paint the fence').click();
  const cardId = board.cards['Paint the fence'] ?? '';
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, board.viewId, cardId)}$`));
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();

  await page.reload();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('card-title')).toHaveValue('Paint the fence');
  expect(placeOf(page.url())).toEqual({
    workspaceId: board.workspaceId,
    boardId: board.boardId,
    viewId: board.viewId,
    cardId,
  });
  expect(errors).toEqual([]);
});

test('an address that does not fit is not found, the same whatever does not fit', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { who, board } = await readerWithBoard(base);
  // A second workspace of the reader, with a board of its own.
  const second = addWorkspace(base, who, 'Second');
  const api = await signedIn(base, who);
  const there = await makeBoard(api, { title: 'Away board', workspaceId: second });
  await api.dispose();
  // A workspace of somebody else.
  const stranger = person('stranger');
  await register(base, stranger);
  const theirs = addWorkspace(base, stranger, 'Theirs');

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, who);
  await expect(page.getByTestId('board-page')).toBeVisible();

  const notFound = page.getByTestId('not-found');
  const seen: string[] = [];
  for (const address of [
    // No such workspace, with a board the reader has.
    boardPath({ workspaceId: NOT_THERE, boardId: board.boardId }, board.viewId),
    // A workspace that is not the reader's.
    addressOf({ workspaceId: theirs }),
    // A board of the reader's other workspace, at this one's address.
    boardPath({ workspaceId: board.workspaceId, boardId: there.boardId }),
    // No such board.
    boardPath({ workspaceId: board.workspaceId, boardId: NOT_THERE }),
  ]) {
    await page.goto(address);
    await expect(notFound).toBeVisible();
    await expect(page.getByTestId('board-page')).toHaveCount(0);
    await expect(page.getByText('Away board')).toHaveCount(0);
    seen.push(await notFound.innerText());
  }
  expect(new Set(seen).size).toBe(1);

  // At its own workspace's address it is there.
  await page.goto(boardPath(there));
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${boardPath(there, there.viewId)}$`));
  expect(errors).toEqual([]);
});

test('an open board of another workspace that the reader has not joined is not found here, and can be joined there', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { who, board } = await readerWithBoard(base);
  const second = addWorkspace(base, who, 'Second');
  // Somebody else of the second workspace makes an open board there.
  const maker = person('maker');
  await register(base, maker);
  joinWorkspace(base, maker, second);
  const api = await signedIn(base, maker);
  const open = await makeBoard(api, {
    title: 'Open elsewhere',
    visibility: 'open',
    workspaceId: second,
  });
  await api.dispose();

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, who);
  await expect(page.getByTestId('board-page')).toBeVisible();

  // At this workspace's address: not found, as any board that is not there; nothing to join.
  await page.goto(boardPath({ workspaceId: board.workspaceId, boardId: open.boardId }));
  await expect(page.getByTestId('not-found')).toBeVisible();
  await expect(page.getByTestId('board-join')).toHaveCount(0);
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  // At its own: it opens, to be joined by the first write on it.
  await page.goto(boardPath(open));
  await expect(page.getByTestId('board-page')).toBeVisible();
  expect(errors).toEqual([]);
});

test('a board found in another workspace opens there, and nothing of the workspace left stays', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { who, board } = await readerWithBoard(base);
  const second = addWorkspace(base, who, 'Second');
  const api = await signedIn(base, who);
  const word = `plum${Date.now().toString(36)}`;
  const there = await makeBoard(api, { title: `Away ${word}`, workspaceId: second });
  await api.dispose();

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, who);
  await expect(boardLink(page, 'Home board')).toBeVisible();
  await page.getByTestId('board-page').getByText('Paint the fence').click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await page.goBack();
  await expect(page.getByTestId('card-dialog')).toBeHidden();

  // The search looks in every workspace; the board found is linked at its own.
  await sidebar(page).getByTestId('sidebar-search').fill(word);
  const found = sidebar(page)
    .getByTestId('search-more')
    .getByRole('link', { name: `Away ${word}` });
  await expect(found).toHaveAttribute('href', boardPath(there));
  await found.click();
  await expect(page).toHaveURL(new RegExp(`${boardPath(there, there.viewId)}$`));
  await sidebar(page).getByTestId('sidebar-search').fill('');
  await expect(boardLink(page, `Away ${word}`)).toBeVisible();
  await expect(boardLink(page, 'Home board')).toHaveCount(0);
  await expect(page.getByText('Paint the fence')).toHaveCount(0);

  // Back: the first workspace again, as it was.
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, board.viewId)}$`));
  await expect(boardLink(page, 'Home board')).toBeVisible();
  await expect(boardLink(page, `Away ${word}`)).toHaveCount(0);
  await expect(page.getByText('Paint the fence')).toBeVisible();

  // Forward, and then the root: the workspace last entered on this browser.
  await page.goForward();
  await expect(boardLink(page, `Away ${word}`)).toBeVisible();
  await page.goto('/');
  await expect(page).toHaveURL(new RegExp(`${boardPath(there, there.viewId)}$`));
  expect(errors).toEqual([]);
});

test('the workspace kept on this browser is each person’s own', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  const { who, board } = await readerWithBoard(base);
  const second = addWorkspace(base, who, 'Second');
  const api = await signedIn(base, who);
  const there = await makeBoard(api, { title: 'Away board', workspaceId: second });
  await api.dispose();
  const other = await readerWithBoard(base);
  // The other works in the second workspace too: a kept workspace that was not
  // their own choice would take them there.
  joinWorkspace(base, other.who, second);

  await page.goto(boardPath(there));
  await signInOnPage(page, who);
  await expect(page.getByTestId('board-page')).toBeVisible();
  await signOut(page);

  // Somebody else signs in here: their own workspace, not the one kept for the first.
  await signInOnPage(page, other.who);
  await expect(page).toHaveURL(ON_A_VIEW);
  expect(placeOf(page.url()).workspaceId).toBe(other.board.workspaceId);
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(boardLink(page, 'Away board')).toHaveCount(0);
  await signOut(page);

  // The first again, at the root: where they were last.
  await signInOnPage(page, who);
  await expect(page).toHaveURL(new RegExp(`${boardPath(there, there.viewId)}$`));
  expect(board.workspaceId).not.toBe(second);
});

test('a board made in a workspace that was left meanwhile does not take the reader back to it', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('late-board');
  await register(base, who);
  const second = addWorkspace(base, who, 'Second');
  const api = await signedIn(base, who);
  const home = await makeBoard(api, { title: 'Home board' });
  const word = `away${Date.now().toString(36)}`;
  const away = await makeBoard(api, { title: word, workspaceId: second });
  await api.dispose();
  await page.goto(boardPath(home, home.viewId));
  await signInOnPage(page, who);
  await expect(page.getByTestId('board-page')).toBeVisible();

  // The answer to making a board is held until the reader is in the other workspace.
  let received = (): void => undefined;
  const started = new Promise<void>((resolve) => {
    received = resolve;
  });
  let release = (): void => undefined;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/boards', async (route) => {
    if (route.request().method() !== 'POST') {
      await route.fallback();
      return;
    }
    received();
    await held;
    await route.continue();
  });
  try {
    await sidebar(page).getByTestId('add-board').click();
    await page.getByTestId('template-picker').getByTestId('empty-board').click();
    await started;

    await sidebar(page).getByTestId('sidebar-search').fill(word);
    await boardLink(page, word).click();
    await expect(page).toHaveURL(new RegExp(`${boardPath(away, away.viewId)}$`));
    await expect(page.getByTestId('board-page')).toBeVisible();

    const answered = page.waitForResponse(madeABoard);
    release();
    await answered;
    await page.waitForLoadState('networkidle');
    await expect(page).toHaveURL(new RegExp(`${boardPath(away, away.viewId)}$`));
    await expect(boardLink(page, 'Home board')).toHaveCount(0);
  } finally {
    release();
  }
});
