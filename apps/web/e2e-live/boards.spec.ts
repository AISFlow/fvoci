/**
 * Boards made and changed on the page, on the real server: "Add board"
 * makes the board and its first view in one request, its title and a new
 * view are kept, and a deleted board comes back with undo.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { ON_A_VIEW } from '../e2e/address.ts';
import { boardLink, boardMenu, chooseFromMenu, sidebar } from '../e2e/shell.support.ts';

import {
  madeABoard,
  person,
  placeOf,
  putAwayWelcomeBoard,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

async function views(
  api: APIRequestContext,
  { workspaceId, boardId }: { workspaceId: string; boardId: string },
): Promise<Schemas['Block'][]> {
  const answer = await api.get(`/api/v1/w/${workspaceId}/boards/${boardId}/views`);
  expect(answer.ok()).toBe(true);
  return (await answer.json()) as Schemas['Block'][];
}

test('a board is made with its first view, changed, deleted and brought back', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('maker');
  await register(base, owner);
  const api = await signedIn(base, owner);
  // The board made here is the one board: deleted, there is none to show in its place.
  await putAwayWelcomeBoard(api);

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, owner);

  // "Add board", an empty one: the board, its "Status", "Assignee" and "Due date", and a
  // kanban grouped by its "Status", made together by the server.
  const made = page.waitForResponse(madeABoard);
  await sidebar(page).getByTestId('add-board').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await made;
  await expect(page).toHaveURL(ON_A_VIEW);
  const { workspaceId, boardId, viewId } = placeOf(page.url());
  const board = (await (
    await api.get(`/api/v1/w/${workspaceId}/containers/${boardId}`)
  ).json()) as Schemas['Container'];
  const status = board.cardProperties.find((template) => template.type === 'select');
  expect(status?.name).toBe('Status');
  const [first, ...others] = await views(api, { workspaceId, boardId });
  expect(others).toEqual([]);
  expect(first).toMatchObject({ id: viewId, title: 'Board view' });
  expect(first?.fields).toMatchObject({ viewType: 'board', groupBy: status?.id });
  // Filed under the reader's default category by the server.
  const categories = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/categories`)
  ).json()) as Schemas['CategoryContainers'][];
  expect(
    categories.flatMap((one) => one.containerMetadata.map((entry) => entry.containerId)),
  ).toContain(boardId);

  // Its title.
  const title = page.getByTestId('top-bar').getByTestId('board-title');
  await title.fill('Launch plan');
  await title.press('Enter');
  await expect(boardLink(page, 'Launch plan')).toBeVisible();
  await expect
    .poll(
      async () =>
        (
          (await (
            await api.get(`/api/v1/w/${workspaceId}/containers/${boardId}`)
          ).json()) as Schemas['Container']
        ).title,
    )
    .toBe('Launch plan');

  // A second view.
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add table view' }).click();
  await expect(page).not.toHaveURL(new RegExp(`/${viewId}$`));
  await expect
    .poll(async () =>
      (await views(api, { workspaceId, boardId })).map((view) => view.fields.viewType).sort(),
    )
    .toEqual(['board', 'table']);
  // The address keeps the view chosen: a reload shows it again.
  const header = page.getByTestId('view-header');
  await expect(header.locator('[data-tool="views"]')).toHaveText('Table view');
  await page.reload();
  await expect(header.locator('[data-tool="views"]')).toHaveText('Table view');
  await expect(page.getByTestId('table-view')).toBeVisible();

  // Deleted: gone from the server; undo brings it back.
  await chooseFromMenu(page, boardMenu(page, 'Launch plan'), 'delete');
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(boardLink(page, 'Launch plan')).toHaveCount(0);
  await expect
    .poll(async () => (await api.get(`/api/v1/w/${workspaceId}/containers/${boardId}`)).status())
    .toBe(404);
  await expect(page.getByTestId('not-found')).toBeVisible();
  await page.getByTestId('undo').click();
  await expect(boardLink(page, 'Launch plan')).toBeVisible();
  // The page, which showed it as gone, shows it again.
  await expect(title).toHaveValue('Launch plan');
  await expect(page.getByTestId('not-found')).toHaveCount(0);
  expect((await api.get(`/api/v1/w/${workspaceId}/containers/${boardId}`)).status()).toBe(200);
  expect(await views(api, { workspaceId, boardId })).toHaveLength(2);

  // Brought back, it is filed nowhere; hidden, it is filed first and stays hidden.
  await chooseFromMenu(page, boardMenu(page, 'Launch plan'), 'hide');
  await expect
    .poll(async () => {
      const categories = (await (
        await api.get(`/api/v1/w/${board.workspaceId}/categories`)
      ).json()) as Schemas['CategoryContainers'][];
      return categories
        .flatMap((one) => one.containerMetadata)
        .find((entry) => entry.containerId === boardId);
    })
    .toEqual({ containerId: boardId, hidden: true });
  await page.waitForTimeout(500);
  await expect(boardLink(page, 'Launch plan')).toHaveCount(0);
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  await api.dispose();
  expect(errors).toEqual([]);
});
