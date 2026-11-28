/**
 * Looking after a wiki in the world in memory: deleting a page with those
 * below it, the wiki's own name, description and icon, deleting the wiki and
 * bringing it back from the trash, reading an open wiki before joining it and
 * writing in it after, sharing a wiki from its head, reading one the
 * reader may not change, the tree with the keyboard alone, and the drawer
 * of a phone's width.
 */
import { expect, test, type Page } from '@playwright/test';

import { primaryKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';
import {
  boardLink,
  FIRST_BOARD,
  hideSidebar,
  openApp,
  pressInBar,
  showSidebar,
  sidebar,
} from './shell.support.ts';
import {
  deleteBySomebodyElse,
  goInside,
  hideTree,
  letSomebodyElseIn,
  nameThePage,
  newWiki,
  ON_A_PAGE,
  ON_A_WIKI,
  pageAction,
  pageBody,
  restoreBySomebodyElse,
  showTree,
  treeItem,
  treeShape,
  wikiBySomebodyElse,
  wikiPath,
  wikiPlaceOf,
  wikiScreen,
} from './wiki.support.ts';

/** A wiki with "Welcome", "Getting started" below it and "Policies" after it; on "Policies". */
async function wikiWithPages(page: Page): Promise<string> {
  const wikiId = await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  return wikiId;
}

test.describe('a wiki looked after', () => {
  test('a wiki is copied whole from its menu in the sidebar, and the copy is opened', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await wikiWithPages(page);
    await showSidebar(page);
    await sidebar(page)
      .getByRole('button', { name: 'Menu of the wiki Handbook', exact: true })
      .click();
    await page.getByRole('menuitem', { name: 'Duplicate wiki' }).click();
    await expect(page).not.toHaveURL(new RegExp(wikiId));
    await expect(page).toHaveURL(ON_A_WIKI);
    await showSidebar(page);
    await expect(sidebar(page).getByRole('link', { name: 'Handbook copy' })).toBeVisible();
    await hideSidebar(page);
    // The copy opens folded: its pages below are there once "Welcome" is opened.
    await showTree(page);
    await treeItem(page, 'Welcome').focus();
    await page.keyboard.press('ArrowRight');
    expect(await treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });

  test('a wiki is saved from its menu as an archive and imported again from the settings', async ({
    page,
  }, testInfo) => {
    await openApp(page);
    await wikiWithPages(page);
    await showSidebar(page);
    const saving = page.waitForEvent('download');
    await sidebar(page)
      .getByRole('button', { name: 'Menu of the wiki Handbook', exact: true })
      .click();
    await page.getByRole('menuitem', { name: 'As a .fvoci archive' }).click();
    const archive = testInfo.outputPath('wiki.fvoci');
    await (await saving).saveAs(archive);
    await showSidebar(page);
    await sidebar(page).getByTestId('open-settings').click();
    await page.getByTestId('settings-dialog').getByTestId('import-archive').setInputFiles(archive);
    await expect(
      page.getByTestId('transfers').locator('[data-transfer="import"][data-finished]'),
    ).toContainText('wiki.fvoci is imported.');
    await page.keyboard.press('Escape');
    await showSidebar(page);
    const titled = async () =>
      (await sidebar(page).getByRole('link').allInnerTexts()).filter((text) => text === 'Handbook');
    await expect.poll(async () => (await titled()).length).toBe(2);
  });

  test('deleting a page asks first, says those below go too, and takes them along', async ({
    page,
  }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Welcome', 'delete');
    const asked = page.getByTestId('confirm-dialog');
    await expect(asked).toContainText('“Welcome”');
    await expect(asked).toContainText('(1)');
    // Cancelled, nothing is gone.
    await asked.getByRole('button', { name: 'Cancel' }).click();
    await showTree(page);
    expect(await treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);

    await pageAction(page, 'Welcome', 'delete');
    await asked.getByRole('button', { name: 'Delete' }).click();
    await showTree(page);
    expect(await treeShape(page)).toEqual(['Policies']);
    // The focus is in the tree, where the deleted page's row was.
    await expect(treeItem(page, 'Policies')).toBeFocused();
    await expect(page.getByTestId('page-title')).toHaveValue('Policies');
  });

  test('the page shown deleted, the first page left is shown', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await pageAction(page, 'Policies', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');
    await expect(page.getByTestId('wiki-notice')).toHaveCount(0);
  });

  test('a press on the question a menu’s line asked takes while the menu is still going away', async ({
    page,
  }) => {
    await openApp(page);
    await wikiWithPages(page);
    // Where the question's "Delete" is once the question has come to rest.
    await pageAction(page, 'Policies', 'delete');
    const asked = page.getByTestId('confirm-dialog');
    const goAhead = asked.getByRole('button', { name: 'Delete' });
    await goAhead.click({ trial: true });
    const box = await goAhead.boundingBox();
    if (!box) throw new Error('no button');
    await asked.getByRole('button', { name: 'Cancel' }).click();
    await expect(asked).toBeHidden();
    // Asked again and pressed there at once: nothing waits for the menu to be gone. At a
    // phone's width it is a sheet, which lies over that place while it goes (elsewhere a
    // dropdown beside the tree, which does not).
    await pageAction(page, 'Policies', 'delete');
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');
  });

  test('the wiki’s description is added behind "ⓘ" from the keyboard alone, and given up with Escape', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard and a mouse');
    await openApp(page);
    await newWiki(page, 'Handbook');
    await hideSidebar(page);
    const about = page.getByTestId('top-bar').getByTestId('container-description');
    const field = page.getByTestId('container-description-field');
    await expect(about).toHaveAccessibleName('Add description');
    await about.focus();
    await page.keyboard.press('Enter');
    await expect(field).toBeFocused();
    await page.keyboard.type('How we work.');
    await page.keyboard.press(await primaryKey(page, 'Enter'));
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();
    await expect(page.getByTestId('wiki-header-description')).toHaveText('How we work.');
    await expect(about).toHaveAccessibleName('Description');

    await page.keyboard.press('Enter');
    await expect(field).toHaveValue('How we work.');
    await page.keyboard.type(' Not kept.');
    await page.keyboard.press('Escape');
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();
    await expect(page.getByTestId('wiki-header-description')).toHaveText('How we work.');
  });

  test('a board’s description being written is saved to the board when the reader goes back to a wiki, whose bar then follows the wiki', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard and a mouse');
    await openApp(page);
    await newWiki(page, 'Handbook');
    await showSidebar(page);
    await boardLink(page, FIRST_BOARD).click();
    const bar = page.getByTestId('top-bar');
    await expect(bar.getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    const about = bar.getByTestId('container-description');
    const field = page.getByTestId('container-description-field');
    await about.click();
    await field.fill('Written, then left for a wiki.');
    // Back to the wiki with no click outside: nothing closed the description first.
    await page.goBack();
    await expect(wikiScreen(page)).toBeVisible();
    await expect(field).toHaveCount(0);
    await expect
      .poll(() =>
        page.evaluate(async (title) => {
          const { backend } = (
            window as unknown as {
              fvociDev: {
                backend: {
                  workspaces: () => Promise<{ id: string }[]>;
                  boards: (id: string) => Promise<{ title: string; description: string }[]>;
                };
              };
            }
          ).fvociDev;
          const [workspace] = await backend.workspaces();
          const boards = workspace ? await backend.boards(workspace.id) : [];
          return boards.find((board) => board.title === title)?.description;
        }, FIRST_BOARD),
      )
      .toBe('Written, then left for a wiki.');

    // The wiki's "ⓘ" is the wiki's, and follows what is saved to it.
    await expect(about).toHaveAccessibleName('Add description');
    await about.click();
    await expect(field).toHaveValue('');
    await field.fill('How we work.');
    await page.keyboard.press(await primaryKey(page, 'Enter'));
    await expect(about).toHaveAccessibleName('Description');
    await expect(page.getByTestId('wiki-header-description')).toHaveText('How we work.');
  });

  test('the wiki’s name, description and icon are changed, undone, and the wiki deleted and restored', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await wikiWithPages(page);
    await showTree(page);
    await page.getByTestId('wiki-menu').click();
    await page.locator('[data-action="edit-wiki"]').click();
    const settings = page.getByTestId('wiki-settings');
    await settings.getByTestId('wiki-settings-name').fill('Guide');
    await settings.getByTestId('wiki-settings-description').fill('How we work.');
    await settings.getByTestId('wiki-settings-random-icon').click();
    await settings.getByTestId('wiki-settings-save').click();
    await expect(page.getByTestId('wiki-header-title')).toHaveText('Guide');
    await expect(page.getByTestId('wiki-header-description')).toHaveText('How we work.');
    await expect(page.getByTestId('wiki-header-icon')).not.toBeEmpty();
    // The tree's drawer is put away first: the bar above is kept away while it is out.
    await hideTree(page);
    await showSidebar(page);
    await expect(sidebar(page).getByRole('link', { name: 'Guide, wiki' })).toBeVisible();
    await hideSidebar(page);

    await pressInBar(page, 'undo');
    await showSidebar(page);
    await expect(sidebar(page).getByRole('link', { name: 'Handbook, wiki' })).toBeVisible();
    await hideSidebar(page);
    await showTree(page);
    await expect(page.getByTestId('wiki-header-description')).toHaveCount(0);
    await expect(page.getByTestId('wiki-header-icon')).toHaveCount(0);

    // Deleted from its own menu: to the first of the sidebar, and back from the trash.
    await showTree(page);
    await page.getByTestId('wiki-menu').click();
    await page.locator('[data-action="delete-wiki"]').click();
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await expect(wikiScreen(page)).toHaveCount(0);
    await showSidebar(page);
    await expect(sidebar(page).getByRole('link', { name: 'Handbook, wiki' })).toHaveCount(0);
    await sidebar(page).getByRole('button', { name: 'Trash' }).click();
    const trash = page.getByTestId('board-trash');
    const item = trash.getByTestId('trash-items').locator('li', { hasText: 'Handbook' });
    await expect(item.getByTestId('trash-kind')).toHaveText('wiki');
    await item.getByRole('button', { name: /^Restore/ }).click();
    await page.keyboard.press('Escape');
    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Handbook, wiki' }).click();
    await expect(page).toHaveURL(new RegExp(`${wikiPath(wikiId)}/`));
    await showTree(page);
    expect(await treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies']);
  });

  test('an open wiki somebody else made is written as its members do: pressing its body joins it', async ({
    page,
  }) => {
    await openApp(page);
    const made = await wikiBySomebodyElse(page, {
      title: 'Design notes',
      visibility: 'open',
      page: 'Hello',
    });
    await goInside(page, wikiPath(made.wikiId));
    await expect(page).toHaveURL(wikiPath(made.wikiId, made.pageId));
    // No gate to press: what its members may do is offered, and the sidebar keeps it apart.
    await expect(page.getByTestId('page-title')).toHaveValue('Hello');
    await expect(page.getByTestId('wiki-join')).toHaveCount(0);
    await showSidebar(page);
    const waiting = sidebar(page).getByTestId('open-to-join');
    await expect(waiting.getByRole('link', { name: 'Design notes, wiki' })).toBeVisible();
    await hideSidebar(page);
    // Its members are there to see, not to change.
    await showTree(page);
    await wikiScreen(page).getByTestId('share-wiki').click();
    const dialog = page.getByTestId('share-dialog');
    await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(1);
    await expect(dialog.getByTestId('share-open')).toHaveCount(0);
    await page.keyboard.press('Escape');
    await hideTree(page);
    // Pressed, the body joins the wiki, and its room lets the reader in to write.
    const body = pageBody(page);
    await body.click();
    await expect(body).toHaveAttribute('contenteditable', 'true');
    await body.click();
    await page.keyboard.type('Written after joining');
    await expect(body).toContainText('Written after joining');
    // A member now: among the reader's own in the sidebar.
    await showSidebar(page);
    await expect(sidebar(page).getByTestId('open-to-join')).toHaveCount(0);
    await expect(sidebar(page).getByRole('link', { name: 'Design notes, wiki' })).toBeVisible();
  });

  test('a wiki is shared from its head: closed and opened to the workspace, and a member added', async ({
    page,
  }) => {
    await openApp(page);
    await newWiki(page, 'Shared notes');
    await showTree(page);
    await wikiScreen(page).getByTestId('share-wiki').click();
    const dialog = page.getByTestId('share-dialog');
    await expect(dialog.getByRole('heading', { name: 'Share wiki' })).toBeVisible();
    // Made open, as "New wiki" makes it; closed and opened again.
    const open = dialog.getByTestId('share-open');
    await expect(dialog).toContainText('can read this wiki and join it');
    await expect(open).toBeChecked();
    await open.click();
    await expect(open).not.toBeChecked();
    await open.click();
    await expect(open).toBeChecked();
    await dialog.getByTestId('share-candidates').getByRole('button').first().click();
    await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(2);
    // A wiki's public link is offered as a board's is.
    await expect(dialog.getByTestId('share-public')).toBeVisible();
  });

  for (const role of ['viewer', 'commenter']) {
    test(`a ${role} of a wiki sees its pages and no way to change them`, async ({ page }) => {
      await openApp(page);
      const made = await wikiBySomebodyElse(page, {
        title: 'Rules',
        visibility: 'private',
        page: 'Read me',
        readerRole: role,
      });
      await goInside(page, wikiPath(made.wikiId, made.pageId));
      await expect(page.locator('h1[data-testid="page-title"]')).toHaveText('Read me');
      await expect(page.getByTestId('page-add-icon')).toHaveCount(0);
      const body = page.getByTestId('page-body');
      await expect(body).toHaveAttribute('data-editor', 'reading');
      // The editor's placeholder, drawn from its attribute, says the page is empty.
      await expect(body.locator('[data-placeholder]').first()).toHaveAttribute(
        'data-placeholder',
        'There is nothing on this page yet.',
      );
      await expect(body).toContainText('You can read this page but not change it.');
      await showTree(page);
      await expect(page.getByTestId('new-page')).toHaveCount(0);
      await expect(page.getByTestId('wiki-menu')).toHaveCount(0);
      await expect(treeItem(page, 'Read me').getByRole('button', { name: /^Menu of/ })).toHaveCount(
        0,
      );
    });
  }
});

