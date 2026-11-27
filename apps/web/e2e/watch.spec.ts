/**
 * Watching, and the links of pages, on the backend in memory: a card's head
 * has "Watch" (pressed while the reader is told of it), which stops and
 * starts again and stays as it was left; a page of a wiki has it too, and
 * its menu copies the page's address.
 */
import { expect, test } from '@playwright/test';

import { ON_A_CARD } from './address.ts';
import { clipboardPermissions, engineOf } from './keyboard.support.ts';
import { openApp } from './shell.support.ts';
import { goInside, newWiki, ON_A_PAGE } from './wiki.support.ts';

test('a card is watched by a press at its head, and stays as it was left', async ({ page }) => {
  await openApp(page);
  await page.locator('[data-card]').first().click();
  await expect(page).toHaveURL(ON_A_CARD);
  const toggle = page.getByTestId('card-header').getByTestId('watch-toggle');
  await expect(toggle).toHaveAccessibleName('Watch');
  const was = (await toggle.getAttribute('aria-pressed')) === 'true';
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', String(!was));
  await expect(toggle).toBeEnabled();

  // Away and back: the server's answer, not what the button was left at.
  const address = new URL(page.url()).pathname;
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-header')).toHaveCount(0);
  await goInside(page, address);
  await expect(toggle).toHaveAttribute('aria-pressed', String(!was));
});

test('a page is watched by who wrote it, and its menu copies its address', async ({ page }) => {
  await page.context().grantPermissions(clipboardPermissions(engineOf(page) ?? 'chromium'));
  await openApp(page);
  await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await expect(page).toHaveURL(ON_A_PAGE);
  const toggle = page.getByTestId('watch-toggle');
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');

  await page.getByTestId('page-menu').click();
  await page.locator('[data-action="copy-link"]').click();
  await expect(page.getByTestId('page-link-copied')).toHaveText('Copied!');
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toBe(page.url());
});
