/**
 * The trash of a wiki's pages, in the world in memory: a page deleted with
 * the pages below it comes back by undo, or from the "Deleted pages" of the
 * wiki's head (how many below it says), and the tree follows a page that
 * somebody else brings back. And a page copied with the pages below it from
 * its menu, right after it, the copy shown, undone.
 */
import { expect, test, type Page } from '@playwright/test';

import { openApp, pressInBar } from './shell.support.ts';
import {
  deleteBySomebodyElse,
  hideTree,
  letSomebodyElseIn,
  nameThePage,
  newWiki,
  pageAction,
  pageTree,
  restoreBySomebodyElse,
  showTree,
  treeShape,
} from './wiki.support.ts';

/** A wiki with "Welcome", "Getting started" below it and "Policies" after it. */
async function wikiWithPages(page: Page): Promise<string> {
  const wikiId = await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  return wikiId;
}

async function deleteWelcome(page: Page): Promise<void> {
  await pageAction(page, 'Welcome', 'delete');
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(['Policies']);
}

test.describe('the trash of a wiki’s pages', () => {
  test('a page deleted comes back with those below it by undo', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await deleteWelcome(page);
    await hideTree(page);
    await pressInBar(page, 'undo');
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
    await hideTree(page);
    await pressInBar(page, 'redo');
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Policies']);
  });

  test('a page deleted is listed with how many below it, and brought back', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await deleteWelcome(page);
    await page.getByTestId('open-page-trash').click();
    const trash = page.getByTestId('page-trash');
    await expect(trash.getByRole('heading')).toHaveText('Deleted pages');
    const item = trash.getByTestId('trash-items').locator('li', { hasText: 'Welcome' });
    await expect(item.getByTestId('trash-below')).toHaveText('Pages below: 1');
    await expect(item).toContainText('Deleted by');
    await expect(trash).toContainText('permanently deleted after 30 days');
    await expect(item.getByTestId('trash-days-left')).toHaveText('30 days left');
    await item.getByRole('button', { name: 'Restore Welcome' }).click();
    await expect(trash.getByTestId('trash-empty')).toBeVisible();
    await trash.getByRole('button', { name: 'Close' }).last().click();
    await expect(trash).toBeHidden();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });

  test('the tree follows a page somebody else brings back', async ({ page }) => {
    await openApp(page);
    const wikiId = await wikiWithPages(page);
    await letSomebodyElseIn(page, wikiId);
    const welcome = await pageTree(page)
      .locator('[data-page][aria-label="Welcome"]')
      .getAttribute('data-page');
    if (!welcome) throw new Error('no page');
    await deleteBySomebodyElse(page, wikiId, welcome);
    await expect.poll(() => treeShape(page)).toEqual(['Policies']);
    await restoreBySomebodyElse(page, wikiId, welcome);
    await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });
});

test.describe('the trash of a wiki’s pages at a phone’s width', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('opened from the drawer of the tree, a page brought back shows in it', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await deleteWelcome(page);
    await page.getByTestId('open-page-trash').click();
    const trash = page.getByTestId('page-trash');
    const box = await trash.boundingBox();
    expect(box && box.x >= 0 && box.x + box.width <= 390).toBe(true);
    await trash.getByRole('button', { name: 'Restore Welcome' }).click();
    await expect(trash.getByTestId('trash-empty')).toBeVisible();
    // The list empty, the focus goes to "Close".
    const close = trash
      .locator('[data-slot="dialog-footer"]')
      .getByRole('button', { name: 'Close' });
    await expect(close).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(trash).toBeHidden();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });
});

test.describe('a page copied at a phone’s width', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('from its menu in the drawer of the tree, the copy shown', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Policies', 'duplicate');
    await expect(page.getByTestId('page-title')).toHaveValue('Policies (copy)');
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Policies', 'Policies (copy)']);
  });
});

test.describe('a page copied', () => {
  test('from its menu with those below it, right after it, shown, and undone', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Welcome', 'duplicate');
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome (copy)');
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Welcome (copy)', '  Getting started', 'Policies']);
    await hideTree(page);
    await pressInBar(page, 'undo');
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });
});
