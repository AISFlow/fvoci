/**
 * A board of more cards than one page of the server holds (1000): the page
 * reads them page after page, and every card is on the board. The board is made
 * with its cards in one request, as an import makes one: a card is made on its
 * own by `createCard`, and not by the blocks of a board.
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
  const cards = Array.from({ length: MANY }, (_, at) => ({
    containerId: 'many',
    type: 'card',
    title: `Card ${String(at + 1)}`,
    createdAt: 1,
    updatedAt: 1,
    fields: { properties: { status: doing } },
  }));
  const made = await api.post(`/api/v1/w/${workspaceId}/containers-and-blocks`, {
    data: {
      containers: [
        {
          id: 'many',
          workspaceId,
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
        },
      ],
      blocks: [
        {
          containerId: 'many',
          type: 'view',
          title: 'By status',
          createdAt: 1,
          updatedAt: 1,
          fields: { viewType: 'board', groupBy: 'status', visibleGroups: [doing, ''] },
        },
        ...cards,
      ],
    },
  });
  expect(made.ok(), `the board and its cards: ${await made.text()}`).toBe(true);
  const stored = (await made.json()) as Schemas['ContainersAndBlocks'];
  const board = {
    workspaceId,
    boardId: stored.containers[0]?.id ?? '',
  };
  const viewId = stored.blocks.find((block) => block.type === 'view')?.id;

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
