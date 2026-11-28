/**
 * The trash on the real server: the boards a reader was an admin of, from the
 * sidebar, and the cards of a board, from its top bar for its editors. Each
 * says who deleted it and when; one brought back is where it was (a board in
 * its category and place, in the reader's other tab too); and those who may
 * not bring one back are not shown it.
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import { boardsIn } from '../e2e/shell.support.ts';

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

function column(page: Page, name: string) {
  return page.locator('[data-column]').filter({
    has: page.locator('h3').filter({ hasText: new RegExp(`^\\s*${name}\\s*$`) }),
  });
}

async function json<T>(answer: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
  expect(answer.ok(), await answer.text()).toBe(true);
  return (await answer.json()) as T;
}

async function openTrash(page: Page): Promise<void> {
  await page.getByTestId('open-trash').click();
  await expect(page.getByTestId('board-trash')).toBeVisible();
}

test('a deleted board is listed for who was its admin, and comes back to its place in the sidebar', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('keeper');
  const helper = person('helper');
  await register(base, owner);
  await register(base, helper);
  const api = await signedIn(base, owner);
  const helperApi = await signedIn(base, helper);
  // A row names who deleted it as people are named everywhere.
  await api.patch('/api/v1/users/me', { data: { customName: 'Kay Keeper' } });
  const alpha = await makeBoard(api, { title: 'Alpha' });
  const beta = await makeBoard(api, {
    title: 'Beta',
    members: [{ who: helper, role: 'editor' }],
  });
  const gamma = await makeBoard(api, { title: 'Gamma' });
  const { workspaceId } = alpha;
  const ids = [alpha.boardId, beta.boardId, gamma.boardId];
  const projects = await json<Schemas['Category']>(
    await api.post(`/api/v1/w/${workspaceId}/categories`, {
      data: {
        name: 'Projects',
        workspaceId,
        userId: (await json<Schemas['User']>(await api.get('/api/v1/users/me'))).id,
      },
    }),
  );
  for (const boardId of ids) {
    const filed = await api.post(
      `/api/v1/w/${workspaceId}/categories/${projects.id}/containers/${boardId}`,
    );
    expect(filed.ok(), await filed.text()).toBe(true);
  }
  const ordered = await api.put(
    `/api/v1/w/${workspaceId}/categories/${projects.id}/container-order`,
    {
      data: ids,
    },
  );
  expect(ordered.ok(), await ordered.text()).toBe(true);
  const order = async (): Promise<string[]> =>
    (
      (
        await json<Schemas['CategoryContainers'][]>(
          await api.get(`/api/v1/w/${workspaceId}/categories`),
        )
      ).find((one) => one.id === projects.id)?.containerMetadata ?? []
    ).map((entry) => entry.containerId);
  expect(await order()).toEqual(ids);

  const deleted = await api.delete(`/api/v1/w/${beta.workspaceId}/containers/${beta.boardId}`);
  expect(deleted.ok()).toBe(true);
  expect(await order()).toEqual([alpha.boardId, gamma.boardId]);

  // The helper was an editor of Beta: their trash does not have it, on the server or the page.
  const theirs = await json<Schemas['DeletedItem'][]>(
    await helperApi.get(`/api/v1/w/${workspaceId}/trash/containers`),
  );
  expect(theirs.map((item) => item.id)).not.toContain(beta.boardId);
  const helperPage = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const helperErrors = watchErrors(helperPage);
  await helperPage.goto('/');
  await signInOnPage(helperPage, helper);
  await openTrash(helperPage);
  await expect(helperPage.getByTestId('trash-empty')).toBeVisible();
  await expect(helperPage.locator('[data-trash-item="Beta"]')).toHaveCount(0);

  // The owner, in two tabs.
  const pages: Page[] = [];
  for (let tab = 0; tab < 2; tab += 1) {
    const page = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
    await page.goto(boardPath(alpha, alpha.viewId));
    await signInOnPage(page, owner);
    pages.push(page);
  }
  const [page, other] = pages as [Page, Page];
  const errors = pages.map(watchErrors);
  for (const each of pages) {
    await expect.poll(() => boardsIn(each, 'Projects')).toEqual(['Alpha', 'Gamma']);
  }

  await openTrash(page);
  const row = page.locator('[data-trash-item="Beta"]');
  await expect(row).toContainText(`Deleted by Kay Keeper (@${owner.username})`);
  await row.getByRole('button', { name: 'Restore Beta' }).click();
  await expect(row).toHaveCount(0);
  await expect.poll(order).toEqual(ids);
  // Back between the boards it was between, in both tabs, without a reload.
  for (const each of pages) {
    await expect.poll(() => boardsIn(each, 'Projects')).toEqual(['Alpha', 'Beta', 'Gamma']);
  }
  await expect(other.locator('[data-problem]')).toHaveCount(0);

  for (const one of [api, helperApi]) await one.dispose();
  expect([...errors, helperErrors].flat()).toEqual([]);
});

test('a deleted card is listed for the editors of its board, and comes back to the board', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('pruner');
  const editor = person('mender');
  const viewer = person('onlooker');
  for (const who of [owner, editor, viewer]) await register(base, who);
  const api = await signedIn(base, owner);
  const viewerApi = await signedIn(base, viewer);
  const board = await makeBoard(api, {
    title: 'Pruned',
    cards: [
      { title: 'Keep me', status: 'To do' },
      { title: 'Throw me out', status: 'To do' },
    ],
    members: [
      { who: editor, role: 'editor' },
      { who: viewer, role: 'viewer' },
    ],
  });
  const gone = board.cards['Throw me out'] ?? '';
  expect(
    (
      await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/batch-delete`, {
        data: { cardIds: [gone] },
      })
    ).ok(),
  ).toBe(true);
  const path = boardPath(board, board.viewId);

  // A viewer may not bring cards back: the server refuses the list, and the page offers none.
  expect(
    (
      await viewerApi.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/trash/cards`)
    ).status(),
  ).toBe(403);
  const viewerPage = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const viewerErrors = watchErrors(viewerPage);
  await viewerPage.goto(path);
  await signInOnPage(viewerPage, viewer);
  await expect(column(viewerPage, 'To do').locator('[data-card]')).toHaveCount(1);
  await expect(viewerPage.getByTestId('open-card-trash')).toHaveCount(0);

  // The editor sees it, says who deleted it, and brings it back.
  const page = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const errors = watchErrors(page);
  await page.goto(path);
  await signInOnPage(page, editor);
  await expect(column(page, 'To do').locator('[data-card]')).toHaveCount(1);
  await page.getByTestId('open-card-trash').click();
  const dialog = page.getByTestId('card-trash');
  const row = dialog.locator('[data-trash-item="Throw me out"]');
  await expect(row).toContainText(`Deleted by ${owner.username}`);
  await row.getByRole('button', { name: 'Restore Throw me out' }).click();
  await expect(row).toHaveCount(0);
  await expect(dialog.getByTestId('trash-empty')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(column(page, 'To do').getByText('Throw me out')).toBeVisible();
  // The viewer's board shows it too, as any card that changes.
  await expect(column(viewerPage, 'To do').locator('[data-card]')).toHaveCount(2);
  const cards = await json<Schemas['Card'][]>(
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
      params: { perPage: 1000 },
    }),
  );
  expect(cards.map((card) => card.title).sort()).toEqual(['Keep me', 'Throw me out']);

  for (const one of [api, viewerApi]) await one.dispose();
  expect([...errors, ...viewerErrors]).toEqual([]);
});
