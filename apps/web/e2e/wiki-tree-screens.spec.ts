/**
 * Pictures of the tree of a wiki in Korean, wide and at 390 pixels, taken
 * only with `FVOCI_SCREENS=1` (`screens.ts`): a page dragged with its drop
 * line, "Move…", the menu of a page with "Duplicate", and the deleted pages.
 */
import { expect, test, type Page } from '@playwright/test';

import { nextFrame, pointOn } from './kanban.support.ts';
import { screensDir } from './screens.ts';
import { openApp } from './shell.support.ts';
import { hideTree, nameThePage, newWiki, pageTree, showTree, treeItem } from './wiki.support.ts';

test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');
test.use({ locale: 'ko-KR' });

/** Opens a page's menu in the tree whatever the language. */
async function menuOf(page: Page, name: string): Promise<void> {
  await showTree(page);
  await treeItem(page, name).getByRole('button').last().click();
}

async function onPage(page: Page, name: string, action: string): Promise<void> {
  await menuOf(page, name);
  await page.locator(`[data-action="${action}"]`).click();
}

for (const [width, height, size] of [
  [1280, 800, 'wide'],
  [390, 844, '390'],
] as const) {
  test(`the tree of a wiki in Korean, ${size}`, async ({ page }, info) => {
    test.skip(info.project.name !== 'desktop', 'one set of pictures');
    const shot = async (name: string): Promise<void> => {
      const drawer = page.getByRole('dialog', { name: '페이지', exact: true });
      if (await drawer.isVisible()) {
        await expect.poll(async () => (await drawer.boundingBox())?.x).toBe(0);
      }
      // A menu's sheet put away has slid out first.
      await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
      await page.screenshot({ path: `${screensDir('wiki-tree')}/${name}-${size}.png` });
    };
    await page.setViewportSize({ width, height });
    await openApp(page);
    await newWiki(page, '팀 안내서');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, '환영합니다');
    await onPage(page, '환영합니다', 'add-child');
    await nameThePage(page, '처음 오신 분께');
    await onPage(page, '환영합니다', 'add-after');
    await nameThePage(page, '일하는 방식');
    await onPage(page, '일하는 방식', 'add-after');
    await nameThePage(page, '휴가와 출장');

    // A page dragged before another: its title follows the pointer, a line where it goes.
    await showTree(page);
    const row = (name: string) => pageTree(page).locator(`[data-page][aria-label="${name}"]`);
    const from = await pointOn(row('휴가와 출장'));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 4, from.y - 6, { steps: 3 });
    const above = await pointOn(row('일하는 방식'), 0.1);
    await page.mouse.move(above.x + 30, above.y, { steps: 6 });
    await nextFrame(page);
    await expect(page.getByTestId('drop-line')).toBeVisible();
    await shot('drag-drop-line');
    await page.keyboard.press('Escape');
    await page.mouse.up();

    // "Move…", the page and those below it not to be chosen.
    await onPage(page, '환영합니다', 'move');
    const dialog = page.getByTestId('move-page');
    await expect(dialog).toBeVisible();
    await shot('move-dialog');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();

    // The menu of a page, with "복제" (its own sheet at 390).
    await menuOf(page, '일하는 방식');
    await expect(page.locator('[data-action="duplicate"]')).toBeVisible();
    await page.screenshot({ path: `${screensDir('wiki-tree')}/page-menu-${size}.png` });
    await page.keyboard.press('Escape');

    // Deleted pages: one with a page below it.
    await onPage(page, '환영합니다', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: '삭제' }).click();
    await onPage(page, '휴가와 출장', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: '삭제' }).click();
    await showTree(page);
    await page.getByTestId('open-page-trash').click();
    const trash = page.getByTestId('page-trash');
    await expect(trash.getByTestId('trash-below')).toBeVisible();
    await shot('page-trash');
    await page.keyboard.press('Escape');
    await hideTree(page);
  });
}
