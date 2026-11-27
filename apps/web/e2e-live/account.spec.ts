/**
 * Accounts on the real server: making one, signing in and out. The session
 * is the server's cookie; the page never sees it.
 */
import { expect, test } from '@playwright/test';

import { boardLink } from '../e2e/shell.support.ts';

import {
  ADMIN,
  ORG_NAME,
  boardPath,
  invitation,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  WELCOME_TITLE,
} from './support.ts';

test.use({ locale: 'en-US' });

test('somebody invited makes an account on the page of the invitation, and signs out again', async ({
  page,
  baseURL,
}) => {
  const errors = watchErrors(page);
  const who = person('newcomer');
  const token = await invitation(baseURL ?? '');

  await page.goto(`/invite/${token}`);
  await expect(page.getByTestId('invite-summary')).toHaveText(
    `${ADMIN.username} invited you to ${ORG_NAME}`,
  );
  const form = page.getByTestId('invite-register-form');
  await form.getByLabel(/^user name$/i).fill(who.username);
  await form.getByLabel(/email/i).fill(who.email);
  await form.getByLabel(/^password$/i).fill(who.password);
  await form.getByRole('button', { name: /^create account and join$/i }).click();

  // Signed in at once, in the organization of the invitation; the address is the organization's.
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(page).toHaveURL(/\/w\/[^/]+/);
  // A member starts with a board of their own: the welcome board, which the app opens.
  await expect(boardLink(page, WELCOME_TITLE)).toBeVisible();

  // The session outlives a reload: it is the server's cookie.
  await page.reload();
  await expect(page.getByTestId('app-shell')).toBeVisible();

  await page.getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  await expect(settings.getByTestId('signed-in-as')).toHaveText(`Signed in as ${who.username}`);
  await settings.getByRole('button', { name: 'Sign out' }).click();

  await expect(page.getByTestId('sign-in-form')).toBeVisible();
  // Signed out on the server too: a reload asks again.
  await page.reload();
  await expect(page.getByTestId('sign-in-form')).toBeVisible();

  await signInOnPage(page, who);
  expect(errors).toEqual([]);
});

test('an account that is refused on the page of an invitation says what is wrong beside each field', async ({
  page,
  baseURL,
}) => {
  const errors = watchErrors(page);
  const who = person('careless');
  const taken = person('earlier');
  await register(baseURL ?? '', taken);
  const token = await invitation(baseURL ?? '');

  await page.goto(`/invite/${token}`);
  const form = page.getByTestId('invite-register-form');
  const name = form.getByLabel(/^user name$/i);
  const email = form.getByLabel(/email/i);
  const password = form.getByLabel(/^password$/i);
  const submit = form.getByRole('button', { name: /^create account and join$/i });

  // Three fields that the browser's own check lets through and the server
  // refuses: a blank name, an address that is too long, a short password.
  await name.fill(' ');
  await email.fill(`${'a'.repeat(250)}@example.test`);
  await password.fill('short');
  await submit.click();
  // All three are told at once, each beside its field.
  await expect(form.getByTestId('problem-username')).toHaveText('Fill this in.');
  await expect(form.getByTestId('problem-email')).toHaveText('This is too long.');
  await expect(form.getByTestId('problem-password')).toHaveText('Use at least 8 characters.');
  await expect(form.getByRole('alert')).toHaveCount(3);
  await expect(name).toHaveAttribute('aria-invalid', 'true');
  await expect(name).toHaveAttribute('aria-describedby', 'account-username-problem');
  // The first field that is wrong is where the reader goes on.
  await expect(name).toBeFocused();

  // Two put right: the third is the only one still told.
  await name.fill(who.username);
  await email.fill(who.email);
  await submit.click();
  await expect(form.getByTestId('problem-password')).toHaveText('Use at least 8 characters.');
  await expect(form.getByRole('alert')).toHaveCount(1);
  await expect(name).not.toHaveAttribute('aria-invalid');
  await expect(password).toBeFocused();

  // What the server finds only when it looks things up is told beside its field too.
  await password.fill(who.password);
  await name.fill(taken.username);
  await submit.click();
  await expect(form.getByTestId('problem-username')).toHaveText('That user name is taken.');
  await expect(form.getByRole('alert')).toHaveCount(1);

  // With everything right the account is made, let in and signed in.
  await name.fill(who.username);
  await submit.click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  expect(errors).toEqual([]);
});

