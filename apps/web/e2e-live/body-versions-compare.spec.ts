/**
 * A version of a card's body read as what changed, on the real server: from
 * the version before it, the words added and removed; from the body now, or
 * as it is, by the keys; two versions of the same body have nothing changed.
 * The comparison is made in a worker the page's content security policy lets
 * start.
 */
import { expect, test } from '@playwright/test';

import { boardPath, makeBoard, person, register, signedIn, watchErrors } from './support.ts';
import { openVersions, pageOf, rows, shown, writeWhole } from './versions.support.ts';

test.use({ locale: 'en-US' });

test('a version shows the words it added and removed, from the one before or the body now, or is shown as it is', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('compare');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Versions compared',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  for (const text of ['alpha beta gamma', 'alpha delta gamma', 'alpha beta gamma']) {
    await writeWhole(api, board, cardId, text);
  }
  const page = await pageOf(browser, boardPath(board, board.viewId, cardId), owner);
  const errors = watchErrors(page);
  const workers: string[] = [];
  page.on('worker', (worker) => workers.push(worker.url()));
  const diff = shown(page).getByTestId('body-diff');
  const choice = shown(page).getByTestId('body-version-compare');
  const whole = shown(page).getByTestId('body-version-whole');

  await openVersions(page);
  await expect(rows(page)).toHaveCount(3);
  // The one in the middle, from the one before it: a word removed, a word added.
  await rows(page).nth(1).click();
  await expect(shown(page).getByTestId('body-version-direction')).toContainText(
    'from the previous version',
  );
  await expect(diff.locator('del')).toHaveText(['beta']);
  await expect(diff.locator('ins')).toHaveText(['delta']);
  await expect(diff.getByTestId('body-diff-counts')).toHaveText('1 added · 1 removed');
  expect(workers.some((url) => url.includes('diff-worker'))).toBe(true);

  // By the keys: from the body now, then as it is, then compared again.
  await choice.focus();
  await page.keyboard.press('ArrowDown');
  await expect(choice).toHaveValue('current');
  await expect(diff.locator('del')).toHaveText(['beta']);
  await page.keyboard.press('Tab');
  await expect(whole).toBeFocused();
  await page.keyboard.press('Space');
  await expect(whole).toHaveAttribute('aria-pressed', 'true');
  await expect(diff).toHaveCount(0);
  await expect(shown(page)).toContainText('alpha delta gamma');
  await page.keyboard.press('Space');
  await expect(diff.locator('ins')).toHaveText(['delta']);

  // The newest is the body now, and the oldest the same body: nothing has changed from either.
  await rows(page).first().click();
  await expect(diff.getByTestId('body-diff-none')).toHaveText('Nothing has changed.');
  await choice.selectOption((await rows(page).nth(2).getAttribute('data-version')) ?? '');
  await expect(shown(page).getByTestId('body-version-direction')).toContainText(
    'from an older version',
  );
  await expect(diff.getByTestId('body-diff-none')).toBeVisible();

  await page.context().close();
  await api.dispose();
  expect(errors).toEqual([]);
});
