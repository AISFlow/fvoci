/**
 * Board views in a body, on the backend in memory: one is put into a page's
 * body from the slash menu (a board, then a view) and shows the view's
 * cards under a header naming the board and the view, with a link to the
 * view on the board; the address of a view pasted is a link until it is
 * taken as a board view; a card pressed in one opens over the page; and on
 * a phone the cards scroll within the view, no higher than 60% of the
 * screen, while the page does not move sideways.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import type { Dev } from './embeds.support.ts';
import { openApp } from './shell.support.ts';
import { paste, slash } from './tables.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Notes');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Plans');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

/** The first board of the workspace with cards, and its first view, as the reader is told of them. */
async function firstView(page: Page) {
  return page.evaluate(async () => {
    const reader = (window as unknown as { fvociDev: Dev }).fvociDev.backend;
    const [workspace] = await reader.workspaces();
    for (const board of await reader.boards(workspace?.id ?? '')) {
      const [cards, views] = await Promise.all([reader.cards(board.id), reader.views(board.id)]);
      const [view] = views;
      if (cards.length > 0 && view) {
        return { workspaceId: workspace?.id ?? '', board, view, cards: cards.length };
      }
    }
    throw new Error('no board with cards');
  });
}

/** Puts a board view of the first view of `board` where the caret is, from the slash menu. */
async function putFromSlash(page: Page, board: string, view: string): Promise<void> {
  await slash(page, 'embed', 'Board view');
  const picker = page.getByTestId('board-view-picker');
  await expect(picker).toBeVisible();
  await picker.getByRole('button', { name: board, exact: true }).click();
  await picker.getByRole('button', { name: new RegExp(`^${view}`) }).click();
  await expect(picker).toBeHidden();
}

test('a board view is put in from the slash menu and shows its view, linked to the board', async ({
  page,
}) => {
  const body = await newPage(page);
  const { board, view } = await firstView(page);
  await body.click();
  await putFromSlash(page, board.title, view.title);
  const shown = body.getByTestId('embedded-view');
  await expect(shown).toBeVisible();
  await expect(shown).toContainText(board.title);
  await expect(shown).toContainText(view.title);
  await expect(shown.getByTestId('embedded-view-kind')).toBeVisible();
  await expect(shown.getByTestId('embedded-card').first()).toBeVisible();
  const open = shown.getByRole('link', { name: 'Open in board' });
  await expect(open).toHaveAttribute('href', new RegExp(`/boards/${board.id}/${view.id}$`));

  // A card pressed opens over the page, which stays.
  const first = shown.getByTestId('embedded-card').first();
  // The card's title, without its icon.
  const title = ((await first.locator('span.truncate').textContent()) ?? '').trim();
  await first.click();
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText(title);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId('page-body')).toBeVisible();

  await open.click();
  await expect(page).toHaveURL(new RegExp(`/boards/${board.id}/${view.id}$`));
});

test('a pasted address of a view is a link until it is taken as a board view', async ({ page }) => {
  const body = await newPage(page);
  const { workspaceId, board, view } = await firstView(page);
  const address = new URL(`/w/${workspaceId}/boards/${board.id}/${view.id}`, page.url()).href;
  await body.click();
  await paste(body, { 'text/plain': address });
  await expect(body).toContainText(address);
  const offer = page.locator('[data-suggestions="board-view"]');
  await expect(offer.getByRole('option')).toHaveText(['As a link', 'As an embed']);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(body.getByTestId('embedded-view')).toBeVisible();
  await expect(body).not.toContainText(address);
});

test('on a phone a board view scrolls within itself and the page does not move sideways', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const body = await newPage(page);
  const { board, view } = await firstView(page);
  await body.click();
  await putFromSlash(page, board.title, view.title);
  const scroll = body.getByTestId('embedded-view-scroll');
  await expect(scroll).toBeVisible();
  const sizes = await scroll.evaluate((element) => ({
    height: element.getBoundingClientRect().height,
    screen: window.innerHeight,
    page: document.documentElement.scrollWidth,
    wide: window.innerWidth,
  }));
  expect(sizes.height).toBeLessThanOrEqual(sizes.screen * 0.6 + 1);
  expect(sizes.page).toBeLessThanOrEqual(sizes.wide);
  // A table's cards keep their titles at the start as it scrolls sideways.
  const first = scroll.locator('tbody .embedded-first').first();
  if ((await first.count()) > 0) {
    await expect(first).toHaveCSS('position', 'sticky');
  }
});