test('somebody with an account signs in on the page of an invitation, and joins with it', async ({
  page,
  baseURL,
}) => {
  const who = person('returning');
  await register(baseURL ?? '', who);
  const token = await invitation(baseURL ?? '');

  await page.goto(`/invite/${token}`);
  await page.getByRole('button', { name: 'Have an account? Sign in' }).click();
  const form = page.getByTestId('invite-sign-in-form');
  await form.getByLabel(/user name or email/i).fill(who.username);
  await form.getByLabel(/^password$/i).fill('not the password');
  await form.getByRole('button', { name: /^sign in and join$/i }).click();
  await expect(form.getByRole('alert')).toHaveText('The name or the password is wrong.');
  await form.getByLabel(/^password$/i).fill(who.password);
  await form.getByRole('button', { name: /^sign in and join$/i }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();

  // Signed in, the page of an invitation asks for one press and no more.
  await page.goto(`/invite/${token}`);
  await expect(page.getByText(`You are signed in as ${who.username}.`)).toBeVisible();
  await page.getByTestId('invite-join').click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
});

test('a link that lets nobody in says so, to whoever opens it', async ({ page, baseURL }) => {
  await invitation(baseURL ?? '');
  await page.goto('/invite/no-such-link');
  const told = page.getByTestId('invite-unavailable');
  await expect(told.getByRole('heading')).toHaveText('This invite link cannot be used');
  await expect(page.getByTestId('invite-register-form')).toHaveCount(0);
  await told.getByRole('link', { name: 'Go to the first page' }).click();
  await expect(page.getByTestId('sign-in-form')).toBeVisible();
});

test('the email address signs in as well as the name, and a name with an @ in it is refused', async ({
  page,
  baseURL,
}) => {
  const byEmail = person('mailer');
  await register(baseURL ?? '', byEmail);
  await page.goto('/');
  await signInOnPage(page, { ...byEmail, username: byEmail.email });
  await expect(boardLink(page, WELCOME_TITLE)).toBeVisible();

  // A second person, on an invitation page of their own, cannot choose a user name
  // containing `@` (including another person's email address).
  const token = await invitation(baseURL ?? '');
  const context = await page.context().browser()?.newContext({ locale: 'en-US' });
  if (!context) throw new Error('no browser');
  const other = await context.newPage();
  const odd = person('at');
  await other.goto(`/invite/${token}`);
  const form = other.getByTestId('invite-register-form');
  const name = form.getByLabel(/^user name$/i);
  const refused = other.waitForResponse(
    (res) => res.url().endsWith('/api/v1/invitations/accept') && res.request().method() === 'POST',
  );
  await name.fill(byEmail.email);
  await form.getByLabel(/email/i).fill(odd.email);
  await form.getByLabel(/^password$/i).fill(odd.password);
  await form.getByRole('button', { name: /^create account and join$/i }).click();
  expect((await refused).status()).toBe(400);
  await expect(form.getByTestId('problem-username')).toHaveText(
    'A user name cannot be an email address or include invisible characters, line breaks or characters that change the direction of text.',
  );
  await expect(other.getByTestId('app-shell')).toHaveCount(0);

  await name.fill(odd.username);
  await form.getByRole('button', { name: /^create account and join$/i }).click();
  await expect(other.getByTestId('app-shell')).toBeVisible();
  await context.close();
});

test('a wrong password is told, and nothing else happens', async ({ page, baseURL }) => {
  const who = person('forgetful');
  await register(baseURL ?? '', who);

  await page.goto('/');
  const form = page.getByTestId('sign-in-form');
  await form.getByLabel(/user name or email/i).fill(who.username);
  await form.getByLabel(/^password$/i).fill('not the password');
  await form.getByRole('button', { name: /^sign in$/i }).click();
  await expect(form.getByRole('alert')).toHaveText('The name or the password is wrong.');
  await expect(page.getByTestId('app-shell')).toHaveCount(0);
});

test('the sign-in page offers no account once the server has its organization, and says what to ask for', async ({
  page,
  baseURL,
}) => {
  await invitation(baseURL ?? '');

  await page.goto('/');
  await expect(page.getByTestId('sign-in-form')).toBeVisible();
  await expect(page.getByTestId('invitation-needed')).toHaveText(
    'No account yet? Ask an administrator of your organization for an invite link.',
  );
  await expect(page.getByRole('button', { name: /create an account/i })).toHaveCount(0);
});

test('a session that ends on an open board, and somebody else signs in on that page, shows only theirs', async ({
  page,
  context,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('leaver');
  const next = person('arriver');
  await register(base, owner);
  await register(base, next);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Private plans',
    cards: [{ title: 'Secret step', status: 'To do' }],
  });
  await api.dispose();

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect(page.getByText('Secret step')).toBeVisible();
  // Something to undo, in this session.
  await page.getByRole('button', { name: 'New card in To do' }).click();
  await page.getByTestId('card-title').fill('Another step');
  await page.goBack();
  const undo = page.getByRole('button', { name: 'Undo' });
  await expect(undo).toBeEnabled();

  // The session ends (the cookie is gone); the next request tells, and the
  // page asks to sign in.
  await context.clearCookies();
  await page.getByText('Secret step').click();
  await expect(page.getByTestId('sign-in-form')).toBeVisible();

  // Somebody else signs in on the same page: nothing of the first session stays.
  await signInOnPage(page, next);
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(page.getByText('Secret step')).toHaveCount(0);
  await expect(page.getByTestId('sidebar').getByText('Private plans')).toHaveCount(0);
  // Nor what the first one could undo.
  await expect(undo).toBeDisabled();
  expect(errors).toEqual([]);
});
