/**
 * A guest on the real server: a board's admin makes a link for one in the
 * share dialog, and whoever accepts it opens that board in the app and edits
 * it, with nothing else of the workspace in sight. The link lets one person in.
 */
import { expect, test } from '@playwright/test';

import { boardLink, sidebar, unfold } from '../e2e/shell.support.ts';

import {
  ORG_NAME,
  boardPath,
  makeBoard,
  person,
  register,
  shot,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

test('a guest comes in by a link from the share dialog, and opens and edits that board alone', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('host');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Guest invitation',
    cards: [{ title: 'Visible to the invited guest', status: 'To do' }],
  });
  const other = await makeBoard(api, { title: 'Not for guests', visibility: 'open' });

  // The board's admin makes the link where the board is shared.
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('share-board').click();
  const share = page.getByTestId('share-dialog');
  await unfold(share.getByTestId('share-guest-toggle'));
  await share.getByTestId('share-guest-make').click();
  const field = share.getByTestId('share-guest-link');
  await expect(field).toHaveValue(/\/invite\/[\w-]{43}$/);
  const link = await field.inputValue();
  await shot(page, 'share-guest-link');

  // Whoever has the link, in a browser of their own.
  const context = await browser.newContext({ locale: 'en-US' });
  const guestPage = await context.newPage();
  const guestErrors = watchErrors(guestPage);
  await guestPage.goto(link);
  await expect(guestPage.getByTestId('invite-summary')).toHaveText(
    `${owner.username} invited you to the board “Guest invitation” of ${ORG_NAME}`,
  );
  await shot(guestPage, 'invite-guest');
  const guest = person('guest');
  const form = guestPage.getByTestId('invite-register-form');
  await form.getByLabel(/^user name$/i).fill(guest.username);
  await form.getByLabel(/email/i).fill(guest.email);
  await form.getByLabel(/^password$/i).fill(guest.password);
  await form.getByRole('button', { name: 'Create account and join' }).click();

  // On the board at once, and it is theirs to edit.
  await expect(guestPage.getByTestId('board-page')).toBeVisible();
  await expect(guestPage).toHaveURL(new RegExp(`/boards/${board.boardId}`));
  await guestPage.getByText('Visible to the invited guest').click();
  await guestPage.getByTestId('card-title').fill('Edited by the guest');
  await guestPage.goBack();
  await expect(page.getByText('Edited by the guest')).toBeVisible();

  // Nothing else of the workspace: that board in the sidebar, and no way to make one.
  await expect(boardLink(guestPage, 'Guest invitation')).toBeVisible();
  await expect(sidebar(guestPage).getByText('Not for guests')).toHaveCount(0);
  for (const control of [
    'add-board',
    'strip-add-board',
    'open-trash',
    'new-category',
    'open-people',
    'share-board',
  ]) {
    await expect(guestPage.getByTestId(control)).toHaveCount(0);
  }
  // What the board's menu in the sidebar has for a guest: saving it, and no more.
  await sidebar(guestPage)
    .getByRole('button', { name: 'Menu of the board Guest invitation' })
    .click();
  await expect(guestPage.getByRole('menuitem')).toHaveText(['Export the board']);
  await guestPage.keyboard.press('Escape');
  // The sidebar is not a guest's to arrange: its categories stay as they are.
  await expect(sidebar(guestPage).locator('[data-category-toggle]').first()).toBeDisabled();
  // The settings bring no boards in.
  await guestPage.getByTestId('open-settings').click();
  await expect(guestPage.getByTestId('export-workspace')).toBeVisible();
  await expect(guestPage.getByTestId('import-archive')).toHaveCount(0);
  await guestPage.keyboard.press('Escape');
  // Nothing the guest's page asked for was refused.
  await expect(guestPage.locator('[data-problem]')).toHaveCount(0);
  await shot(guestPage, 'guest-in-the-app');
  // And by its address neither: the other board is not there for a guest.
  const guestApi = await signedIn(base, guest);
  expect(
    (await guestApi.get(`/api/v1/w/${other.workspaceId}/containers/${other.boardId}`)).status(),
  ).toBe(404);
  await guestPage.reload();
  await expect(guestPage.getByTestId('board-page')).toBeVisible();

  // The link was for one: the next who opens it finds it of no use.
  const late = await browser.newContext({ locale: 'en-US' });
  const latePage = await late.newPage();
  await latePage.goto(link);
  await expect(latePage.getByTestId('invite-unavailable')).toBeVisible();
  await expect(latePage.getByTestId('invite-register-form')).toHaveCount(0);
  await shot(latePage, 'invite-unavailable');

  await Promise.all([api.dispose(), guestApi.dispose(), context.close(), late.close()]);
  expect(errors).toEqual([]);
  expect(guestErrors).toEqual([]);
});
