/**
 * The language of the interface as an account setting on the real server:
 * kept by the account, so that another device signs in to the same language;
 * `auto` following the browser; chosen in the settings and saved there.
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import { person, register, signedIn, signInOnPage, watchErrors, type Schemas } from './support.ts';

async function language(api: APIRequestContext): Promise<string> {
  return ((await (await api.get('/api/v1/users/me/settings')).json()) as Schemas['UserSettings'])
    .language;
}

const languageSetting = (page: Page) =>
  page.getByTestId('settings-dialog').locator('[data-setting="language"]');

test('the account keeps the language: another device signs in to it, and auto follows the browser', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('polyglot');
  await register(base, who);
  const api = await signedIn(base, who);
  expect(await language(api)).toBe('auto');
  // Chosen on another device.
  await api.patch('/api/v1/users/me/settings', { data: { language: 'ko' } });

  // A new device, whose browser asks for English: signed out, it is in English.
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto('/');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await signInOnPage(page, who);
  // Signed in, it follows the account: the page loads again in Korean.
  await expect(page.locator('html')).toHaveAttribute('lang', 'ko');
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await page.getByTestId('open-settings').click();
  await expect(languageSetting(page).getByRole('button')).toContainText('한국어');

  // Back to automatic in the settings: saved, and the page is the browser's English again.
  await languageSetting(page).getByRole('button').click();
  await page.getByRole('option', { name: /^자동 \(English\)/ }).click();
  await expect.poll(() => language(api)).toBe('auto');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.getByTestId('app-shell')).toBeVisible();

  // Another language chosen here is the account's, and stays over a reload.
  await page.getByTestId('open-settings').click();
  await languageSetting(page).getByRole('button').click();
  await page.getByRole('combobox', { name: 'Search languages' }).fill('ko');
  await page.getByRole('option', { name: '한국어' }).click();
  await expect.poll(() => language(api)).toBe('ko');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ko');
  // The page that loaded again for it is up, and reads nothing any more, before it is
  // loaded once more: what a load cuts off on its way (the test's own doing here) is
  // told as errors by Safari's engine.
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await page.waitForLoadState('networkidle');
  await page.reload();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'ko');

  await api.dispose();
  await context.close();
  expect(errors).toEqual([]);
});
