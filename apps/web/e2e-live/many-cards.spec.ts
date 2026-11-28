/**
 * A board of more cards than one page of the server holds (1000): the page
 * reads them page after page, and every card is on the board. Each card is
 * made on its own by `createCard`.
 */
import { expect, test } from '@playwright/test';

import { boardPath, person, register, type Schemas, signedIn, watchErrors } from './support.ts';

test.use({ locale: 'en-US' });

/** One more than the most cards a page of the server holds. */
const MANY = 1001;

test('every card of a board of more than a thousand is shown', async ({ page, baseURL }) => {
  test.setTimeout(120_000);
  const base = baseURL ?? '';
  const owner = person('many');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const workspaces = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const workspaceId = workspaces[0]?.id ?? '';
  const doing = 'opt-doing';
  const made = await api.post(`/api/v1/w/${workspaceId}/boards`, {
    data: {
      title: 'Many',
      visibility: 'private',
      cardProperties: [
        {
          id: 'status',
          name: 'Status',
          type: 'select',
          options: [{ id: doing, label: 'Doing', color: 'blue' }],
        },
      ],
      firstView: {
        title: 'By status',
        viewType: 'board',
        fields: { viewType: 'board', groupBy: 'status', visibleGroups: [doing, ''] },
      },
    },
  });
  expect(made.ok(), `the board: ${await made.text()}`).toBe(true);
  const board = { workspaceId, boardId: ((await made.json()) as Schemas['Container']).id };
  const views = (await (
    await api.get(`/api/v1/w/${workspaceId}/boards/${board.boardId}/views`)
  ).json()) as Schemas['Block'][];
  const viewId = views[0]?.id;
  // A few cards at a time, each as `createCard` makes one.
  const titles = Array.from({ length: MANY }, (_, at) => `Card ${String(at + 1)}`);
  for (let at = 0; at < titles.length; at += 20) {
    await Promise.all(
      titles.slice(at, at + 20).map(async (title) => {
        const card = await api.post(`/api/v1/w/${workspaceId}/boards/${board.boardId}/cards`, {
          data: { title, properties: { status: doing } },
        });
        expect(card.ok(), `${title}: ${await card.text()}`).toBe(true);
      }),
    );
  }

  // The page is signed in as the client of the API is. Signing in on the page is no part of this,
  // and the shell is drawn in one breath with the cards: a wait for it is a wait for a thousand
  // cards, which are the work of seconds and of more on a busy machine.
  await page.context().addCookies((await api.storageState()).cookies);
  const errors = watchErrors(page);
  const lastPage = page.waitForResponse((answer) => answer.url().includes('/cards?page=1&'));
  await page.goto(boardPath(board, viewId));
  expect((await lastPage).ok(), 'the second page of cards').toBe(true);
  await expect(page.getByTestId('board-page')).toBeVisible();
  const column = page.locator('[data-column]').filter({
    has: page.locator('h3').filter({ hasText: /^\s*Doing\s*$/ }),
  });
  await expect(column.locator('[data-column-count] [aria-hidden="true"]')).toHaveText(String(MANY));
  expect(errors).toEqual([]);
});
