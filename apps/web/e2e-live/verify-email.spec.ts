/**
 * A server that confirms the address of a registration (`email_verification`,
 * `tools/live-server.ts --verifying`): registering makes nothing yet and says
 * to check the mail; a sign-in before the link is refused with a way to the
 * link; the link opens the account's first board, signed in; a link that was
 * replaced confirms nothing. Screens of each with `FVOCI_SCREENS=1`.
 */
import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { screensDir } from '../e2e/screens.ts';
import { inbox, memberLink, person, signedIn, watchErrors, type Person } from './support.ts';

// The server holds one organization: who registers first starts it, and the
// account after comes by a link of theirs.
test.describe.configure({ mode: 'serial' });
/** Who started the organization, once the first test has. */
let founder: Person | undefined;

/** The screen as it is, wide and at 390 pixels, under `screens/verify/`. */
async function verifyShot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const folder = screensDir('verify');
  mkdirSync(folder, { recursive: true });
  const wide = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.screenshot({ path: path.join(folder, `${name}-wide.png`), animations: 'disabled' });
  await page.setViewportSize({ width: 390, height: 844 });
  // Nothing wider than the screen: no page that scrolls sideways.
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.screenshot({ path: path.join(folder, `${name}-390.png`), animations: 'disabled' });
  await page.setViewportSize(wide);
}

/** The confirmation links mailed to `email` so far, oldest first. */
async function links(baseURL: string, email: string): Promise<string[]> {
  return (await inbox(baseURL))
    .filter((mail) => mail.recipient === email)
    .flatMap((mail) =>
      mail.decoded
        .split(/\r?\n/)
        .filter((line) => line.startsWith(baseURL) && line.includes('/verify-email#'))
        .map((line) => line.trim()),
    );
}

test('a registration waits for its mailed link, which signs in on the first board', async ({
  page,
  baseURL,
}) => {
  if (!baseURL) throw new Error('missing live server address');
  const who = person('verify');
  const errors = watchErrors(page);
  await page.goto('/');
  const options = await page.request.get('/api/v1/login-options');
  expect((await options.json()) as { emailVerification: boolean }).toMatchObject({
    emailVerification: true,
  });

  await page.getByRole('button', { name: 'New here? Create an account' }).click();
  // The server's first account: it names its organization, with no choice to make.
  await expect(page.getByTestId('register-first')).toBeVisible();
  await expect(page.getByTestId('register-use')).toHaveCount(0);
  await page.getByTestId('register-organization-name').fill('Verified Works');
  await page.getByLabel('User name', { exact: true }).fill(who.username);
  await page.getByLabel('Email address').fill(who.email);
  await page.getByLabel('Password', { exact: true }).fill(who.password);
  await expect(page.getByTestId('verify-hint')).toBeVisible();
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByRole('heading', { name: 'Check your email' })).toBeFocused();
  await expect(page.getByTestId('check-mail')).toContainText(who.email);
  // At a phone's width, nothing is wider than the screen.
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.setViewportSize({ width: 1280, height: 720 });
  await expect.poll(async () => (await links(baseURL, who.email)).length).toBe(1);
  await verifyShot(page, 'check-mail');

  // Before the link: the right password is told to confirm, and mails the link again.
  await page.goto('/');
  await page.getByLabel('User name or email').fill(who.username);
  await page.getByLabel('Password', { exact: true }).fill(who.password);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Confirm your email address first');
  await page.getByRole('button', { name: 'Send the link again' }).click();
  await expect(page.getByTestId('resend-status')).toContainText('a new link is on its way');
  await verifyShot(page, 'sign-in-refused');
  await expect.poll(async () => (await links(baseURL, who.email)).length).toBe(2);
  // A wrong password is told nothing of the registration.
  await page.getByLabel('Password', { exact: true }).fill('not the password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('The name or the password is wrong');

  const [first, second] = await links(baseURL, who.email);
  if (!first || !second) throw new Error('two links were mailed');
  // The first link was replaced by the second: it confirms nothing, and says so.
  await page.goto(first);
  await expect(page.getByTestId('verify-problem')).toContainText('replaced by a newer one');
  await expect(page.getByTestId('verify-request-form')).toBeVisible();
  await verifyShot(page, 'link-refused');

  // The browser that registered: the link is enough there.
  await page.goto(second);
  await expect(page).toHaveURL(/\/w\/[^/]+\/boards\/[^/]+/);
  expect(new URL(page.url()).hash).toBe('');
  await expect(page.getByTestId('sign-in-form')).toHaveCount(0);
  const me = await page.request.get('/api/v1/users/me');
  expect(me.status()).toBe(200);
  expect(((await me.json()) as { username: string }).username).toBe(who.username);
  await verifyShot(page, 'first-board');
  expect(errors).toEqual([]);
  founder = who;
});

