/**
 * Deleting an account, on the real server, from the settings to what is left
 * of it: the person asks, with their name and password, and is signed out
 * everywhere; signing in shows that the account is to be deleted and opens
 * nothing but taking that back; taken back, the account is as it was. Asked
 * again, the person who runs the server erases it at once on the
 * administration socket, and somebody who reads what the person wrote sees
 * it written by a deleted user. Somebody who forgot the password meanwhile
 * resets it by mail, and signs in with the new one to the same way back.
 */
import { expect, test, type Page } from '@playwright/test';

import {
  apiClient,
  boardPath,
  inbox,
  makeBoard,
  operator,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
} from './support.ts';

test.use({ locale: 'en-US' });

/** A body of one line: the text, and a mention of the user after it. */
const body = (text: string, mentioned: string) => ({
  type: 'doc',
  content: [
    {
      type: 'paragraph',
      content: [
        { type: 'text', text: `${text} ` },
        { type: 'mention', attrs: { userId: mentioned } },
      ],
    },
  ],
});

/** Asks for the deletion in the settings, as `who`; the page is signed out by it. */
async function askOnPage(page: Page, who: Person): Promise<void> {
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('account-deletion');
  await section.getByRole('button', { name: 'Delete account' }).click();
  await expect(section.getByTestId('account-deletion-text')).toContainText(
    'Your account is deleted 30 days after you ask.',
  );
  await section.getByLabel(/^Type your user name/).fill(who.username);
  await section.getByLabel('Current password').fill(who.password);
  await section.getByRole('button', { name: 'Delete my account' }).click();
  await expect(page.getByTestId('sign-in-form')).toBeVisible();
  // The sign-out is not unexplained.
  await expect(page.getByTestId('account-leaving-asked')).toContainText(
    'Your account will be deleted in 30 days',
  );
}

/** Signs in on the sign-in page to an account that waits: the page shows that, and no board. */
async function signInToWaiting(page: Page, who: Person): Promise<void> {
  const form = page.getByTestId('sign-in-form');
  await form.getByLabel(/user name or email/i).fill(who.username);
  await form.getByLabel(/^password$/i).fill(who.password);
  await form.getByRole('button', { name: /^sign in$/i }).click();
  await expect(page.getByTestId('account-leaving')).toBeVisible();
  await expect(page.getByTestId('app-shell')).toHaveCount(0);
}