test.describe('the tree with the keyboard alone', () => {
  test('moves, folds, opens and shows pages, and makes one inside a page', async ({ page }) => {
    await openApp(page);
    await wikiWithPages(page);
    await showTree(page);
    // Tab from the button before the tree reaches one page of it: the page shown.
    await page.getByTestId('new-page').focus();
    await page.keyboard.press('Tab');
    const welcome = treeItem(page, 'Welcome');
    const started = treeItem(page, 'Getting started');
    const policies = treeItem(page, 'Policies');
    await expect(policies).toBeFocused();
    await page.keyboard.press('Home');
    await expect(welcome).toBeFocused();
    // Left folds an open page, right opens it again and stays, then goes into it.
    await page.keyboard.press('ArrowLeft');
    await expect(welcome).toHaveAttribute('aria-expanded', 'false');
    await expect(started).toHaveCount(0);
    await page.keyboard.press('ArrowRight');
    await expect(welcome).toHaveAttribute('aria-expanded', 'true');
    await expect(welcome).toBeFocused();
    await page.keyboard.press('ArrowRight');
    await expect(started).toBeFocused();
    // Left from a page without children goes to its parent; down and up go row by row.
    await page.keyboard.press('ArrowLeft');
    await expect(welcome).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(started).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(policies).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await expect(started).toBeFocused();
    await page.keyboard.press('End');
    await expect(policies).toBeFocused();
    await page.keyboard.press('Home');
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');

    // The menu from the keyboard: a page inside "Policies", named, and the tree has it.
    await showTree(page);
    await policies.focus();
    await page.keyboard.press('Shift+F10');
    await expect(page.locator('[data-action="add-child"]')).toBeVisible();
    await page.locator('[data-action="add-child"]').press('Enter');
    await nameThePage(page, 'Leave');
    await showTree(page);
    expect(await treeShape(page)).toEqual(['Welcome', '  Getting started', 'Policies', '  Leave']);
    await expect(page).toHaveURL(ON_A_PAGE);
    expect(wikiPlaceOf(page.url()).pageId).not.toBe('');
  });
});

