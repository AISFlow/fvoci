/**
 * References between a card and a page of a wiki, end to end, on the
 * in-memory backend: a page's body names a card with `[[` and a card's body
 * names the page, each chip opens what it names, and each names what names it.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { documentBoundary } from './keyboard.support.ts';
import { openApp } from './shell.support.ts';
import {
  goInside,
  nameThePage,
  newWiki,
  ON_A_PAGE,
  pageBody,
  wikiPlaceOf,
} from './wiki.support.ts';

interface StoredCard {
  id: string;
  title: string;
  fields: { isTemplate?: boolean };
}
interface Dev {
  backend: {
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
    cards(boardId: string): Promise<StoredCard[]>;
  };
}

/** A board of the world and two of its cards whose titles are their own. */
async function cardsOfTheWorld(
  page: Page,
): Promise<{ boardId: string; source: StoredCard; target: StoredCard }> {
  await openApp(page);
  return page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title === 'Issue tracker');
    if (!board) throw new Error('no board');
    const cards = await backend.cards(board.id);
    // Pick two cards that have unique titles and no seeded references pointing to or from them.
    const byTitle = (title: string): StoredCard | undefined =>
      cards.find((card) => !card.fields.isTemplate && card.title === title);
    const source = byTitle('The exported total differs from the screen');
    const target = byTitle('Search misses a word with an ending attached');
    if (!source || !target) throw new Error('too few cards');
    return { boardId: board.id, source, target };
  });
}

const offered = (page: Page): Locator => page.locator('[data-suggestions="reference"]');

/** Puts a reference at the end of `body` to what `[[` finds for `title`. */
async function name(page: Page, body: Locator, title: string): Promise<void> {
  await body.click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(` [[${title.toLowerCase()}`);
  const option = offered(page).getByRole('option', { name: new RegExp(title) });
  await expect(option).toHaveCount(1);
  await expect(option).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
  await expect(offered(page)).toHaveCount(0);
}

test('a page names a card and a card names the page, each opens the other, and each lists what names it', async ({
  page,
}) => {
  const { boardId, source, target } = await cardsOfTheWorld(page);
  await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await expect(page).toHaveURL(ON_A_PAGE);
  await nameThePage(page, 'Road plan');
  const { pageId } = wikiPlaceOf(page.url());

  // The page names the card, which the chip opens.
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  await name(page, pageBody(page), target.title);
  const toCard = page.getByTestId('page-body').locator('[data-reference]').last();
  await expect(toCard).toHaveAttribute('data-state', 'shown');
  await expect(toCard).toContainText(target.title);
  await toCard.getByRole('link').click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-title')).toHaveValue(target.title);
  // The card lists the page, which leads back to it.
  const naming = page.getByTestId('card-backlinks');
  await expect(naming.getByRole('link')).toHaveText(['Road plan']);
  await naming.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/wikis/[0-9a-f-]{36}/${pageId}$`));
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  // Nothing names the page yet: no such part (what names it is asked as the page opens,
  // and the end of this test sees the same list appear once a card names the page).
  await expect(page.getByTestId('card-backlinks')).toHaveCount(0);

  // A card names the page: a chip with its title, which opens it.
  await goInside(page, boardPath(boardId, '-', source.id));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  const cardBody = page.getByTestId('card-body').locator('[contenteditable]').first();
  await name(page, cardBody, 'Road plan');
  const toPage = page.getByTestId('card-body').locator('[data-reference]').last();
  await expect(toPage).toHaveAttribute('data-kind', 'page');
  await expect(toPage).toContainText('Road plan');
  await toPage.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/wikis/[0-9a-f-]{36}/${pageId}$`));
  // The page lists the card that names it.
  const pageNaming = page.getByTestId('card-backlinks');
  await expect(pageNaming).toContainText('What links to this page');
  await expect(pageNaming.getByRole('link')).toHaveText([new RegExp(source.title)]);
});