test('an account is asked to be deleted, taken back, asked again and erased; what it wrote is a deleted user’s', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const leaver = person('leaver');
  const reader = person('reader');
  // Somebody who writes nothing: mentioned by the leaver, and erased too.
  const quiet = person('quiet');
  for (const who of [leaver, reader, quiet]) await register(base, who);
  const asQuiet = await signedIn(base, quiet);
  const quietId = ((await (await asQuiet.get('/api/v1/users/me')).json()) as { id: string }).id;
  await asQuiet.dispose();
  const api = await signedIn(base, leaver);
  const board = await makeBoard(api, {
    title: 'Left behind',
    cards: [{ title: 'Written before leaving', status: 'To do' }],
    members: [{ who: reader, role: 'editor' }],
  });
  const cardId = board.cards['Written before leaving'] ?? '';
  const posted = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`,
    {
      data: { body: body('Words that stay', quietId) },
    },
  );
  expect(posted.ok(), await posted.text()).toBe(true);

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, leaver);

  // A wrong password is told at its field, and nothing is asked.
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('account-deletion');
  await section.getByRole('button', { name: 'Delete account' }).click();
  await section.getByLabel(/^Type your user name/).fill(leaver.username);
  await section.getByLabel('Current password').fill('not the password');
  await section.getByRole('button', { name: 'Delete my account' }).click();
  await expect(section.getByRole('alert')).toHaveText('The current password is wrong.');
  expect((await api.get('/api/v1/users/me')).status()).toBe(200);
  await page.reload();
  await expect(page.getByTestId('app-shell')).toBeVisible();

  // Asked: every session of the account is over, this page's too.
  await askOnPage(page, leaver);
  expect((await api.get('/api/v1/users/me')).status()).toBe(401);

  // Signing in shows when the account is deleted, and opens the way back only.
  await signInToWaiting(page, leaver);
  await expect(page.getByTestId('account-leaving-text')).toContainText(
    /It is deleted in 30 days, on \w+ \d+, \d{4}\./,
  );
  const waiting = await signedIn(base, leaver);
  const refused = await waiting.get('/api/v1/users/me');
  expect(refused.status()).toBe(401);
  expect(((await refused.json()) as { reason?: string }).reason).toBe('account_deletion_pending');
  // After a reload too: the page knows the session for what it is.
  await page.reload();
  await expect(page.getByTestId('account-leaving')).toBeVisible();

  // Taken back, the account is as it was.
  await page.getByRole('button', { name: 'Cancel the deletion' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  expect((await waiting.get('/api/v1/users/me')).status()).toBe(200);
  await page.goto(boardPath(board, board.viewId, cardId));
  await expect(page.getByText('Words that stay')).toBeVisible();

  // Asked again, and erased at once by the person who runs the server.
  await page.goto('/');
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await askOnPage(page, leaver);
  expect(await operator(base, `users/${leaver.username}/erase`)).toBe(204);
  const fresh = await apiClient(base);
  const login = await fresh.post('/api/v1/login', {
    data: { username: leaver.username, password: leaver.password },
  });
  expect(login.status()).toBe(401);
  expect(await operator(base, `users/${leaver.username}/erase`)).toBe(404);
  // Somebody who did not ask goes on the operator's word too.
  expect(await operator(base, `users/${quiet.username}/erase`)).toBe(204);

  // Somebody who reads what they wrote sees a deleted user, and no name.
  const other = await browser.newContext({ locale: 'en-US' });
  const reading = await other.newPage();
  const readingErrors = watchErrors(reading);
  await reading.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(reading, reader);
  await expect(reading.getByText('Words that stay')).toBeVisible();
  const comments = reading.getByTestId('card-comments');
  await expect(comments.getByText('Deleted user', { exact: true })).toBeVisible();
  await expect(comments).not.toContainText(leaver.username);
  // Whom the words mention wrote nothing, and is a deleted user all the same.
  await expect(comments.locator('[data-mention]')).toContainText('Deleted user');
  await expect(comments).not.toContainText(quiet.username);
  await expect(comments).not.toContainText('Unknown user');
  // Among the people who can be added to the board, they are not.
  const people = await signedIn(base, reader);
  const found = await people.get(`/api/v1/w/${board.workspaceId}/users`, {
    params: { search: leaver.username },
  });
  expect(await found.json()).toEqual([]);

  expect(errors).toEqual([]);
  expect(readingErrors).toEqual([]);
  await other.close();
  for (const client of [api, waiting, fresh, people]) await client.dispose();
});

test('an account that waits resets a forgotten password by mail and signs in to take the deletion back', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const forgetful = person('forgetful');
  await register(base, forgetful);
  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, forgetful);
  await askOnPage(page, forgetful);

  // The password is forgotten: a reset by mail, from the sign-in page.
  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await page.getByLabel('Email address').fill(forgetful.email);
  await page.getByRole('button', { name: 'Send reset link' }).click();
  await expect(page.getByRole('status')).toContainText('If this email belongs to an account');
  await expect
    .poll(async () => (await inbox(base)).filter((mail) => mail.recipient === forgetful.email))
    .toHaveLength(1);
  const mail = (await inbox(base)).find((mail) => mail.recipient === forgetful.email);
  const link = mail?.decoded
    .split(/\r?\n/)
    .find((line) => line.startsWith(base) && line.includes('/reset-password#'));
  if (!link) throw new Error('the reset message has no link');
  await page.goto(link.trim());
  const renewed = { ...forgetful, password: 'a password found again' };
  await page.getByLabel('New password').fill(renewed.password);
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Your password has been reset');

  // Signed in with it, the account waits still: the way back, and nothing else.
  await page.getByRole('link', { name: 'Back to sign in' }).click();
  await signInToWaiting(page, renewed);
  await expect(page.getByTestId('account-leaving-text')).toContainText(
    /It is deleted in 30 days, on \w+ \d+, \d{4}\./,
  );
  await page.getByRole('button', { name: 'Cancel the deletion' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  const back = await signedIn(base, renewed);
  expect((await back.get('/api/v1/users/me')).status()).toBe(200);
  await back.dispose();
  expect(errors).toEqual([]);
});