test.describe('at a phone’s width', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('the tree is a drawer: opened, a page made and chosen in it, and put away', async ({
    page,
  }) => {
    await openApp(page);
    await wikiWithPages(page);
    const drawer = page.getByRole('dialog', { name: 'Pages' });
    await expect(drawer).toBeHidden();
    await page.getByTestId('open-page-tree').click();
    await expect(drawer).toBeVisible();
    // Once it has slid in, nothing of it is past the edge of the screen.
    await expect
      .poll(async () => {
        const box = await drawer.boundingBox();
        return box !== null && box.x >= 0 && box.x + box.width <= 390;
      })
      .toBe(true);
    await treeItem(page, 'Welcome').click();
    await expect(drawer).toBeHidden();
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');
    await expect(pageBody(page)).toBeVisible();

    // A page made in the drawer: the drawer goes, the new page's title has the caret.
    await page.getByTestId('open-page-tree').click();
    await page.getByTestId('new-page').click();
    await expect(drawer).toBeHidden();
    await nameThePage(page, 'From the drawer');
    await page.getByTestId('open-page-tree').click();
    await expect(treeItem(page, 'From the drawer')).toBeVisible();
    await page.keyboard.press('Escape');

    await page.getByTestId('open-page-tree').click();
    await page.getByTestId('page-tree-scrim').click({ position: { x: 380, y: 400 } });
    await expect(drawer).toBeHidden();
    await hideTree(page);
  });

  test('while the drawer is out the bar above is kept away, and the drawer goes with a page deleted elsewhere', async ({
    page,
  }) => {
    await openApp(page);
    const wikiId = await wikiWithPages(page);
    await letSomebodyElseIn(page, wikiId);
    await hideTree(page);
    await expect(page.getByTestId('page-title')).toHaveValue('Policies');
    const { pageId } = wikiPlaceOf(page.url());
    const drawer = page.getByRole('dialog', { name: 'Pages' });
    const bar = page.getByTestId('top-bar');
    await expect(bar).not.toHaveAttribute('inert', '');
    await page.getByTestId('open-page-tree').click();
    await expect(drawer).toBeVisible();
    // The bar is no part of the wiki's screen, and as far away as the page under the drawer.
    await expect(bar).toHaveAttribute('inert', '');

    // Somebody else deletes the page shown: the screen shows another, with no drawer over it.
    await deleteBySomebodyElse(page, wikiId, pageId);
    await expect(page.getByTestId('page-title')).toHaveValue('Welcome');
    await expect(drawer).toBeHidden();
    await expect(page.getByTestId('open-page-tree')).toHaveAttribute('aria-expanded', 'false');
    await expect(bar).not.toHaveAttribute('inert', '');

    // The page is back and the reader follows an address to it (a link, a hit of the
    // search): the drawer that was out over it is not.
    await restoreBySomebodyElse(page, wikiId, pageId);
    await page.evaluate(
      (address) => {
        history.pushState(null, '', address);
        dispatchEvent(new PopStateEvent('popstate'));
      },
      wikiPath(wikiId, pageId),
    );
    await expect(page.getByTestId('page-title')).toHaveValue('Policies');
    expect(wikiPlaceOf(page.url()).pageId).toBe(pageId);
    await expect(drawer).toBeHidden();
    await expect(bar).not.toHaveAttribute('inert', '');
    // The drawer left nothing in the history: the tree opens and closes as before.
    await page.getByTestId('open-page-tree').click();
    await expect(drawer).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();
    await expect(page.getByTestId('page-title')).toHaveValue('Policies');
  });
});

