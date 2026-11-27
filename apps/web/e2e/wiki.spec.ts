/**
 * Wikis, end to end, on the in-memory backend: made from "New", filed in the
 * sidebar among the boards, at an address of their own that no board's
 * screen opens (nor a board a wiki's).
 */
import { expect, test } from '@playwright/test';

import { boardPath, ON_A_BOARD, WORKSPACE } from './address.ts';
import {
  boardLink,
  boardsIn,
  FIRST_BOARD,
  openApp,
  pressInBar,
  showSidebar,
  sidebar,
  stageOf,
  strip,
  topBar,
} from './shell.support.ts';
import {
  deleteBySomebodyElse,
  goInside,
  letSomebodyElseIn,
  pageBody,
  pageBySomebodyElse,
  typeAsSomebodyElse,
  newWiki,
  nameThePage,
  ON_A_PAGE,
  pageAction,
  hideTree,
  showTree,
  treeItem,
  treeShape,
  wikiPath,
  wikiPlaceOf,
  wikiScreen,
} from './wiki.support.ts';

test.describe('a new wiki', () => {
  test('is made from "New", shown empty at its address, and filed among the boards', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    await expect(page).toHaveURL(wikiPath(wikiId));

    // Empty: the way to its first page.
    await expect(page.getByTestId('wiki-empty')).toBeVisible();
    await expect(page.getByTestId('wiki-first-page')).toHaveText('Make the first page');
    await expect(topBar(page)).toContainText('Handbook');

    await showSidebar(page);
    const link = sidebar(page).getByRole('link', { name: 'Handbook, wiki', exact: true });
    await expect(link).toHaveAttribute('aria-current', 'page');
    await expect(link.locator('[data-board-icon]')).toHaveAttribute('data-kind', 'wiki');
    // The default category holds boards and wikis, under a name of neither.
    expect(await boardsIn(page, 'General')).toContain('Handbook');
    await expect(
      sidebar(page).getByRole('button', { name: 'Menu of the wiki Handbook' }),
    ).toBeVisible();
    if ((await stageOf(page)) === 'medium') {
      await expect(strip(page).getByRole('link', { name: 'Handbook, wiki' })).toBeVisible();
    }
  });

  test('is opened from the sidebar, and a board from there again', async ({ page }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    await showSidebar(page);
    await boardLink(page, FIRST_BOARD).click();
    await expect(page).toHaveURL(ON_A_BOARD);
    await expect(page.getByTestId('board-page')).toBeVisible();
    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Handbook, wiki' }).click();
    await expect(page).toHaveURL(wikiPath(wikiId));
    await expect(wikiScreen(page)).toBeVisible();
  });

  test('is no board: not at a board’s address, nor among the templates; a board is no wiki', async ({
    page,
  }) => {
    await openApp(page);
    const boardId = new URL(page.url()).pathname.split('/')[4] ?? '';
    const wikiId = await newWiki(page, 'Handbook');

    await goInside(page, boardPath(wikiId));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(page.getByTestId('board-page')).toHaveCount(0);

    await goInside(page, wikiPath(boardId));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(wikiScreen(page)).toHaveCount(0);

    await goInside(page, boardPath(boardId));
    await expect(page.getByTestId('board-page')).toBeVisible();
    await showSidebar(page);
    await sidebar(page).getByTestId('add-board').click();
    const picker = page.getByTestId('template-picker');
    await expect(picker.getByTestId('templates')).toBeAttached();
    await expect(picker.getByText('Handbook')).toHaveCount(0);
  });

  test('a wiki’s address naming no wiki is not found, and the root goes to the last place', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    // The workspace's address alone goes back to where the reader was: the wiki.
    await goInside(page, `/w/${WORKSPACE}`);
    await expect(page).toHaveURL(wikiPath(wikiId));
    await goInside(page, wikiPath('01930000-0000-7000-8000-00000000ffff'));
    await expect(page.getByTestId('not-found')).toBeVisible();
  });
});

