/**
 * The sidebar on the real server: categories made, renamed, folded, ordered
 * and deleted, boards filed, placed and hidden. Every change is checked on
 * the server, and shows in the reader's other tab without a reload (the
 * server's four category messages).
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import {
  boardLink,
  boardMenu,
  boardsIn,
  categories,
  category,
  categoryMenu,
  categoryToggle,
  chooseFromMenu,
  sidebar,
} from '../e2e/shell.support.ts';

import {
  makeBoard,
  person,
  putAwayWelcomeBoard,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

async function stored(
  api: APIRequestContext,
  workspaceId: string,
): Promise<Schemas['CategoryContainers'][]> {
  const answer = await api.get(`/api/v1/w/${workspaceId}/categories`);
  expect(answer.ok()).toBe(true);
  return (await answer.json()) as Schemas['CategoryContainers'][];
}

/** The names of the reader's categories on the server, in their order. */
async function storedNames(api: APIRequestContext, workspaceId: string): Promise<string[]> {
  return (await stored(api, workspaceId))
    .sort((first, second) => first.sortOrder - second.sortOrder)
    .map((one) => one.name);
}

async function storedCategory(api: APIRequestContext, workspaceId: string, name: string) {
  return (await stored(api, workspaceId)).find((one) => one.name === name);
}

/**
 * The boards of this test a category shows. Open boards other tests made
 * are in every reader's sidebar too.
 */
async function mine(page: Page, name: string): Promise<string[]> {
  return (await boardsIn(page, name)).filter((title) => ['Alpha', 'Beta'].includes(title));
}

async function nameCategory(page: Page, name: string, button: 'Create' | 'Update') {
  const dialog = page.getByTestId('category-dialog');
  await dialog.getByLabel('Category name').fill(name);
  await dialog.getByRole('button', { name: button, exact: true }).click();
  await expect(dialog).toBeHidden();
}

