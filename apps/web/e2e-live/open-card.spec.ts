/**
 * A card open while somebody else changes what the reader may do with it, on
 * the real server: taken off the board or out of its workspace, the card
 * closes and the page says the board is not theirs to see; made a viewer, the body turns read-only and
 * says why; the card deleted, it says the card is gone, and once restored it
 * holds what was typed last. All while the admin is in the same card in
 * another browser.
 */
import path from 'node:path';

import { expect, test, type Browser, type Page } from '@playwright/test';

import { screensDir } from '../e2e/screens.ts';
import {
  ADMIN,
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
} from './support.ts';

test.use({ locale: 'en-US' });

/** How soon the reader's page follows the change: the server tells the room at once. */
const PROMPTLY = { timeout: 5_000 };

function editor(page: Page) {
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

/** The page with the card open, signed in as `who`, and what it throws from the start. */
async function cardOpen(
  browser: Browser,
  address: string,
  who: Person,
): Promise<{ page: Page; errors: string[] }> {
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const errors = watchErrors(page);
  await page.goto(address);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return { page, errors };
}

/** The pictures of the screen, as wide as the page is and 390 pixels wide, with `FVOCI_SCREENS=1`. */
async function shot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const folder = screensDir('collab-r4');
  const wide = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.screenshot({ path: path.join(folder, `${name}-wide.png`) });
  await page.setViewportSize({ width: 390, height: 844 });
  // The frame of the card is laid out again for the narrow page before the picture.
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
  );
  await page.screenshot({ path: path.join(folder, `${name}-390.png`) });
  await page.setViewportSize(wide);
}

/** The admin and a colleague with the card open in two browsers, the colleague an editor. */
async function bothInTheCard(browser: Browser, base: string, title: string) {
  const admin = person('admin');
  const colleague = person('colleague');
  await register(base, admin);
  await register(base, colleague);
  const api = await signedIn(base, admin);
  const board = await makeBoard(api, {
    title,
    cards: [{ title: 'Agenda', status: 'To do' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards.Agenda ?? '';
  const address = boardPath(board, board.viewId, cardId);
  const [first, second] = [
    await cardOpen(browser, address, admin),
    await cardOpen(browser, address, colleague),
  ];
  const found = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/users`, {
      params: { search: colleague.username },
    })
  ).json()) as Schemas['User'][];
  const colleagueId = found.find((user) => user.username === colleague.username)?.id ?? '';
  const memberPath = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/members/${colleagueId}`;
  return {
    api,
    first: first.page,
    second: second.page,
    // Read at the end: errors that come later are in them too.
    watched: [first.errors, second.errors],
    memberPath,
    colleagueId,
    board,
    cardId,
  };
}

test('taken off the board while the card is open: the card closes, and the page says why', async ({
  browser,
  baseURL,
}) => {
  const { api, first, second, watched, memberPath } = await bothInTheCard(
    browser,
    baseURL ?? '',
    'Taken off',
  );
  await editor(second).click();
  await second.keyboard.type('Bora was here');
  await expect(editor(first)).toContainText('Bora was here');

  expect((await api.delete(memberPath)).ok()).toBe(true);
  await expect(second.getByTestId('card-dialog')).toHaveCount(0, PROMPTLY);
  await expect(second.getByTestId('not-found')).toBeVisible();
  await shot(second, 'taken-off');

  // The admin goes on in the card, alone.
  await editor(first).click();
  await first.keyboard.press('End');
  await first.keyboard.type(' and left');
  await expect(editor(first)).toContainText('Bora was here and left');
  await expect(first.getByTestId('body-room-note')).toHaveCount(0);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('made a viewer while the card is open: the body turns read-only, and says why', async ({
  browser,
  baseURL,
}) => {
  const { api, first, second, watched, memberPath } = await bothInTheCard(
    browser,
    baseURL ?? '',
    'Made a viewer',
  );
  const role = await api.put(memberPath, {
    data: {
      role: 'viewer',
    },
  });
  expect(role.ok()).toBe(true);
  await expect(second.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading', PROMPTLY);
  await expect(second.getByTestId('body-room-note')).toHaveText(
    'You can read this description but not change it.',
  );
  await expect(second.getByTestId('card-body').locator('[contenteditable="true"]')).toHaveCount(0);
  await shot(second, 'made-viewer');

  // Still in the room: what the admin types shows, and nothing typed here goes anywhere.
  await editor(first).click();
  await first.keyboard.type('Decided: Monday');
  await expect(editor(second)).toHaveText('Decided: Monday');
  await editor(second).click({ force: true });
  await second.keyboard.type('nope');
  // What the admin types after shows on both sides, and the viewer's typing never does.
  await first.keyboard.type(' at noon');
  await expect(editor(second)).toHaveText('Decided: Monday at noon');
  await expect(editor(first)).toHaveText('Decided: Monday at noon');
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('deleted while open: the card says it is gone, and restored it holds what was typed last', async ({
  browser,
  baseURL,
}) => {
  const { api, first, second, watched, board, cardId } = await bothInTheCard(
    browser,
    baseURL ?? '',
    'Deleted meanwhile',
  );
  await editor(second).click();
  await second.keyboard.type('the last words');
  await expect(editor(first)).toContainText('the last words');

  // At once, before the room has written the body to the card.
  const card = `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks/${cardId}`;
  expect((await api.delete(card)).ok()).toBe(true);
  await expect(second.getByTestId('card-missing')).toBeVisible(PROMPTLY);
  await shot(second, 'deleted');
  await second.getByTestId('card-missing').getByRole('button', { name: 'Close' }).click();
  await expect(second.getByTestId('card-dialog')).toHaveCount(0);

  expect((await api.post(`${card}/restore`)).ok()).toBe(true);
  const restored = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  expect(JSON.stringify(restored.body)).toContain('the last words');
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('taken out of the workspace while the card is open: the card closes, and the page says why', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { api, first, second, watched, board, colleagueId } = await bothInTheCard(
    browser,
    base,
    'Left the workspace',
  );
  // The workspace's administrator takes the colleague out: no board is told, the room is.
  const admin = await signedIn(base, ADMIN);
  const out = await admin.delete(`/api/v1/w/${board.workspaceId}/members/${colleagueId}`);
  expect(out.ok()).toBe(true);
  await expect(second.getByTestId('card-dialog')).toHaveCount(0, PROMPTLY);
  await expect(second.getByTestId('not-found')).toBeVisible();
  await shot(second, 'left-workspace');
  await first.context().close();
  await second.context().close();
  await admin.dispose();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});