test('in another browser the link asks for the password the registration was made with', async ({
  page,
  browser,
  baseURL,
}) => {
  if (!baseURL || !founder) throw new Error('the server has no organization yet');
  const api = await signedIn(baseURL, founder);
  const [workspace] = (await (await api.get('/api/v1/users/me/workspaces')).json()) as {
    id: string;
  }[];
  if (!workspace) throw new Error('the founder has no organization');
  const invitation = await memberLink(api, workspace.id);
  await api.dispose();

  // A new account by the link, its address mistyped: it waits, as a registration does.
  const who = person('elsewhere');
  const mistyped = `x${who.email}`;
  await page.goto(`/invite/${invitation.token ?? ''}`);
  await expect(page.getByTestId('invite-summary')).toBeVisible();
  await page.getByLabel('User name', { exact: true }).fill(who.username);
  await page.getByLabel('Email address').fill(mistyped);
  await page.getByLabel('Password', { exact: true }).fill(who.password);
  await page.getByRole('button', { name: 'Create account and join' }).click();
  await expect(page.getByTestId('check-mail')).toContainText(mistyped);
  await expect.poll(async () => (await links(baseURL, mistyped)).length).toBe(1);
  await verifyShot(page, 'check-mail-invited');

  // Signing up again from this browser replaces it: the same name, at once.
  await page.getByRole('button', { name: 'Sign up again' }).click();
  await expect(page.getByLabel('Email address')).toBeFocused();
  await expect(page.getByLabel('User name', { exact: true })).toHaveValue(who.username);
  await page.getByLabel('Email address').fill(who.email);
  await page.getByLabel('Password', { exact: true }).fill(who.password);
  await page.getByRole('button', { name: 'Create account and join' }).click();
  await expect(page.getByTestId('check-mail')).toContainText(who.email);
  await expect.poll(async () => (await links(baseURL, who.email)).length).toBe(1);
  const [link] = await links(baseURL, who.email);
  const [replaced] = await links(baseURL, mistyped);
  if (!link || !replaced) throw new Error('both links were mailed');
  // The link of the registration that was replaced confirms nothing, even here.
  await page.goto(replaced);
  await expect(page.getByTestId('verify-problem')).toContainText('invalid, has expired');

  // Whoever reads the mail elsewhere: nothing comes of the link alone, in Korean as in English.
  const korean = await browser.newContext({ baseURL, locale: 'ko-KR' });
  const reader = await korean.newPage();
  const readerErrors = watchErrors(reader);
  await reader.goto(link);
  await expect(reader.getByLabel('비밀번호')).toBeFocused();
  await expect(reader.getByTestId('verify-password-intro')).toContainText(
    '가입한 브라우저가 아닙니다',
  );
  // At a phone's width, nothing is wider than the screen.
  await reader.setViewportSize({ width: 390, height: 844 });
  expect(
    await reader.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
  ).toBe(true);
  await reader.setViewportSize({ width: 1280, height: 720 });
  expect((await reader.request.get('/api/v1/users/me')).status()).toBe(401);
  await verifyShot(reader, 'password-asked-ko');
  await reader.getByLabel('비밀번호').fill('not the password');
  await reader.getByRole('button', { name: '확인하고 로그인' }).click();
  await expect(reader.getByRole('alert')).toContainText('가입할 때 정한 비밀번호가 아닙니다');
  expect((await reader.request.get('/api/v1/users/me')).status()).toBe(401);
  await verifyShot(reader, 'password-wrong-ko');
  expect(readerErrors).toEqual([]);
  await korean.close();

  // The link was not used: who registered, in yet another browser, finishes it.
  const other = await browser.newContext({ baseURL, locale: 'en-US' });
  const registrant = await other.newPage();
  await registrant.goto(link);
  await expect(registrant.getByLabel('Password')).toBeFocused();
  await verifyShot(registrant, 'password-asked');
  await registrant.getByLabel('Password').fill(who.password);
  await registrant.getByRole('button', { name: 'Confirm and sign in' }).click();
  await expect(registrant).toHaveURL(/\/w\/[^/]+/);
  expect(new URL(registrant.url()).hash).toBe('');
  const me = await registrant.request.get('/api/v1/users/me');
  // What came back when it is not the account, for the run that fails here.
  expect(me.status()).toBe(200);
  expect(((await me.json()) as { username: string }).username).toBe(who.username);
  await other.close();

  // Used once: the browser that registered is told so, and is not signed in by it.
  await page.goto(link);
  await expect(page.getByTestId('verify-problem')).toContainText('invalid, has expired');
  expect((await page.request.get('/api/v1/users/me')).status()).toBe(401);
});

test('a guest of a wiki who confirms is on that wiki', async ({ page, baseURL }) => {
  if (!baseURL || !founder) throw new Error('the server has no organization yet');
  const api = await signedIn(baseURL, founder);
  const [workspace] = (await (await api.get('/api/v1/users/me/workspaces')).json()) as {
    id: string;
  }[];
  if (!workspace) throw new Error('the founder has no organization');
  const wikis = await api.post(`/api/v1/w/${workspace.id}/wikis`, {
    data: { visibility: 'private', title: 'Handbook' },
  });
  expect(wikis.ok(), await wikis.text()).toBe(true);
  const wiki = (await wikis.json()) as { id: string };
  const made = await api.post(`/api/v1/w/${workspace.id}/invitations`, {
    data: { role: 'guest', containerId: wiki.id, containerRole: 'viewer' },
  });
  expect(made.ok(), await made.text()).toBe(true);
  const invitation = (await made.json()) as { token?: string };
  await api.dispose();

  const who = person('wikiguest');
  const errors = watchErrors(page);
  await page.goto(`/invite/${invitation.token ?? ''}`);
  await expect(page.getByTestId('invite-summary')).toContainText('Handbook');
  await page.getByLabel('User name', { exact: true }).fill(who.username);
  await page.getByLabel('Email address').fill(who.email);
  await page.getByLabel('Password', { exact: true }).fill(who.password);
  await page.getByRole('button', { name: 'Create account and join' }).click();
  await expect(page.getByTestId('check-mail')).toContainText(who.email);
  await expect.poll(async () => (await links(baseURL, who.email)).length).toBe(1);
  const [link] = await links(baseURL, who.email);
  if (!link) throw new Error('the link was mailed');

  await page.goto(link);
  await expect(page).toHaveURL(new RegExp(`/w/${workspace.id}/wikis/${wiki.id}`));
  await expect(page.getByRole('heading', { name: 'Handbook' }).first()).toBeVisible();
  expect(errors).toEqual([]);
});
