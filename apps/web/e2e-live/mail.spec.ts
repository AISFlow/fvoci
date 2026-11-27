import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { screensDir } from '../e2e/screens.ts';
import { unfold } from '../e2e/shell.support.ts';
import {
  ADMIN,
  boardPath,
  makeBoard,
  apiClient,
  inbox,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

export async function mailShot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const folder = screensDir('mail');
  mkdirSync(folder, { recursive: true });
  const wide = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.screenshot({ path: path.join(folder, `${name}-wide.png`), animations: 'disabled' });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect
    .poll(() =>
      page.evaluate(() =>
        [
          ...document.querySelectorAll(
            '[data-testid="share-dialog"], [data-testid="people-dialog"], [data-testid="invite-email-form"]',
          ),
        ].every((dialog) => {
          const box = dialog.getBoundingClientRect();
          return box.left >= 0 && box.right <= window.innerWidth;
        }),
      ),
    )
    .toBe(true);
  await page.screenshot({ path: path.join(folder, `${name}-390.png`), animations: 'disabled' });
  await page.setViewportSize(wide);
}

test('recovers by SMTP link, signs in with the new password and ends old sessions', async ({
  page,
  baseURL,
}) => {
  if (!baseURL) throw new Error('missing live server address');
  const origin = baseURL;
  const who = person('mail-reset');
  await register(origin, who);
  const oldSession = await signedIn(origin, who);
  const errors = watchErrors(page);
  await page.goto('/');
  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await expect(page.getByTestId('request-password-form')).toBeVisible();
  await mailShot(page, 'request');
  await page.getByLabel('Email address').fill(who.email);
  await page.getByRole('button', { name: 'Send reset link' }).click();
  await expect(page.getByRole('status')).toContainText('If this email belongs to an account');
  await expect
    .poll(async () => (await inbox(origin)).filter((mail) => mail.recipient === who.email).length)
    .toBe(1);
  const mail = (await inbox(origin)).find((mail) => mail.recipient === who.email);
  if (!mail) throw new Error('reset message was not delivered');
  const link = mail.decoded
    .split(/\r?\n/)
    .find((line) => line.startsWith(origin) && line.includes('/reset-password#'));
  if (!link) throw new Error('reset message has no link');
  expect(new URL(link.trim()).hash).toHaveLength(44);
  await page.goto(link.trim());
  await expect(page.getByTestId('reset-password-form')).toBeVisible();
  await mailShot(page, 'new-password');
  await page.getByLabel('New password').fill('New password from SMTP');
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Your password has been reset');
  expect(new URL(page.url()).hash).toBe('');
  expect((await oldSession.get('/api/v1/users/me')).status()).toBe(401);
  const anon = await apiClient(origin);
  expect((await anon.post('/api/v1/login', { data: who })).status()).toBe(401);
  await page.getByRole('link', { name: 'Back to sign in' }).click();
  await signInOnPage(page, { ...who, password: 'New password from SMTP' });
  expect(errors).toEqual([]);
  await oldSession.dispose();
  await anon.dispose();
});

test('emails an existing invitation to multiple recipients from people management', async ({
  page,
  baseURL,
}) => {
  if (!baseURL) throw new Error('missing live server address');
  await page.goto('/');
  await signInOnPage(page, ADMIN);
  await page.getByTestId('open-people').click();
  const dialog = page.getByTestId('people-dialog');
  await dialog.getByTestId('people-make-link').click();
  const link = await dialog.getByTestId('people-link').inputValue();
  const first = person('mail-invite-one').email;
  const second = person('mail-invite-two').email;
  await dialog.getByLabel('Email invitation to').fill(`${first},\n${second}`);
  await mailShot(page, 'member-invitation');
  await dialog.getByRole('button', { name: 'Send invitation email' }).click();
  await expect(dialog.getByTestId('invite-email-form').getByRole('status')).toContainText(
    'Invitation mail queued',
  );
  await expect
    .poll(
      async () =>
        (await inbox(baseURL)).filter(
          (mail) => mail.recipient === first || mail.recipient === second,
        ).length,
    )
    .toBe(2);
  for (const mail of (await inbox(baseURL)).filter(
    (mail) => mail.recipient === first || mail.recipient === second,
  )) {
    expect(mail.decoded).toContain(link);
  }
  const anonymous = await page.context().browser()?.newContext();
  if (!anonymous) throw new Error('browser context unavailable');
  const invited = await anonymous.newPage();
  await invited.goto(link);
  await expect(invited.getByTestId('invite-page')).toBeVisible();
  await anonymous.close();
});

test('emails a guest invitation from the share dialog', async ({ page, baseURL }) => {
  if (!baseURL) throw new Error('missing live server address');
  const owner = person('mail-host');
  await register(baseURL, owner);
  const api = await signedIn(baseURL, owner);
  const board = await makeBoard(api, { title: 'Mail a guest invitation' });
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('share-board').click();
  const share = page.getByTestId('share-dialog');
  await unfold(share.getByTestId('share-guest-toggle'));
  await share.getByTestId('share-guest-make').click();
  const link = await share.getByTestId('share-guest-link').inputValue();
  const recipient = person('mail-guest').email;
  await share.getByLabel('Email invitation to').fill(recipient);
  await expect(share.getByLabel('Email invitation to')).toHaveAccessibleDescription(
    /their own one-use link/,
  );
  await mailShot(page, 'guest-invitation');
  await share.getByRole('button', { name: 'Send invitation email' }).click();
  await expect(share.getByRole('status')).toContainText('Invitation mail queued');
  const received = async () =>
    (await inbox(baseURL)).find((mail) => mail.recipient === recipient)?.decoded ?? '';
  await expect.poll(received).toContain('/invite/');
  // A guest link lets one person in once: the recipient has one of their own, and
  // the link that was made stays its maker's.
  expect(await received()).not.toContain(link);
  await api.dispose();
});
