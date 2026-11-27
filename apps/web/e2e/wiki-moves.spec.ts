/**
 * Moving a page of a wiki with the pages below it: with Alt (⌥) and the
 * arrows, from the page's menu and its "Move…", dragged with a mouse and
 * with a finger, and undone. In the world in memory, which keeps the
 * server's rules (nowhere below itself).
 */
import { expect, test, type Page } from '@playwright/test';

import { Finger, nextFrame, pointOn, touch } from './kanban.support.ts';
import { primaryKey } from './keyboard.support.ts';
import { openApp } from './shell.support.ts';
import {
  hideTree,
  nameThePage,
  newWiki,
  pageAction,
  pageTree,
  showTree,
  treeItem,
  treeShape,
} from './wiki.support.ts';

/** A wiki with "Welcome" ("Getting started" below it), "Policies" and "Travel". */
async function wikiWithPages(page: Page): Promise<string> {
  const wikiId = await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  await pageAction(page, 'Policies', 'add-after');
  await nameThePage(page, 'Travel');
  await showTree(page);
  await expect
    .poll(() => treeShape(page))
    .toEqual(['Welcome', '  Getting started', 'Policies', 'Travel']);
  return wikiId;
}

/** The row of a page of the tree, by its name. */
const row = (page: Page, name: string) =>
  pageTree(page).locator(`[data-page][aria-label="${name}"]`);

