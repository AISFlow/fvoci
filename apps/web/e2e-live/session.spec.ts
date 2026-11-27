/**
 * The server's answers the web app follows (`contracts/websocket.md`
 * 3): a category message goes to every connection of its user, so every
 * tab's sidebar follows; and a session that ends closes its
 * connections, so a page on it asks to sign in by itself.
 */
import { expect, test, type Page } from '@playwright/test';

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

test.use({ locale: 'en-US' });

/**
 * Settles once the page's connection has subscribed to the workspace: the
 * server tells a connection only what happens after that (it has no
 * answer to wait for). Set up before the page opens.
 */
function subscribed(page: Page): Promise<void> {
  return new Promise((resolve) => {
    page.on('websocket', (socket) => {
      socket.on('framesent', (frame) => {
        if (typeof frame.payload === 'string' && frame.payload.includes('subscribeWorkspace')) {
          resolve();
        }
      });
    });
  });
}

test('a category made elsewhere shows in the sidebar of every tab of its user', async ({
  context,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('sorter');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Sorted', cards: [] });
  const first = await context.newPage();
  const second = await context.newPage();
  const watched = [watchErrors(first), watchErrors(second)];
  const listening = Promise.all([subscribed(first), subscribed(second)]);
  await first.goto(boardPath(board, board.viewId));
  await signInOnPage(first, owner);
  // The same browser: the second tab has the session too.
  await second.goto(boardPath(board, board.viewId));
  await expect(second.getByTestId('board-page')).toBeVisible();
  await listening;

  const me = (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
  const made = await api.post(`/api/v1/w/${board.workspaceId}/categories`, {
    data: { name: 'Errands', workspaceId: board.workspaceId, userId: me.id, type: 'custom' },
  });
  expect(made.ok(), await made.text()).toBe(true);
  const category = (await made.json()) as Schemas['Category'];
  for (const page of [first, second]) {
    await expect(page.locator(`[data-category="${category.id}"]`)).toContainText('Errands');
  }
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('signing out in one tab ends the session of the other, which asks to sign in by itself', async ({
  context,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('twotabs');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Two tabs', cards: [] });
  await api.dispose();
  const first = await context.newPage();
  const second = await context.newPage();
  const watched = [watchErrors(first), watchErrors(second)];
  const listening = Promise.all([subscribed(first), subscribed(second)]);
  await first.goto(boardPath(board, board.viewId));
  await signInOnPage(first, owner);
  await second.goto(boardPath(board, board.viewId));
  await expect(second.getByTestId('board-page')).toBeVisible();
  await listening;

  // Signed out in the first tab: the server closes the session's connections.
  const out = await first.request.post('/api/v1/logout', {
    headers: { 'X-Requested-With': 'XMLHttpRequest' },
  });
  expect(out.ok()).toBe(true);
  // The second tab, untouched, follows: its connection closed, it reads again, and is told.
  await expect(second.getByTestId('sign-in-form')).toBeVisible({ timeout: 15_000 });
  expect(watched.flat()).toEqual([]);
});
