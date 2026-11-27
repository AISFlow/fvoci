/**
 * The search of what cards and pages say, on the backend in memory: its keys
 * and button, finding by a word of a body and going there by the keyboard
 * alone, going to a page of a wiki, the kinds, an earlier answer that comes
 * late, what the dialog says of each state of the search, and a client
 * configuration that says bodies are not searched (`?search=0`).
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { ON_A_CARD, ON_A_VIEW, placeOf } from './address.ts';
import { primaryKey, primaryName } from './keyboard.support.ts';
import { newWiki, pageTree, showTree, wikiPath, wikiScreen } from './wiki.support.ts';
import {
  boardMenu,
  chooseFromMenu,
  nextFrame,
  openApp,
  showSidebar,
  sidebar,
  stageOf,
  strip,
  topBar,
  undoInBar,
} from './shell.support.ts';

/** What the page's development handle offers this test (src/app/backend.ts). Only what is used. */
interface Dev {
  backend: {
    latency: number;
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
    createCard(boardId: string, draft: object): Promise<{ id: string }>;
    card(boardId: string, cardId: string): Promise<unknown>;
    failNext(method: string, error: unknown): void;
    search: { find(workspaceId: string, ask: object): Promise<unknown[]> };
    wiki: { createPage(wikiId: string, draft: object): Promise<{ id: string }> };
  };
}

const dialog = (page: Page): Locator => page.getByTestId('search-dialog');
const field = (page: Page): Locator => page.getByTestId('search-field');
const hits = (page: Page): Locator => dialog(page).getByRole('option');

const paragraph = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** Makes a card on a board of the seed (the first, unless named), with a title and a body. */
async function cardWith(
  page: Page,
  title: string,
  body: string,
  boardTitle = 'Project schedule',
): Promise<string> {
  return page.evaluate(
    async ({ title, body, boardTitle }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const [workspace] = await backend.workspaces();
      const boards = await backend.boards(workspace?.id ?? '');
      const board = boards.find((each) => each.title.startsWith(boardTitle));
      if (!board) throw new Error(`no board ${boardTitle}`);
      return (await backend.createCard(board.id, { title, body })).id;
    },
    { title, body: paragraph(body), boardTitle },
  );
}

/** Makes pages at the top of a wiki, one after another, with these titles. */
async function pagesIn(page: Page, wikiId: string, titles: string[]): Promise<string[]> {
  return page.evaluate(
    async ({ wikiId, titles }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const made: string[] = [];
      for (const title of titles) made.push((await backend.wiki.createPage(wikiId, { title })).id);
      return made;
    },
    { wikiId, titles },
  );
}

/** Opens the search by its keys, finds the one card the words find, and goes to it. */
async function goTo(page: Page, words: string, title: string): Promise<void> {
  await page.keyboard.press(await primaryKey(page, 'k'));
  await search(page, words);
  await expect(dialog(page)).toHaveAttribute('data-status', 'found');
  await expect(hits(page).locator('[data-hit-title]')).toHaveText([title]);
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByTestId('card-dialog')).toContainText(title);
}

/** Makes the next search refused with this status (and `Retry-After`, for 429). */
async function refuseNext(page: Page, status: number, extra: object = {}): Promise<void> {
  await page.evaluate(
    async ({ status, extra }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const refusal = await backend.card('nowhere', 'nothing').catch((error: unknown) => error);
      const BackendError = (
        refusal as { constructor: new (status: number, body: object) => unknown }
      ).constructor;
      backend.failNext('search', new BackendError(status, { detail: 'refused', ...extra }));
    },
    { status, extra },
  );
}

/** Opens the app, once its search is known to be there (the button shows it). */
async function openWithSearch(page: Page): Promise<void> {
  await openApp(page);
  await expect(topBar(page).getByTestId('open-search')).toBeVisible();
}

async function search(page: Page, words: string): Promise<void> {
  await field(page).fill(words);
}