test.describe('a page moved', () => {
  test('with Alt and the arrows, the focus staying on it, and undone', async ({ page }, info) => {
    test.skip(touch(info), 'a keyboard');
    await openApp(page);
    await wikiWithPages(page);
    await treeItem(page, 'Travel').focus();
    await page.keyboard.press('Alt+ArrowUp');
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
    await expect(treeItem(page, 'Travel')).toBeFocused();
    // Into the page above, last among its pages, which show.
    await page.keyboard.press('Alt+ArrowRight');
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', '  Travel', 'Policies']);
    await expect(treeItem(page, 'Travel')).toBeFocused();
    await page.keyboard.press('Alt+ArrowUp');
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Travel', '  Getting started', 'Policies']);
    // Out to its parent's level, right after it.
    await page.keyboard.press('Alt+ArrowLeft');
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
    // Where it goes nowhere, nothing changes: the first page up.
    await treeItem(page, 'Welcome').focus();
    await page.keyboard.press('Alt+ArrowUp');
    await page.keyboard.press('Alt+ArrowLeft');
    expect(await treeShape(page)).toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
    // Undone: back where it was, one step at a time.
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Travel', '  Getting started', 'Policies']);
  });

  test('from its menu: up, down, and to a page chosen in "Move…"', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Travel', 'move-up');
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
    await pageAction(page, 'Travel', 'move-down');
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Policies', 'Travel']);

    await pageAction(page, 'Welcome', 'move');
    const dialog = page.getByTestId('move-page');
    await expect(dialog).toContainText('Move “Welcome”');
    // Neither itself nor a page below it.
    await expect(dialog.getByRole('radio', { name: 'Welcome, level 1' })).toBeDisabled();
    await expect(dialog.getByRole('radio', { name: 'Getting started, level 2' })).toBeDisabled();
    await expect(dialog.getByRole('radio', { name: 'Top of the wiki' })).toBeChecked();
    await dialog.getByRole('radio', { name: 'Travel, level 1' }).check();
    await dialog.getByRole('button', { name: 'Move', exact: true }).click();
    await expect(dialog).toBeHidden();
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Policies', 'Travel', '  Welcome', '    Getting started']);
  });

  test('dragged with a mouse between two pages and onto one, never below itself', async ({
    page,
  }, info) => {
    test.skip(touch(info), 'a mouse');
    await openApp(page);
    await wikiWithPages(page);
    const from = await pointOn(row(page, 'Travel'));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 4, from.y - 6, { steps: 3 });
    await expect(page.getByTestId('drag-chip')).toHaveText('Travel');
    // The upper quarter of "Policies": a line before it.
    const above = await pointOn(row(page, 'Policies'), 0.1);
    await page.mouse.move(above.x, above.y, { steps: 6 });
    await nextFrame(page);
    await expect(row(page, 'Policies')).toHaveAttribute('data-drop', 'before');
    await expect(page.getByTestId('drop-line')).toBeVisible();
    await page.mouse.up();
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);

    // Onto "Getting started": below it.
    const start = await pointOn(row(page, 'Policies'));
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    await page.mouse.move(start.x + 4, start.y + 6, { steps: 3 });
    const onto = await pointOn(row(page, 'Getting started'));
    await page.mouse.move(onto.x, onto.y, { steps: 6 });
    await expect(row(page, 'Getting started')).toHaveAttribute('data-drop', 'inside');
    await page.mouse.up();
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', '    Policies', 'Travel']);

    // "Welcome" over a page below it marks nothing, and changes nothing let go.
    const welcome = await pointOn(row(page, 'Welcome'));
    await page.mouse.move(welcome.x, welcome.y);
    await page.mouse.down();
    await page.mouse.move(welcome.x + 4, welcome.y + 6, { steps: 3 });
    const below = await pointOn(row(page, 'Policies'));
    await page.mouse.move(below.x, below.y, { steps: 6 });
    await expect(pageTree(page).locator('[data-drop]')).toHaveCount(0);
    await page.mouse.up();
    expect(await treeShape(page)).toEqual([
      'Welcome',
      '  Getting started',
      '    Policies',
      'Travel',
    ]);
  });

  test('after the last page of a level, as deep as the pointer is far in', async ({
    page,
  }, info) => {
    test.skip(touch(info), 'a mouse');
    await openApp(page);
    await wikiWithPages(page);
    const drag = async (to: { x: number; y: number }): Promise<void> => {
      const from = await pointOn(row(page, 'Travel'));
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(from.x + 4, from.y - 6, { steps: 3 });
      await page.mouse.move(to.x, to.y, { steps: 6 });
      await nextFrame(page);
    };
    // The lower quarter of "Getting started", the last below "Welcome": far in, after it.
    const last = row(page, 'Getting started');
    const box = await last.boundingBox();
    if (!box) throw new Error('no row');
    await drag({ x: box.x + box.width / 2, y: box.y + box.height * 0.9 });
    await expect(last).toHaveAttribute('data-drop', 'after');
    // Far left: after "Welcome", at the top.
    await page.mouse.move(box.x + 2, box.y + box.height * 0.9, { steps: 3 });
    await nextFrame(page);
    await page.mouse.up();
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
  });

  test('dragged with a finger that rests on it first', async ({ page }, info) => {
    test.skip(!touch(info), 'a finger');
    await openApp(page);
    await wikiWithPages(page);
    // The drawer of the tree has slid all the way in.
    await expect
      .poll(async () => (await row(page, 'Travel').boundingBox())?.x ?? -1)
      .toBeGreaterThanOrEqual(0);
    const finger = await Finger.on(page);
    await finger.down(await pointOn(row(page, 'Travel')));
    await expect(page.getByTestId('drag-chip')).toBeVisible();
    await finger.move(await pointOn(row(page, 'Welcome'), 0.1));
    await expect(row(page, 'Welcome')).toHaveAttribute('data-drop', 'before');
    await finger.up();
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Travel', 'Welcome', '  Getting started', 'Policies']);
  });
});

test.describe('a page moved at a phone’s width', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('from its menu in the drawer of the tree', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Policies', 'move');
    const dialog = page.getByTestId('move-page');
    await dialog.getByRole('radio', { name: 'Welcome, level 1' }).check();
    await dialog.getByRole('button', { name: 'Move', exact: true }).click();
    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Welcome', '  Getting started', '  Policies', 'Travel']);
    await hideTree(page);
  });
});