test.describe('screens', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');
  test.use({ locale: 'ko-KR' });

  /** Opens a page's menu in the tree whatever the language, and presses one of its lines. */
  async function onPage(page: Page, name: string, action: string): Promise<void> {
    await showTree(page);
    // The row's last button is its menu's (a sheet in a narrow pane, a menu elsewhere).
    await treeItem(page, name).getByRole('button').last().click();
    await page.locator(`[data-action="${action}"]`).click();
  }

  for (const [width, height, size] of [
    [1280, 800, 'wide'],
    [390, 844, '390'],
  ] as const) {
    test(`the wiki in Korean, ${size}`, async ({ page }, info) => {
      test.skip(info.project.name !== 'desktop', 'one set of pictures');
      const shot = async (name: string): Promise<void> => {
        // A drawer of the tree that is out has slid all the way in first.
        const drawer = page.getByRole('dialog', { name: '페이지' });
        if (await drawer.isVisible()) {
          await expect.poll(async () => (await drawer.boundingBox())?.x).toBe(0);
        }
        await page.screenshot({ path: `${screensDir('wiki')}/${name}-${size}.png` });
      };
      await page.setViewportSize({ width, height });
      await openApp(page);

      // "New wiki", as it asks.
      await showSidebar(page);
      await sidebar(page).getByTestId('add-board').click();
      const picker = page.getByTestId('template-picker');
      await picker.getByTestId('new-title').fill('팀 안내서');
      await shot('new-wiki-dialog');
      await picker.getByTestId('new-wiki').click();
      await expect(page).toHaveURL(ON_A_WIKI);
      await expect(page.getByTestId('wiki-empty')).toBeVisible();
      // The sidebar's drawer, which made the wiki, has slid all the way out.
      if (size === '390') await expect(sidebar(page)).toBeHidden();
      await shot('empty-wiki');

      // Pages three deep, a body, a viewer.
      await page.getByTestId('wiki-first-page').click();
      await nameThePage(page, '환영합니다');
      await page.getByTestId('page-add-icon').click();
      await onPage(page, '환영합니다', 'add-child');
      await nameThePage(page, '처음 오신 분께');
      await onPage(page, '처음 오신 분께', 'add-child');
      await nameThePage(page, '계정 만들기');
      await onPage(page, '환영합니다', 'add-after');
      await nameThePage(page, '일하는 방식');
      await onPage(page, '환영합니다', 'add-after');
      await nameThePage(page, '휴가와 근무 시간에 대한 아주 긴 제목의 페이지');
      await showTree(page);
      await treeItem(page, '계정 만들기').click();
      await hideTree(page);
      await pageBody(page).click();
      await page.keyboard.type(
        '계정은 첫날 받는 메일의 링크로 만듭니다. 비밀번호는 열두 자 이상으로 정하세요.',
      );
      await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
      await showTree(page);
      await shot('wiki-screen');

      // The page itself, the tree put away where it is a drawer.
      await hideTree(page);
      await shot('page');

      // The sidebar, boards and wikis in one list.
      await hideTree(page);
      await showSidebar(page);
      await shot('sidebar');
      await hideSidebar(page);

      // Its people, from the head of the tree.
      await showTree(page);
      await wikiScreen(page).getByTestId('share-wiki').click();
      await expect(page.getByTestId('share-dialog')).toBeVisible();
      await shot('share');
      await page.keyboard.press('Escape');
      await expect(page.getByTestId('share-dialog')).toHaveCount(0);

      // An open wiki of somebody else's, read before it is joined: written as its members do.
      const made = await wikiBySomebodyElse(page, {
        title: '디자인 노트',
        visibility: 'open',
        page: '색',
      });
      await goInside(page, wikiPath(made.wikiId));
      await expect(page.getByTestId('page-title')).toHaveValue('색');
      await expect(page.getByTestId('wiki-join')).toHaveCount(0);
      // The sidebar's drawer has slid all the way out.
      if (size === '390') await expect(sidebar(page)).toBeHidden();
      await shot('join');
    });
  }
});