test('categories and the boards under them are kept on the server and show in the other tab', async ({
  browser,
  baseURL,
}) => {
  // Two tabs through every change of the sidebar: twice the steps of most tests here,
  // and more than their time when the runner has several workers.
  test.slow();
  const base = baseURL ?? '';
  const owner = person('sidebar');
  await register(base, owner);
  const api = await signedIn(base, owner);
  await putAwayWelcomeBoard(api);
  const alpha = await makeBoard(api, { title: 'Alpha' });
  const beta = await makeBoard(api, { title: 'Beta' });
  const { workspaceId } = alpha;
  // The default category the server made, with the two new boards in it (E3).
  const [defaults] = await stored(api, workspaceId);
  expect(defaults?.type).toBe('system');
  const home = defaults?.name ?? '';
  // The sidebar names it in the reader's language, whatever the server stored.
  const general = 'General';
  expect(defaults?.containerMetadata.map((entry) => entry.containerId).sort()).toEqual(
    [alpha.boardId, beta.boardId].sort(),
  );

  // One reader, two tabs: the second shares the first one's session.
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const first = await context.newPage();
  const second = await context.newPage();
  const errors = [first, second].map(watchErrors);
  await first.goto('/');
  await signInOnPage(first, owner);
  await second.goto('/');
  await expect(boardLink(second, 'Beta')).toBeVisible();

  // A new category goes first.
  await sidebar(first).getByTestId('new-category').click();
  await nameCategory(first, 'Work', 'Create');
  // First in this tab at once, before the server's message is read.
  expect(await categories(first)).toEqual(['Work', general]);
  await expect.poll(() => categories(second)).toEqual(['Work', general]);
  expect(await storedNames(api, workspaceId)).toEqual(['Work', home]);

  // Renamed.
  await chooseFromMenu(first, categoryMenu(first, 'Work'), 'rename');
  await nameCategory(first, 'Projects', 'Update');
  await expect.poll(() => categories(second)).toEqual(['Projects', general]);
  expect(await storedNames(api, workspaceId)).toEqual(['Projects', home]);

  // The default category keeps its name: its menu has no renaming.
  await categoryMenu(first, general).click();
  await expect(first.locator('[data-action="move-up"]')).toBeVisible();
  await expect(first.locator('[data-action="rename"]')).toHaveCount(0);
  await first.keyboard.press('Escape');

  // Boards filed under it from their menus, each last.
  const projects = (await storedCategory(api, workspaceId, 'Projects'))?.id ?? '';
  await chooseFromMenu(first, boardMenu(first, 'Beta'), `move-to-${projects}`);
  await expect.poll(() => boardsIn(second, 'Projects')).toEqual(['Beta']);
  await chooseFromMenu(first, boardMenu(first, 'Alpha'), `move-to-${projects}`);
  await expect.poll(() => boardsIn(second, 'Projects')).toEqual(['Beta', 'Alpha']);
  await expect
    .poll(async () =>
      (await storedCategory(api, workspaceId, 'Projects'))?.containerMetadata.map(
        (entry) => entry.containerId,
      ),
    )
    .toEqual([beta.boardId, alpha.boardId]);

  // Placed: Alpha goes up one, by the keyboard.
  await boardLink(first, 'Alpha').focus();
  await first.keyboard.press('Space');
  await first.keyboard.press('ArrowUp');
  await first.keyboard.press('Space');
  await expect.poll(() => boardsIn(second, 'Projects')).toEqual(['Alpha', 'Beta']);
  await expect
    .poll(async () =>
      (await storedCategory(api, workspaceId, 'Projects'))?.containerMetadata.map(
        (entry) => entry.containerId,
      ),
    )
    .toEqual([alpha.boardId, beta.boardId]);

  // The categories in another order.
  await chooseFromMenu(first, categoryMenu(first, general), 'move-up');
  await expect.poll(() => categories(second)).toEqual([general, 'Projects']);
  expect(await storedNames(api, workspaceId)).toEqual([home, 'Projects']);

  // Folded.
  await categoryToggle(first, 'Projects').click();
  await expect(categoryToggle(second, 'Projects')).toHaveAttribute('aria-expanded', 'false');
  expect((await storedCategory(api, workspaceId, 'Projects'))?.collapsed).toBe(true);
  await categoryToggle(second, 'Projects').click();
  await expect(categoryToggle(first, 'Projects')).toHaveAttribute('aria-expanded', 'true');
  await expect
    .poll(async () => (await storedCategory(api, workspaceId, 'Projects'))?.collapsed)
    .toBe(false);

  // Hidden, and undo shows it again.
  await chooseFromMenu(first, boardMenu(first, 'Alpha'), 'hide');
  await expect(boardLink(second, 'Alpha')).toHaveCount(0);
  await first.getByTestId('undo').click();
  await expect.poll(() => boardsIn(second, 'Projects')).toEqual(['Alpha', 'Beta']);
  await expect
    .poll(async () => (await storedCategory(api, workspaceId, 'Projects'))?.containerMetadata)
    .toEqual([
      { containerId: alpha.boardId, hidden: false },
      { containerId: beta.boardId, hidden: false },
    ]);

  // Hidden: gone from both tabs, still filed, hidden on the server.
  await chooseFromMenu(first, boardMenu(first, 'Alpha'), 'hide');
  await expect(boardLink(second, 'Alpha')).toHaveCount(0);
  await expect(boardLink(first, 'Alpha')).toHaveCount(0);
  await expect
    .poll(async () => (await storedCategory(api, workspaceId, 'Projects'))?.containerMetadata)
    .toEqual([
      { containerId: alpha.boardId, hidden: true },
      { containerId: beta.boardId, hidden: false },
    ]);

  // Deleted: its boards go back to the default category.
  await chooseFromMenu(first, categoryMenu(first, 'Projects'), 'delete');
  await first.getByRole('alertdialog').getByRole('button', { name: 'Delete', exact: true }).click();
  // Alpha stays hidden there, in this tab too.
  expect(await categories(first)).toEqual([general]);
  expect(await mine(first, general)).toEqual(['Beta']);
  await expect.poll(() => categories(second)).toEqual([general]);
  await expect.poll(() => mine(second, general)).toEqual(['Beta']);
  expect(await storedNames(api, workspaceId)).toEqual([home]);

  // The hidden boards are listed at the foot of the sidebar; one is shown again from there.
  const hiddenList = sidebar(first).getByTestId('hidden-boards');
  await hiddenList.getByRole('button', { name: 'Hidden boards and wikis (1)' }).click();
  await hiddenList.getByRole('button', { name: 'Show Alpha in the sidebar' }).click();
  await expect(hiddenList).toHaveCount(0);
  await expect.poll(() => mine(second, general)).toContain('Alpha');
  await expect
    .poll(async () =>
      (await storedCategory(api, workspaceId, home))?.containerMetadata.find(
        (entry) => entry.containerId === alpha.boardId,
      ),
    )
    .toEqual({ containerId: alpha.boardId, hidden: false });
  await chooseFromMenu(first, boardMenu(first, 'Alpha'), 'hide');

  // As it was left, after a reload.
  await second.reload();
  await expect.poll(() => mine(second, general)).toEqual(['Beta']);
  await expect(category(second, 'Projects')).toHaveCount(0);
  await expect(first.locator('[data-problem]')).toHaveCount(0);

  await api.dispose();
  await context.close();
  expect(errors.flat()).toEqual([]);
});
