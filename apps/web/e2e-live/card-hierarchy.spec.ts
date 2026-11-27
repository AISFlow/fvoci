/**
 * Card hierarchy end-to-end against the real server:
 * - Creates a sub-card inline & attaches an existing completed card under a parent
 * - Reloads the browser and confirms hierarchy, breadcrumb, progress, and Kanban badges persist
 * - Deletes the parent card and restores it from board trash, confirming sub-cards cascade both ways
 */
import { expect, test } from '@playwright/test';

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

test('card hierarchy persists across reload and cascades delete/restore on the real server', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('hier');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Hierarchy Live',
    cards: [
      { title: 'Parent Goal', status: 'To do' },
      { title: 'Existing Done', status: 'Done' },
    ],
  });
  const parentId = board.cards['Parent Goal'] ?? '';
  const existingId = board.cards['Existing Done'] ?? '';

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, parentId));
  await signInOnPage(page, owner);
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();

  // Create a new sub-card inline and link the existing Done card.
  const input = dialog.getByTestId('subcard-new-input');
  await input.fill('Live Child');
  await input.press('Enter');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Live Child');

  await dialog.getByTestId('subcard-link-trigger').click();
  await page.getByRole('option', { name: /Existing Done/ }).click();
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Reload and verify server persistence.
  await page.reload();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('subcards-list')).toContainText('Live Child');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Existing Done');
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Navigate to sub-card and verify parent breadcrumb persists.
  await dialog.locator(`[data-subcard-open="${existingId}"]`).click();
  await expect(dialog.getByTestId('card-parent-breadcrumb')).toContainText('Parent Goal');
  await dialog.locator(`[data-parent-crumb="${parentId}"]`).click();
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Delete parent card -> parent + sub-cards leave the board.
  await dialog.getByTestId('card-menu').click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(`[data-card="${parentId}"]`)).toHaveCount(0);
  await expect(page.getByText('Live Child')).toHaveCount(0);
  await expect(page.locator(`[data-card="${existingId}"]`)).toHaveCount(0);

  // Restore parent from board trash -> parent and both sub-cards return with hierarchy intact.
  await page.getByTestId('open-card-trash').click();
  const trash = page.getByTestId('card-trash');
  const row = trash.locator('[data-trash-item="Parent Goal"]');
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Restore Parent Goal' }).click();
  await expect(row).toHaveCount(0);
  await page.keyboard.press('Escape');

  await expect(page.locator(`[data-card="${parentId}"]`)).toBeVisible();
  await expect(page.locator(`[data-card="${parentId}"] [data-slot="card-subcards"]`)).toContainText(
    '1/2',
  );
  await expect(page.locator(`[data-card="${existingId}"]`)).toBeVisible();

  // Open restored parent and verify both sub-cards came back intact.
  await page.locator(`[data-card="${parentId}"]`).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('subcards-list')).toContainText('Live Child');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Existing Done');
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Reload once more to verify restored hierarchy persisted on the server.
  await page.reload();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('subcards-list')).toContainText('Live Child');
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  await api.dispose();
  expect(errors).toEqual([]);
});