test.describe('the pages of a wiki', () => {
  test('are made at the top, inside a page and below one, and kept in a tree', async ({ page }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await expect(page).toHaveURL(ON_A_PAGE);
    await nameThePage(page, 'Welcome');
    const welcome = wikiPlaceOf(page.url()).pageId;

    await pageAction(page, 'Welcome', 'add-child');
    await nameThePage(page, 'Getting started');
    await pageAction(page, 'Getting started', 'add-child');
    await nameThePage(page, 'Accounts');
    await pageAction(page, 'Welcome', 'add-after');
    await nameThePage(page, 'Policies');
    await showTree(page);
    await page.getByTestId('new-page').click();
    await nameThePage(page, 'Archive');

    await showTree(page);
    expect(await treeShape(page)).toEqual([
      'Welcome',
      '  Getting started',
      '    Accounts',
      'Policies',
      'Archive',
    ]);
    await expect(treeItem(page, 'Archive')).toHaveAttribute('aria-selected', 'true');

    // The wiki's address alone shows its first page, in place of that address.
    await goInside(page, wikiPath(wikiId));
    await expect(page).toHaveURL(wikiPath(wikiId, welcome));
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');
  });

  test('take an icon and a new title, which the tree follows, and undo puts them back', async ({
    page,
  }) => {
    await openApp(page);
    await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, 'Welcome');
    await page.getByTestId('page-add-icon').click();
    await expect(page.getByTestId('page-icon')).toBeVisible();
    await showTree(page);
    await expect(treeItem(page, 'Welcome').getByTestId('tree-icon')).not.toBeEmpty();
    await hideTree(page);

    const title = page.getByTestId('page-title');
    await title.fill('Hello');
    await title.press('Enter');
    await showTree(page);
    await expect(treeItem(page, 'Hello')).toBeVisible();
    await hideTree(page);
    await pressInBar(page, 'undo');
    await expect(title).toHaveValue('Welcome');
    await showTree(page);
    await expect(treeItem(page, 'Welcome')).toBeVisible();
    await hideTree(page);
    await pressInBar(page, 'undo');
    await expect(page.getByTestId('page-add-icon')).toBeVisible();
    await expect(page.getByTestId('page-icon')).toHaveCount(0);
  });

  test('in a narrow pane the tree is a drawer that takes the focus, keeps it, and gives it back', async ({
    page,
  }) => {
    await openApp(page);
    await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, 'Welcome');
    const opener = page.getByTestId('open-page-tree');
    test.skip(!(await opener.isVisible()), 'the tree is a column beside the page here');

    const drawer = page.getByRole('dialog', { name: 'Pages' });
    await opener.click();
    await expect(treeItem(page, 'Welcome')).toBeFocused();
    // Tab goes round inside the drawer.
    for (let step = 0; step < 4; step += 1) await page.keyboard.press('Tab');
    expect(await drawer.evaluate((element) => element.contains(document.activeElement))).toBe(true);
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();
    await expect(opener).toBeFocused();

    // A page chosen in the drawer is shown, the drawer gone and its button focused.
    await opener.click();
    await expect(treeItem(page, 'Welcome')).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expect(opener).toBeFocused();
  });

  test('a page’s body is written together, who else is on it shows, and the tree follows their pages', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, 'Welcome');
    const place = wikiPlaceOf(page.url());
    await letSomebodyElseIn(page, wikiId);

    const body = pageBody(page);
    await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
    await body.click();
    await page.keyboard.type('From the reader.');
    // Theirs has what the reader typed; the reader's, what they add to it.
    const theirs = await typeAsSomebodyElse(page, place, 'From the reader.', ' From them.');
    expect(theirs).toContain('From the reader.');
    await expect(body).toHaveText('From the reader. From them.');
    // They are on the page: at its head, and on its row of the tree.
    await expect(page.getByTestId('page-heading').getByTestId('page-viewers')).toBeVisible();

    // Pages they make and delete show in the tree, those below a deleted one gone too.
    const theirPage = await pageBySomebodyElse(page, wikiId, { title: 'Theirs' });
    await pageBySomebodyElse(page, wikiId, { title: 'Below theirs', parentId: theirPage });
    await showTree(page);
    await expect(treeItem(page, 'Welcome').getByTestId('page-viewers')).toBeVisible();
    await expect(treeItem(page, 'Theirs')).toBeVisible();
    await deleteBySomebodyElse(page, wikiId, theirPage);
    await expect(treeItem(page, 'Theirs')).toHaveCount(0);
    expect(await treeShape(page)).toEqual(['Welcome']);
  });

  test('a page that is not there is not found', async ({ page }) => {
    await openApp(page);
    const wikiId = await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, 'Welcome');
    await goInside(page, wikiPath(wikiId, '01930000-0000-7000-8000-00000000ffff'));
    await expect(page.getByTestId('not-found')).toBeVisible();
  });
});
