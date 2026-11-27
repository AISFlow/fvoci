/**
 * The sidebar's search on the real server: it narrows the list at once, and
 * the server adds what the list does not show: a board with the words in
 * another order, and a hidden one, which can be shown again from there.
 */
import { expect, test } from '@playwright/test';

import { boardMenu, chooseFromMenu, sidebar } from '../e2e/shell.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

test('the search finds boards on the server beyond what the sidebar shows', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('seeker');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const word = `kiwi${Date.now().toString(36)}`;
  const shown = await makeBoard(api, { title: `Orchard ${word} plan` });
  await makeBoard(api, { title: `Hidden ${word} notes` });

  const errors = watchErrors(page);
  await page.goto(boardPath(shown, shown.viewId));
  await signInOnPage(page, owner);
  await chooseFromMenu(page, boardMenu(page, `Hidden ${word} notes`), 'hide');

  const field = sidebar(page).getByTestId('sidebar-search');
  const more = sidebar(page).getByTestId('search-more');

  // The words in another order: the list's own filter finds nothing, the server does.
  const answered = page.waitForResponse((answer) => answer.url().includes('/boards/search?q=plan'));
  await field.fill(`plan ${word.toUpperCase()}`);
  await answered;
  await expect(more.getByRole('link', { name: `Orchard ${word} plan` })).toBeVisible();

  // A hidden board is found, marked, and shown again from there.
  await field.fill(`notes ${word}`);
  const hidden = more.getByRole('link', { name: new RegExp(`Hidden ${word} notes`) });
  await expect(hidden).toContainText('Hidden');
  await more.getByRole('button', { name: `Show Hidden ${word} notes in the sidebar` }).click();
  await field.fill('');
  await expect(
    sidebar(page).getByRole('link', { name: `Hidden ${word} notes`, exact: true }),
  ).toBeVisible();

  // Opened from the results.
  await field.fill(`plan ${word}`);
  await more.getByRole('link', { name: `Orchard ${word} plan` }).click();
  await expect(page).toHaveURL(new RegExp(boardPath(shown)));
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  await api.dispose();
  expect(errors).toEqual([]);
});
