/**
 * The app on its in-memory backend, end to end: the address bar and the
 * screens stay in step. The parts of the page have tests of their own.
 */
import { expect, test, type Page } from '@playwright/test';

import {
  addressOf,
  boardPath,
  ON_A_CARD,
  ON_A_VIEW,
  placeOf,
  SETTLED,
  WORKSPACE,
  type Place,
} from './address.ts';

test('the root shows the first board of the sidebar, and its first view', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(ON_A_VIEW);
  await expect(page).toHaveURL(SETTLED);
});

test('the root is put out of the history: back leaves the app', async ({ page }) => {
  await page.goto('about:blank');
  await page.goto('/');
  await expect(page).toHaveURL(SETTLED);
  await page.goBack();
  expect(page.url()).toBe('about:blank');
});

test('a workspace alone shows its first board, in its place in the history', async ({ page }) => {
  await page.goto('about:blank');
  await page.goto(addressOf({ workspaceId: WORKSPACE }));
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(SETTLED);
  await page.goBack();
  expect(page.url()).toBe('about:blank');
});

/** The board and view the root settles on. */
async function firstPlace(page: Page): Promise<Place> {
  await page.goto('/');
  await expect(page).toHaveURL(ON_A_VIEW);
  return placeOf(page.url());
}

const NOT_THERE = '01930000-0000-7000-8000-00000000ffff';

test('an address that does not fit says the same, whatever does not fit, with the way home', async ({
  page,
}) => {
  const { boardId, viewId } = await firstPlace(page);
  const notFound = page.getByTestId('not-found');
  const seen: string[] = [];
  for (const address of [
    // A workspace that is not there, with a board that is.
    addressOf({ workspaceId: NOT_THERE, boardId, viewId }),
    addressOf({ workspaceId: NOT_THERE }),
    // A board that is not there, in the workspace that is.
    boardPath(NOT_THERE),
    // The old shape of the addresses is gone.
    ['', 'board', boardId, viewId].join('/'),
  ]) {
    await page.goto(address);
    await expect(notFound).toBeVisible();
    await expect(page.getByTestId('board-page')).toHaveCount(0);
    seen.push(await notFound.innerText());
  }
  expect(new Set(seen).size).toBe(1);
  expect(seen[0]).toContain('There is nothing at this address.');

  await notFound.getByRole('link', { name: 'Back to home' }).click();
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(SETTLED);
});

test('a board of another workspace is not found at this workspace’s address', async ({ page }) => {
  const { boardId, viewId } = await firstPlace(page);
  const notFound = page.getByTestId('not-found');
  const elsewhere = await page.evaluate(
    async ([id, other]) => {
      const dev = (
        window as unknown as {
          fvociDev: {
            backend: { world: { containers: Map<string, { workspaceId: string }> } };
            workspace: { containers: { id: string; title: string }[] };
          };
        }
      ).fvociDev;
      // Another board, moved to another workspace the reader can still read it in.
      const board = dev.workspace.containers.find((one) => one.id !== id);
      const stored = board && dev.backend.world.containers.get(board.id);
      if (!board || !stored) throw new Error('the seed has one board');
      stored.workspaceId = other;
      return board.id;
    },
    [boardId, NOT_THERE] as const,
  );
  const go = (to: string): Promise<void> =>
    page.evaluate((address) => {
      history.pushState(null, '', address);
      window.dispatchEvent(new PopStateEvent('popstate'));
    }, to);
  await go(boardPath(elsewhere));
  await expect(notFound).toBeVisible();
  await expect(page.getByTestId('board-page')).toHaveCount(0);
  // Back to a board of this workspace: shown again.
  await page.goBack();
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${boardPath(boardId, viewId)}$`));
});

test('a card is an address: opening adds a step, back closes it, forward opens it again', async ({
  page,
}) => {
  await page.goto('/');
  const card = page.locator('[data-card]').first();
  await expect(card).toBeVisible();
  const board = page.url();
  await card.click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page).toHaveURL(ON_A_CARD);
  const opened = page.url();

  await page.goBack();
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  expect(page.url()).toBe(board);

  await page.goForward();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  expect(page.url()).toBe(opened);

  // Loaded on its own address, the card is open over its board.
  await page.goto(opened);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('board-page')).toBeVisible();
});
