/**
 * The head of a board at the widths a person meets it at, on the in-memory
 * backend: a window of 1,000 by 800 with the sidebar beside the board (a pane
 * of 744) and folded to its strip (944), with the card search closed and
 * open; a window of 1,280; one of 626; a phone of 390. Whatever the width,
 * the views are behind one button that names the view shown, its name not
 * cut, and the list it opens has every view in its order, each name whole:
 * on its line in a sheet, which wraps it, or in the tooltip of a dropdown's.
 *
 * The first board of the seed has five views: "Gantt" (shown first),
 * "Milestones", "Month calendar", "Status board", "Task table".
 *
 * With `FVOCI_SCREENS=1` it also takes pictures of each, light and dark
 * (`target/screens/board-head/`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { placeOf } from './address.ts';
import { screensDir } from './screens.ts';
import { headCuts as cuts, openApp, shell, sidebar, topBar } from './shell.support.ts';

const SEED_VIEWS = ['Gantt', 'Milestones', 'Month calendar', 'Status board', 'Task table'];

interface Dev {
  backend: {
    views: (boardId: string) => Promise<{ id: string; fields: object }[]>;
    createView: (boardId: string, draft: { title: string; fields: object }) => Promise<unknown>;
  };
}

const header = (page: Page): Locator => page.getByTestId('view-header');
const tool = (page: Page, name: string): Locator => header(page).locator(`[data-tool="${name}"]`);
/** The views in the open list: links to them. */
const listed = (page: Page): Locator =>
  page.getByRole('menu', { name: 'Views' }).locator('a[role="menuitem"]');

async function paneWidth(page: Page): Promise<number> {
  return header(page).evaluate((element) => Math.round(element.getBoundingClientRect().width));
}

async function headerHeight(page: Page): Promise<number> {
  return header(page).evaluate((element) => element.getBoundingClientRect().height);
}

/** The names of the lines of the open list that are cut by their box. */
async function cutLines(page: Page): Promise<string[]> {
  return listed(page).evaluateAll((lines) =>
    lines
      .filter((line) =>
        [...line.querySelectorAll('span')].some((each) => each.scrollWidth > each.clientWidth),
      )
      .map((line) => line.textContent.trim()),
  );
}

async function collapseSidebar(page: Page): Promise<void> {
  await sidebar(page).getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
}

async function openSearch(page: Page): Promise<void> {
  await tool(page, 'search').click();
  await expect(header(page).getByRole('searchbox', { name: 'Search cards' })).toBeFocused();
}

async function closeSearch(page: Page): Promise<void> {
  await header(page).getByRole('searchbox', { name: 'Search cards' }).press('Escape');
  await expect(tool(page, 'search')).toBeFocused();
}

async function openList(page: Page): Promise<void> {
  await tool(page, 'views').click();
  await expect(listed(page).first()).toBeVisible();
}

async function picture(page: Page, name: string): Promise<void> {
  if (!process.env.FVOCI_SCREENS) return;
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: path.join(screensDir('board-head'), `${name}.png`), scale: 'css' });
}

