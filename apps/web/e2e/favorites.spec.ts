/**
 * The reader's stars, on the backend in memory: the ★ of a board at the top
 * bar (where the title leaves it room; in a narrow pane a line of its "…", and
 * of the board's menu in the sidebar) and of a card at its head put them under
 * "Favorites" at the head of the sidebar, where the keyboard moves them as it
 * moves boards; the search, before anything is typed, offers them and what was
 * opened last on this browser, of which no diary is kept; and the stars are
 * buttons of 44 pixels a side.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { ON_A_CARD } from './address.ts';
import {
  boardMenu,
  FIRST_BOARD,
  hideSidebar,
  openApp,
  pane,
  showSidebar,
  sidebar,
  topBar,
} from './shell.support.ts';

const favorites = (page: Page): Locator => sidebar(page).getByTestId('sidebar-favorites');
const starred = (page: Page): Locator => favorites(page).locator('[data-favorite]');

async function titlesOf(list: Locator): Promise<string[]> {
  return (await list.allTextContents()).map((text) => text.trim());
}

/**
 * Stars the board on the page where the bar has room for the star: its button beside the
 * title, a finger wide, from where a view's header has one row (704 pixels); in a narrow
 * pane the line of the bar's "…"; between, the board's line in the sidebar.
 */
async function starTheBoard(page: Page): Promise<void> {
  const width = (await pane(page).boundingBox())?.width ?? 0;
  const button = topBar(page).getByTestId('favorite-toggle');
  if (width >= 704) {
    await expect(button).toHaveAccessibleName('Favorite');
    await expect(button).toHaveAttribute('aria-pressed', 'false');
    const box = await button.boundingBox();
    expect(box && [Math.round(box.width), Math.round(box.height)]).toEqual([44, 44]);
    await button.click();
    await expect(button).toHaveAttribute('aria-pressed', 'true');
    return;
  }
  await expect(button).toBeHidden();
  const menu = width < 544 ? topBar(page).getByTestId('bar-menu') : boardMenu(page, FIRST_BOARD);
  if (width >= 544) await showSidebar(page);
  await menu.click();
  await page.getByRole('menuitem', { name: 'Add to favorites' }).click();
  await menu.click();
  await expect(page.getByRole('menuitem', { name: 'Remove from favorites' })).toBeVisible();
  await page.keyboard.press('Escape');
  await hideSidebar(page);
}

test('a board and a card are starred, listed in the sidebar, and moved by the keyboard', async ({
  page,
}) => {
  await openApp(page);
  await starTheBoard(page);

  await page.locator('[data-card]').first().click();
  await expect(page).toHaveURL(ON_A_CARD);
  const cardStar = page.getByTestId('card-header').getByTestId('favorite-toggle');
  await cardStar.click();
  await expect(cardStar).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Escape');

  await showSidebar(page);
  await expect(starred(page)).toHaveCount(2);
  const [boardName] = await titlesOf(starred(page));
  expect(boardName).toBeTruthy();

  // The card is carried above the board with the keyboard, as a board is among boards.
  await starred(page).nth(1).focus();
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Space');
  await expect.poll(async () => (await titlesOf(starred(page)))[1]).toBe(boardName);

  // Folded by hand, the stars are out of the way.
  await favorites(page).getByRole('button', { name: 'Favorites' }).click();
  await expect(starred(page)).toHaveCount(0);
});

test('the search starts with the stars and what was opened last, and goes there', async ({
  page,
}) => {
  await openApp(page);
  await starTheBoard(page);
  await page.locator('[data-card]').first().click();
  await expect(page).toHaveURL(ON_A_CARD);
  await page.keyboard.press('Escape');

  await topBar(page).getByTestId('open-search').click();
  const dialog = page.getByTestId('search-dialog');
  await expect(dialog.getByTestId('search-favorites').locator('[data-search-place]')).toHaveCount(
    1,
  );
  const recent = dialog.getByTestId('search-recent').locator('[data-search-place]');
  await expect(recent.first()).toBeVisible();
  const kept = await page.evaluate(() =>
    Object.keys(localStorage).filter((key) => key.startsWith('fvoci:recent:')),
  );
  expect(kept).toHaveLength(1);

  // Typed, the stars whose title holds it, and no more what was opened last.
  await page.getByTestId('search-field').fill('zzzz-nothing');
  await expect(dialog.getByTestId('search-favorites')).toHaveCount(0);
  await expect(dialog.getByTestId('search-recent')).toHaveCount(0);
  await page.getByTestId('search-field').fill('');
  await recent.first().click();
  await expect(dialog).toHaveCount(0);
});
