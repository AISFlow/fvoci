/**
 * Sharing a board on the real server, from its share dialog: people of the
 * workspace found by name and added, a member's role changed and the member
 * taken off, the board opened to the workspace with a minimum role, and the
 * public link turned on (the reader's address, `/shared/<board>?r=<token>`),
 * made anew and off. Each is checked on the server;
 * the person added sees the board without a reload.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { boardLink } from '../e2e/shell.support.ts';

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

async function members(api: APIRequestContext, board: { workspaceId: string; boardId: string }) {
  return (await (
    await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/members`)
  ).json()) as Schemas['ContainerMember'][];
}

async function sharing(api: APIRequestContext, board: { workspaceId: string; boardId: string }) {
  return (await (
    await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/public-link`)
  ).json()) as Schemas['PublicLink'];
}

test('a board is shared from its dialog, and every change is kept on the server', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  // The workspace's oldest ten are offered before anything is typed; the guest
  // is past them and is found by name.
  for (let made = 0; made < 10; made += 1) await register(base, person('crowd'));
  const owner = person('sharer');
  const guest = person('guest');
  await register(base, owner);
  await register(base, guest);
  const api = await signedIn(base, owner);
  const guestApi = await signedIn(base, guest);
  const guestId = ((await (await guestApi.get('/api/v1/users/me')).json()) as Schemas['User']).id;
  const board = await makeBoard(api, { title: 'Garden share' });

  const ownerPage = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const guestPage = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const errors = [ownerPage, guestPage].map(watchErrors);
  await ownerPage.goto(boardPath(board, board.viewId));
  await signInOnPage(ownerPage, owner);
  await guestPage.goto('/');
  await signInOnPage(guestPage, guest);
  await expect(boardLink(guestPage, 'Garden share')).toHaveCount(0);

  await ownerPage.getByTestId('share-board').click();
  const dialog = ownerPage.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(1);

  // Not among the workspace's first ten, offered before anything is typed.
  await expect(dialog.getByTestId('share-candidates').locator('li').first()).toBeVisible();
  await expect(dialog.getByRole('button', { name: `Add ${guest.username}` })).toHaveCount(0);
  // Found by name, added as an editor (the board is private).
  await dialog.getByTestId('share-search').fill(guest.username);
  await dialog.getByRole('button', { name: `Add ${guest.username}` }).click();
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(2);
  await expect(dialog.getByTestId('share-search')).toBeFocused();
  await expect
    .poll(async () => (await members(api, board)).find((one) => one.userId === guestId))
    .toMatchObject({ role: 'editor' });
  // The guest's sidebar gets it without a reload.
  await expect(boardLink(guestPage, 'Garden share')).toBeVisible();

  // Another role.
  await dialog.getByLabel(`Role of ${guest.username}`).selectOption('viewer');
  await expect
    .poll(async () => (await members(api, board)).find((one) => one.userId === guestId))
    .toMatchObject({ role: 'viewer' });

  // The reader's own role is another admin's to change.
  await expect(dialog.getByLabel(`Role of ${owner.username}`)).toBeDisabled();
  await expect(dialog.getByRole('button', { name: `Remove ${owner.username}` })).toBeDisabled();

  // Taken off.
  await dialog.getByRole('button', { name: `Remove ${guest.username}` }).click();
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(1);
  await expect
    .poll(async () => (await members(api, board)).map((one) => one.userId))
    .not.toContain(guestId);
  await expect(boardLink(guestPage, 'Garden share')).toHaveCount(0);

  // Open to the workspace, commenters at least.
  await dialog.getByTestId('share-open').click();
  await dialog.getByTestId('share-minimum-role').selectOption('commenter');
  await expect
    .poll(async () => {
      const read = (await (
        await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`)
      ).json()) as Schemas['Container'];
      return [read.visibility, read.minimumRole];
    })
    .toEqual(['open', 'commenter']);

  // The public link: on, made anew (the old token is gone), off.
  await dialog.getByTestId('share-public').click();
  const link = dialog.getByTestId('share-link');
  await expect(link).toHaveValue(/[?]r=/);
  const first = await sharing(api, board);
  expect(first.enabled).toBe(true);
  // The reader's address of the board (`shared.spec.ts`), which names no workspace.
  const address = new URL(await link.inputValue());
  expect(address.pathname).toBe(`/shared/${board.boardId}`);
  expect(address.searchParams.get('r')).toBe(first.token);
  await dialog.getByTestId('share-regenerate').click();
  await ownerPage
    .getByRole('alertdialog')
    .getByRole('button', { name: 'Regenerate token' })
    .click();
  await expect.poll(async () => (await sharing(api, board)).token).not.toBe(first.token);
  const second = await sharing(api, board);
  await expect
    .poll(async () => new URL(await link.inputValue()).searchParams.get('r'))
    .toBe(second.token);
  await dialog.getByTestId('share-public').click();
  await expect(link).toHaveCount(0);
  await expect.poll(async () => (await sharing(api, board)).enabled).toBe(false);

  await expect(ownerPage.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  await guestApi.dispose();
  await ownerPage.context().close();
  await guestPage.context().close();
  expect(errors.flat()).toEqual([]);
});
