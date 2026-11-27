/**
 * Changing the password in the settings, on the real server: a wrong
 * current password and a short new one are told, and once it is changed the
 * reader's other sessions are over while this page goes on, also after a
 * reload.
 */
import { expect, test } from '@playwright/test';

import { apiClient, person, register, signedIn, signInOnPage, watchErrors } from './support.ts';

test.use({ locale: 'en-US' });

test('the password is changed in the settings; the other sessions end and this one goes on', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('keyholder');
  await register(base, who);
  const elsewhere = await signedIn(base, who);
  expect((await elsewhere.get('/api/v1/users/me')).status()).toBe(200);

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, who);
  await page.getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  const section = settings.getByTestId('password-change');
  await section.getByRole('button', { name: 'Change password' }).click();
  const current = section.getByLabel('Current password');
  const next = section.getByLabel('New password');
  const save = section.getByRole('button', { name: 'Change password' });

  await current.fill('not the password');
  await next.fill('a new long password');
  await save.click();
  await expect(section.getByRole('alert')).toHaveText('The current password is wrong.');
  await expect(current).toBeFocused();
  await expect(current).toHaveAttribute('aria-invalid', 'true');

  await current.fill(who.password);
  await next.fill('short');
  await save.click();
  await expect(section.getByRole('alert')).toHaveText(
    'The new password needs at least 8 characters.',
  );

  await next.fill('a new long password');
  await save.click();
  await expect(section.getByRole('status')).toHaveText(
    'The password is changed. Every other place you were signed in is signed out.',
  );
  await expect(current).toBeHidden();
  await expect(section.getByRole('button', { name: 'Change password' })).toBeFocused();

  // The other session is over; this page's goes on, after a reload too.
  expect((await elsewhere.get('/api/v1/users/me')).status()).toBe(401);
  await page.reload();
  await expect(page.getByTestId('app-shell')).toBeVisible();

  // Only the new password signs in.
  const fresh = await apiClient(base);
  const login = (password: string) =>
    fresh.post('/api/v1/login', { data: { username: who.username, password } });
  expect((await login(who.password)).status()).toBe(401);
  expect((await login('a new long password')).status()).toBe(204);

  await fresh.dispose();
  await elsewhere.dispose();
  expect(errors).toEqual([]);
});
