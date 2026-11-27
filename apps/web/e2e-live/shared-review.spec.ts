/** Malformed public links must keep the public reader's boundary, including when signed in. */
import { expect, test } from '@playwright/test';

import { boardPath, makeBoard, person, register, signedIn, signInOnPage } from './support.ts';

test.use({ locale: 'en-US' });

test('a public link with an empty token shows not found without signing in', async ({ page }) => {
  await page.goto('/shared/00000000-0000-7000-8000-00000000dead?r=');
  await expect(page.getByTestId('not-found')).toBeVisible();
  await expect(page.getByRole('button', { name: /^sign in$/i })).toHaveCount(0);
});

test('an empty public token does not load the signed-in app and its sidebar', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('shared-empty-review');
  await register(base, owner);
  const api = await signedIn(base, owner);
  try {
    const board = await makeBoard(api, { title: 'Members only plan' });
    await page.goto(boardPath(board, board.viewId));
    await signInOnPage(page, owner);
    await expect(page.getByTestId('board-page')).toBeVisible();

    // A nonempty invalid token is the control: it correctly keeps the reader isolated.
    await page.goto(`/shared/${board.boardId}?r=invalid`);
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(page.getByTestId('sidebar')).toHaveCount(0);

    // Empty is also invalid, and must not bring the account's private sidebar back.
    await page.goto(`/shared/${board.boardId}?r=`);
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(page.getByTestId('sidebar')).toHaveCount(0);
    await expect(page.getByTestId('share-board')).toHaveCount(0);
  } finally {
    await api.dispose();
  }
});
