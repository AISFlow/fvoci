/**
 * The window is named after what it shows before what holds it, the way a
 * browser's tabs and history list them, and "fvoci" alone once it shows
 * nothing named. A course's unit is named in `course.spec.ts`.
 */
import { expect, test } from '@playwright/test';

import { card } from './kanban.support.ts';
import { FIRST_BOARD, openApp } from './shell.support.ts';
import { goInside, nameThePage, newWiki } from './wiki.support.ts';

test('the window is named by the card, the board, the page and the wiki shown', async ({
  page,
}) => {
  await openApp(page);
  await expect(page).toHaveTitle(`${FIRST_BOARD} — fvoci`);
  await card(page, 'Design the screens').click();
  await expect(page).toHaveTitle(`Design the screens · ${FIRST_BOARD} — fvoci`);
  await page.goBack();
  await expect(page).toHaveTitle(`${FIRST_BOARD} — fvoci`);

  await newWiki(page, 'Handbook');
  await expect(page).toHaveTitle('Handbook — fvoci');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await expect(page).toHaveTitle('Welcome · Handbook — fvoci');

  await goInside(page, '/today');
  await expect(page.getByTestId('today-hub-page')).toBeVisible();
  await expect(page).toHaveTitle('Today — fvoci');
  await page.goBack();
  await expect(page).toHaveTitle('Welcome · Handbook — fvoci');
});