/** Four more views for the first board, one of them with a name far longer than the button. */
async function eightViews(page: Page): Promise<string[]> {
  const names = [
    'Backlog',
    'Done this week',
    'Everything that is still waiting for somebody else to answer before it can go on',
    'Sprint board',
  ];
  await page.evaluate(
    async ({ boardId, names: titles }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const [first] = await backend.views(boardId);
      for (const title of titles) {
        await backend.createView(boardId, { title, fields: first?.fields ?? {} });
      }
    },
    { boardId: placeOf(page.url()).boardId, names },
  );
  return [...SEED_VIEWS, ...names].sort((first, second) => first.localeCompare(second, 'en'));
}

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`on the ${scheme} screen`, () => {
    test.beforeEach(async ({ page }, testInfo) => {
      test.skip(testInfo.project.name !== 'desktop', 'sets its own sizes');
      await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
    });

    test('a window of 1,000 names the view on one button, beside the sidebar or without it, searching or not', async ({
      page,
    }) => {
      await page.setViewportSize({ width: 1000, height: 800 });
      await openApp(page);
      expect(await paneWidth(page)).toBe(744);
      await expect(header(page)).toHaveAttribute('data-layout', 'row');

      // No tabs: one button in the landmark of the views, at the start of the row.
      const views = page.getByRole('navigation', { name: 'Views' });
      await expect(views.getByRole('link')).toHaveCount(0);
      await expect(views.locator('[data-tool="views"]')).toHaveText('Gantt');
      // One row, as high as the tools: the button stands where the tabs did.
      expect(await headerHeight(page)).toBe(48);
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1000-sidebar-${scheme}`);

      // The list: every view in its order, the shown one marked, each name whole.
      await openList(page);
      await expect(listed(page)).toHaveText(SEED_VIEWS);
      await expect(listed(page).first()).toHaveAttribute('aria-current', 'page');
      expect(await cutLines(page)).toEqual([]);
      await picture(page, `1000-sidebar-list-${scheme}`);
      await page.keyboard.press('Escape');
      await expect(tool(page, 'views')).toBeFocused();

      // Searching, the button stays as it was.
      await openSearch(page);
      await expect(tool(page, 'views')).toHaveText('Gantt');
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1000-sidebar-search-${scheme}`);
      await closeSearch(page);

      await collapseSidebar(page);
      expect(await paneWidth(page)).toBe(944);
      await expect(tool(page, 'views')).toHaveText('Gantt');
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1000-strip-${scheme}`);
      await openSearch(page);
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1000-strip-search-${scheme}`);
    });

    test('the keys open the list, find a view by its letters, and leave the focus on the button that names it', async ({
      page,
    }) => {
      test.skip(scheme === 'dark', 'the same on either screen');
      await page.setViewportSize({ width: 1000, height: 800 });
      await openApp(page);
      const views = tool(page, 'views');
      const first = page.url();

      // From the empty search field, Shift+Tab reaches the button; Enter opens the list.
      await openSearch(page);
      await page.keyboard.press('Shift+Tab');
      await expect(views).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(listed(page).first()).toBeFocused();
      // Letters go to the first view whose name starts with them, the arrows on from there.
      // (A space is the key that chooses: it is not one of the letters.)
      await page.keyboard.type('m');
      await expect(listed(page).filter({ hasText: 'Milestones' })).toBeFocused();
      await page.keyboard.press('ArrowDown');
      await expect(listed(page).filter({ hasText: 'Month calendar' })).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(views).toHaveText('Month calendar');
      await expect(views).toBeFocused();
      await expect(page).not.toHaveURL(first);
      // Said to a screen reader: the view shown is another.
      await expect(page.getByTestId('view-shown')).toHaveText('Now showing: Month calendar');

      // Arrows go down the list; Enter on the kind of view to add shows the new one.
      await page.keyboard.press('ArrowDown');
      await expect(listed(page).first()).toBeFocused();
      await page.getByRole('menuitem', { name: 'Add table view' }).focus();
      await page.keyboard.press('Enter');
      await expect(views).toHaveText('Table view');
      await expect(views).toBeFocused();
      await expect(page.getByTestId('view-shown')).toHaveText('Now showing: Table view');

      // Escape closes the list and leaves the view as it is.
      await page.keyboard.press('Enter');
      await page.keyboard.press('Escape');
      await expect(views).toBeFocused();
      await expect(views).toHaveText('Table view');
    });

    test('a window too low for a popover lists the views in a sheet', async ({ page }) => {
      test.skip(scheme === 'dark', 'the same on either screen');
      // Wide enough for the whole row, low enough for a compact page: lists open as sheets.
      await page.setViewportSize({ width: 1280, height: 480 });
      await openApp(page);
      await expect(header(page)).toHaveAttribute('data-layout', 'row');
      await tool(page, 'views').click();
      const sheet = page.getByRole('dialog', { name: 'Views' });
      await expect(sheet).toBeVisible();
      await expect(sheet.locator('a[role="menuitem"]')).toHaveText(SEED_VIEWS);
      await sheet.getByRole('menuitem', { name: 'Task table', exact: true }).click();
      await expect(sheet).toHaveCount(0);
      await expect(tool(page, 'views')).toHaveText('Task table');
    });

    test('a window of 1,280 names the view, searching or not', async ({ page }) => {
      await page.setViewportSize({ width: 1280, height: 800 });
      await openApp(page);
      await expect(header(page)).toHaveAttribute('data-layout', 'row');
      await expect(tool(page, 'views')).toHaveText('Gantt');
      expect(await headerHeight(page)).toBe(48);
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1280-${scheme}`);
      await openList(page);
      await picture(page, `1280-list-${scheme}`);
      await page.keyboard.press('Escape');
      await openSearch(page);
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `1280-search-${scheme}`);
    });

    test('a window of 626 has the folded head with the same button', async ({ page }) => {
      await page.setViewportSize({ width: 626, height: 890 });
      await openApp(page);
      await expect(header(page)).toHaveAttribute('data-layout', 'folded');
      await expect(tool(page, 'views')).toHaveText('Gantt');
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `626-${scheme}`);
      await openList(page);
      await expect(listed(page)).toHaveText(SEED_VIEWS);
      await picture(page, `626-list-${scheme}`);
    });

    test('a phone of 390 has the folded head, and nothing spills', async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await openApp(page);
      await expect(header(page)).toHaveAttribute('data-layout', 'folded');
      await expect(tool(page, 'views')).toHaveText('Gantt');
      await expect(topBar(page).getByTestId('container-description')).toBeHidden();
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `390-${scheme}`);
      await tool(page, 'views').click();
      const sheet = page.getByRole('dialog', { name: 'Views' });
      await expect(sheet.locator('a[role="menuitem"]')).toHaveText(SEED_VIEWS);
      await picture(page, `390-list-${scheme}`);
      await page.keyboard.press('Escape');
      await expect(sheet).toHaveCount(0);
      await openSearch(page);
      expect(await cuts(page)).toEqual({ cut: [], scroll: 0 });
      await picture(page, `390-search-${scheme}`);
    });

    test('eight views, one with a very long name: the button cuts it and keeps it whole in its tooltip', async ({
      page,
    }) => {
      for (const [width, height] of [
        [1280, 800],
        [626, 890],
        [390, 844],
      ] as const) {
        await page.setViewportSize({ width, height });
        await openApp(page);
        const names = await eightViews(page);
        const long = names.find((name) => name.startsWith('Everything')) ?? '';
        await openList(page);
        await expect(listed(page)).toHaveText(names);
        // A sheet wraps it, with no tooltip; a dropdown cuts it, whole in the line's tooltip,
        // which a name shown whole has none of.
        const longLine = listed(page).filter({ hasText: long });
        const shortLine = listed(page).filter({ hasText: names.find((name) => name !== long) });
        await longLine.hover();
        if (width === 390) {
          expect(await cutLines(page)).toEqual([]);
          await expect(longLine).not.toHaveAttribute('title');
        } else {
          expect(await cutLines(page)).toEqual([long]);
          await expect(longLine).toHaveAttribute('title', long);
        }
        await shortLine.first().hover();
        await expect(shortLine.first()).not.toHaveAttribute('title');
        await picture(page, `${String(width)}-eight-list-${scheme}`);
        await listed(page).filter({ hasText: long }).click();
        const views = tool(page, 'views');
        await views.hover();
        await expect(views).toHaveAttribute('title', long);
        // Cut on the button, and the row does not spill.
        expect(
          await views.locator('span').evaluate((name) => name.scrollWidth > name.clientWidth),
        ).toBe(true);
        expect((await cuts(page)).scroll).toBe(0);
        await picture(page, `${String(width)}-eight-${scheme}`);
      }
    });
  });
}