test.describe('the search of cards and pages', () => {
  test('the platform’s ⌘K or Ctrl+K opens it, Escape closes it and the focus goes back', async ({
    page,
  }) => {
    await openWithSearch(page);
    const open = topBar(page).getByTestId('open-search');
    await expect(open).toBeVisible();
    const modifier = await primaryName(page);
    await expect(open).toHaveAttribute('aria-keyshortcuts', `${modifier}+K`);
    // The other platform's modifier is no alias: on a Mac, Ctrl+K in a field deletes to the
    // end of the line. Pressed first, it leaves the search shut.
    await page.getByTestId('board-page').focus();
    await page.keyboard.press(`${modifier === 'Meta' ? 'Control' : 'Meta'}+k`);
    // A search the keys open is drawn by the next frame.
    await nextFrame(page);
    await expect(dialog(page)).toHaveCount(0);
    await page.keyboard.press(await primaryKey(page, 'k'));
    await expect(dialog(page)).toBeVisible();
    await expect(field(page)).toBeFocused();
    await expect(page.getByTestId('search-hint')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.getByTestId('board-page')).toBeFocused();
    // The button opens it too, and has the focus back.
    await open.click();
    await expect(field(page)).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(open).toBeFocused();
  });

  test('offers the boards to go to at once, then those whose title holds what is typed', async ({
    page,
  }) => {
    await openWithSearch(page);
    const open = topBar(page).getByTestId('open-search');
    await open.click();
    const jumps = dialog(page).getByTestId('search-jumps').locator('[data-search-jump]');
    await expect(jumps.first()).toBeVisible();
    await search(page, 'STUDY');
    await expect(jumps).toHaveCount(1);
    await expect(jumps).toContainText('Study notes');
    // They are links beside the list of what was found, not options of it.
    await expect(hits(page).filter({ hasText: 'Study notes' })).toHaveCount(0);
    await jumps.click();
    await expect(dialog(page)).toHaveCount(0);
    await expect(page).toHaveURL(ON_A_VIEW);
    await expect(topBar(page)).toContainText('Study notes');
    // Gone elsewhere: the focus is the new page's, not back on the button that opened the search.
    await expect(page.locator('main')).toBeFocused();
    await expect(open).not.toBeFocused();
  });

  test('finds a card by a word of its body and opens it, by the keyboard alone', async ({
    page,
  }) => {
    await openWithSearch(page);
    const cardId = await cardWith(page, 'Quarterly review', 'The zephyrquokka migration is due');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'zephyrquokka');
    await expect(dialog(page)).toHaveAttribute('data-status', 'found');
    await expect(hits(page)).toHaveCount(1);
    const hit = hits(page).first();
    await expect(hit).toHaveAttribute('aria-selected', 'true');
    await expect(hit.locator('[data-hit-title]')).toHaveText('Quarterly review');
    await expect(hit.locator('[data-hit-container]')).toHaveText(/Project schedule/);
    await expect(hit.locator('mark')).toHaveText('zephyrquokka');
    await expect(field(page)).toHaveAttribute(
      'aria-activedescendant',
      (await hit.getAttribute('id')) ?? '',
    );
    await expect(page.getByTestId('search-said')).toHaveText('Found: 1');
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.getByTestId('card-dialog')).toBeVisible();
    await expect(page).toHaveURL(ON_A_CARD);
    expect(page.url()).toContain(cardId);
  });

  test('the arrows choose among what was found, around the ends', async ({ page }) => {
    await openWithSearch(page);
    await cardWith(page, 'First cassowary', 'one');
    await cardWith(page, 'Second cassowary', 'two');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'cassowary');
    await expect(hits(page)).toHaveCount(2);
    const selected = dialog(page).locator('[role="option"][aria-selected="true"]');
    const first = await selected.textContent();
    await page.keyboard.press('ArrowDown');
    await expect(selected).not.toHaveText(first ?? '');
    await page.keyboard.press('ArrowDown');
    await expect(selected).toHaveText(first ?? '');
    await page.keyboard.press('ArrowUp');
    await expect(selected).not.toHaveText(first ?? '');
  });

  test('finds one kind at a time', async ({ page }) => {
    await openWithSearch(page);
    await cardWith(page, 'Platypus plan', 'a body');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'platypus');
    await expect(hits(page)).toHaveCount(1);
    // Diaries are the own space's: their entries are no kind to find here.
    await expect(dialog(page).getByRole('radio', { name: 'Diary' })).toHaveCount(0);
    await dialog(page).getByRole('radio', { name: 'Pages' }).click();
    await expect(dialog(page)).toHaveAttribute('data-status', 'none');
    await expect(page.getByTestId('search-said')).toHaveText('Nothing found.');
    await dialog(page).getByRole('radio', { name: 'Cards' }).click();
    await expect(hits(page)).toHaveCount(1);
    await expect(hits(page).first()).toHaveAttribute('data-kind', 'card');
  });

  test('an earlier answer that comes late does not take the place of the last one', async ({
    page,
  }) => {
    await openWithSearch(page);
    await cardWith(page, 'Ocelot one', 'first');
    await cardWith(page, 'Ocelot two', 'second');
    // Each search waits until the test lets it answer, in the order the test chooses.
    await page.evaluate(() => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const held: (() => Promise<void>)[] = [];
      const find = backend.search.find.bind(backend.search);
      backend.search.find = (workspaceId, ask) =>
        new Promise((resolve, reject) => {
          held.push(() => find(workspaceId, ask).then(resolve, reject));
        });
      Object.assign(window, { heldSearches: held });
    });
    const held = (): Promise<number> =>
      page.evaluate(() => (window as unknown as { heldSearches: unknown[] }).heldSearches.length);
    const answer = (index: number): Promise<void> =>
      page.evaluate(async (at) => {
        await (window as unknown as { heldSearches: (() => Promise<void>)[] }).heldSearches[at]?.();
      }, index);
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'ocelot');
    await expect.poll(held).toBe(1);
    await search(page, 'ocelot two');
    await expect.poll(held).toBe(2);
    await answer(1);
    await expect(hits(page)).toHaveCount(1);
    await expect(hits(page).locator('[data-hit-title]')).toHaveText('Ocelot two');
    // The first answer arrives, and is dropped.
    await answer(0);
    await expect(hits(page)).toHaveCount(1);
    await expect(hits(page).locator('[data-hit-title]')).toHaveText('Ocelot two');
  });

  test('a card gone to from another card takes its place: closing it shows the board', async ({
    page,
  }) => {
    await openWithSearch(page);
    await cardWith(page, 'Heron first', 'egret one');
    await cardWith(page, 'Heron second', 'egret two');
    await goTo(page, 'egret one', 'Heron first');
    await goTo(page, 'egret two', 'Heron second');
    // The card shown in place of the other has the focus: Escape reaches it.
    await expect(page.getByTestId('card-dialog')).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('card-dialog')).toHaveCount(0);
    await expect(page).toHaveURL(ON_A_VIEW);
    // The page before the first card is still a step back: no card is left in between.
    await page.goBack();
    await expect(page.getByTestId('card-dialog')).toHaveCount(0);
  });

  test('a card of another board opens over that board, and closing it leaves the board', async ({
    page,
  }) => {
    await openWithSearch(page);
    await cardWith(page, 'Lemur here', 'tamarin near');
    const there = await cardWith(page, 'Lemur there', 'tamarin far', 'Issue tracker');
    await goTo(page, 'tamarin near', 'Lemur here');
    const before = placeOf(page.url());
    await goTo(page, 'tamarin far', 'Lemur there');
    await expect(page).toHaveURL(ON_A_CARD);
    const shown = placeOf(page.url());
    expect(shown.cardId).toBe(there);
    expect(shown.boardId).not.toBe(before.boardId);
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('card-dialog')).toHaveCount(0);
    await expect(page).toHaveURL(ON_A_VIEW);
    expect(placeOf(page.url()).boardId).toBe(shown.boardId);
  });

  test('a page opens over its wiki, another of it in its place, a card from it and back, and Back goes where each was', async ({
    page,
  }) => {
    await openWithSearch(page);
    const board = placeOf(page.url()).boardId;
    const wikiId = await newWiki(page, 'Handbook');
    // Not the wiki's first page: going over the wiki would show that one by itself.
    const [keeping, harbour] = await pagesIn(page, wikiId, ['Lighthouse keeping', 'Harbour log']);
    const card = await cardWith(page, 'Keeper notes', 'the beacon is lit');
    await page.goBack();
    await expect(page).toHaveURL(ON_A_VIEW);
    const title = wikiScreen(page).getByTestId('page-title');
    const onPage = (pageId = ''): RegExp => new RegExp(`${wikiPath(wikiId, pageId)}$`);

    // From a board, over the wiki.
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'harbour');
    await expect(dialog(page)).toHaveAttribute('data-status', 'found');
    await expect(hits(page)).toHaveCount(1);
    await expect(hits(page).first()).toHaveAttribute('data-kind', 'page');
    await expect(hits(page).first().locator('[data-hit-container]')).toHaveText('Handbook');
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toHaveCount(0);
    await expect(page).toHaveURL(onPage(harbour));
    await expect(title).toHaveValue('Harbour log');
    // Not left on nothing: Tab goes on from the page.
    await expect(page.locator('main')).toBeFocused();

    // Another page of the wiki shown, as from its tree.
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'lighthouse');
    await expect(hits(page).locator('[data-hit-title]')).toHaveText(['Lighthouse keeping']);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(onPage(keeping));
    await expect(title).toHaveValue('Lighthouse keeping');
    await expect(page.locator('main')).toBeFocused();

    // A card from a page, and a page from the card.
    await goTo(page, 'beacon', 'Keeper notes');
    expect(placeOf(page.url())).toMatchObject({ boardId: board, cardId: card });
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'harbour');
    await expect(hits(page).locator('[data-hit-title]')).toHaveText(['Harbour log']);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(onPage(harbour));
    await expect(title).toHaveValue('Harbour log');

    // Back, one place at a time.
    await page.goBack();
    await expect(page).toHaveURL(ON_A_CARD);
    await expect(page.getByTestId('card-dialog')).toContainText('Keeper notes');
    await page.goBack();
    await expect(page).toHaveURL(onPage(keeping));
    await expect(title).toHaveValue('Lighthouse keeping');
    await page.goBack();
    await expect(page).toHaveURL(onPage(harbour));
    await expect(title).toHaveValue('Harbour log');
    await page.goBack();
    await expect(page).toHaveURL(ON_A_VIEW);
    expect(placeOf(page.url()).boardId).toBe(board);
  });

  test('390 pixels wide, the search opens nothing over the drawer of the pages', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await openWithSearch(page);
    const wikiId = await newWiki(page, 'Handbook');
    const [pier] = await pagesIn(page, wikiId, ['Pier']);
    await topBar(page).getByTestId('open-search').click();
    await search(page, 'pier');
    await expect(hits(page).locator('[data-hit-title]')).toHaveText(['Pier']);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`${wikiPath(wikiId, pier)}$`));
    await showTree(page);
    // The drawer keeps the top bar away, its button with it, and the keys open no search.
    await expect(topBar(page)).toHaveAttribute('inert', '');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await expect(dialog(page)).toHaveCount(0);
    await expect(pageTree(page)).toBeVisible();
    await page.goBack();
    await expect(pageTree(page)).toBeHidden();
    await expect(page).toHaveURL(new RegExp(`${wikiPath(wikiId, pier)}$`));
  });

  test('the keys open nothing over a drawer or another dialog', async ({ page }) => {
    await openWithSearch(page);
    if (await strip(page).isVisible()) {
      await strip(page).getByTestId('strip-open-settings').click();
    } else {
      await showSidebar(page);
      await sidebar(page).getByTestId('open-settings').click();
    }
    await expect(page.getByTestId('settings-dialog')).toBeVisible();
    await page.keyboard.press(await primaryKey(page, 'k'));
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.getByTestId('settings-dialog')).toBeVisible();
  });

  test('Enter while newer words are searched for goes to what they find, not to the older list', async ({
    page,
  }) => {
    await openWithSearch(page);
    await cardWith(page, 'Gecko older', 'iguana');
    await cardWith(page, 'Gecko newer', 'iguana basilisk');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'iguana');
    await expect(hits(page)).toHaveCount(2);
    // The next search answers when the test lets it.
    await page.evaluate(() => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const find = backend.search.find.bind(backend.search);
      backend.search.find = (workspaceId, ask) =>
        new Promise((resolve, reject) => {
          Object.assign(window, {
            letSearchAnswer: () => find(workspaceId, ask).then(resolve, reject),
          });
        });
    });
    await search(page, 'iguana basilisk');
    await expect(dialog(page)).toHaveAttribute('data-status', 'searching');
    await expect.poll(() => page.evaluate(() => 'letSearchAnswer' in window)).toBe(true);
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toBeVisible();
    await expect(page.getByTestId('card-dialog')).toHaveCount(0);
    await page.evaluate(async () => {
      await (window as unknown as { letSearchAnswer: () => Promise<void> }).letSearchAnswer();
    });
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.getByTestId('card-dialog')).toContainText('Gecko newer');
  });

  test('the keys open nothing over a confirmation', async ({ page }) => {
    await openWithSearch(page);
    test.skip((await stageOf(page)) !== 'expanded', 'a sidebar beside the board, no drawer');
    await chooseFromMenu(page, boardMenu(page, 'Meeting notes'), 'delete');
    await expect(page.getByTestId('confirm-dialog')).toBeVisible();
    await page.keyboard.press(await primaryKey(page, 'k'));
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.getByTestId('confirm-dialog')).toBeVisible();
  });

  test('what a body says is shown as text, the matched words marked', async ({ page }) => {
    await openWithSearch(page);
    await cardWith(page, 'Markup', 'before <script>alert(1)</script> wombat <b>after</b>');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'wombat');
    const snippet = hits(page).first().locator('[data-hit-snippet]');
    await expect(snippet).toHaveText('before <script>alert(1)</script> wombat <b>after</b>');
    await expect(snippet.locator('script, b')).toHaveCount(0);
    await expect(snippet.locator('mark')).toHaveText('wombat');
  });

  test('each state of the search says what it is', async ({ page }) => {
    await openWithSearch(page);
    await cardWith(page, 'Narwhal notes', 'body');
    await page.keyboard.press(await primaryKey(page, 'k'));
    // Not available: said, with a way to try again.
    await refuseNext(page, 503, { reason: 'search_unavailable' });
    await search(page, 'narwhal');
    await expect(dialog(page)).toHaveAttribute('data-status', 'unavailable');
    await expect(dialog(page)).toContainText('Search is not available right now.');
    await page.getByTestId('search-retry').click();
    await expect(hits(page)).toHaveCount(1);
    // Too many: said, and asked again by itself after the time given.
    await refuseNext(page, 429, { retryAfter: 1 });
    await search(page, 'narwhal notes');
    await expect(dialog(page)).toHaveAttribute('data-status', 'limited');
    await expect(dialog(page)).toContainText('Searching again in a moment');
    await expect(dialog(page)).toHaveAttribute('data-status', 'found');
    // Nothing: said.
    await search(page, 'nothing-has-this-word');
    await expect(dialog(page)).toHaveAttribute('data-status', 'none');
    // The field takes no more than the server does.
    await expect(field(page)).toHaveAttribute('maxlength', '200');
  });

  test('where bodies are not searched there is no button and the keys open nothing', async ({
    page,
  }) => {
    await openApp(page, '/?search=0');
    await expect(undoInBar(page)).toBeVisible();
    await expect(topBar(page).getByTestId('open-search')).toHaveCount(0);
    await page.keyboard.press(await primaryKey(page, 'k'));
    await page.keyboard.press(`${(await primaryName(page)) === 'Meta' ? 'Control' : 'Meta'}+k`);
    await nextFrame(page);
    await expect(dialog(page)).toHaveCount(0);
  });

  test('390 pixels wide, the button is there and the dialog fills the screen', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await openWithSearch(page);
    // The button takes its room from the members' word, not from the title, and fits.
    await expect(topBar(page).getByTestId('board-members-count')).toHaveText(/^\d+$/, {
      useInnerText: true,
    });
    const title = await topBar(page).getByRole('heading', { level: 1 }).boundingBox();
    expect(title?.width).toBeGreaterThan(0);
    const rights = await topBar(page)
      .getByRole('button')
      .evaluateAll((buttons) => buttons.map((button) => button.getBoundingClientRect().right));
    expect(rights.length).toBeGreaterThan(4);
    expect(Math.max(...rights)).toBeLessThanOrEqual(390);
    await cardWith(page, 'Axolotl', 'small screens');
    await topBar(page).getByTestId('open-search').click();
    await expect(dialog(page)).toHaveAttribute('data-compact', 'true');
    // Once it has opened: it zooms in from a little smaller.
    await expect.poll(async () => (await dialog(page).boundingBox())?.width).toBe(390);
    expect((await dialog(page).boundingBox())?.x).toBe(0);
    await search(page, 'axolotl');
    await hits(page).first().click();
    await expect(page.getByTestId('card-dialog')).toBeVisible();
    await expect(dialog(page)).toHaveCount(0);
  });

  test('a phone held sideways, too low for the dialog, is filled by it from edge to edge', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 844, height: 390 });
    await openWithSearch(page);
    await page.keyboard.press(await primaryKey(page, 'k'));
    await expect(dialog(page)).toHaveAttribute('data-compact', 'true');
    await expect.poll(async () => (await dialog(page).boundingBox())?.width).toBe(844);
    expect((await dialog(page).boundingBox())?.x).toBe(0);
  });

  test('a hit opened in a new tab leaves the search open, the focus in its field', async ({
    page,
  }) => {
    await openWithSearch(page);
    await cardWith(page, 'Ibis', 'a tab of its own');
    await page.keyboard.press(await primaryKey(page, 'k'));
    await search(page, 'ibis');
    await expect(hits(page)).toHaveCount(1);
    // The browser's click: a new tab (in the background, of no interest here), the search left open.
    await hits(page)
      .first()
      .click({ modifiers: ['ControlOrMeta'] });
    await expect(dialog(page)).toBeVisible();
    await expect(field(page)).toBeFocused();
    await expect(hits(page).first()).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('card-dialog')).toBeVisible();
  });
});
