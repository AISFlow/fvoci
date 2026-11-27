/**
 * The numbers of cards, end to end, on the in-memory backend: every view
 * shows a card's number and says it, an open card shows it at its head, `#`
 * and a number in a body put a reference to that card of the board (a chip;
 * the body holds the card's identifier, never its number), and both searches
 * find a card by its number. With `FVOCI_SCREENS=1` it takes the pictures
 * `card-numbers-*`, into target/screens/card-numbers/.
 */
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';

import { documentBoundary, primaryKey } from './keyboard.support.ts';
import { boardPage, dialog, openView } from './kanban.support.ts';
import { screensDir } from './screens.ts';
import { newWiki } from './wiki.support.ts';

const WELCOME = 'Getting started with fvoci';

interface DevCard {
  id: string;
  title: string;
  number?: number;
  body?: unknown;
}
interface Dev {
  workspace: { board: { cards: DevCard[] } };
  backend: { card(boardId: string, cardId: string): Promise<DevCard> };
}

/** The cards of the open board as its store holds them: their identifiers, titles and numbers. */
function cardsOfBoard(page: Page): Promise<DevCard[]> {
  return page.evaluate(() =>
    (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board.cards.map(
      ({ id, title, number }) => ({ id, title, ...(number === undefined ? {} : { number }) }),
    ),
  );
}

/** The card of the open board with the title. */
async function cardTitled(page: Page, title: string): Promise<Required<Omit<DevCard, 'body'>>> {
  const found = (await cardsOfBoard(page)).find((one) => one.title === title);
  if (found?.number === undefined) throw new Error(`no card "${title}" with a number`);
  return { id: found.id, title: found.title, number: found.number };
}

const numberIn = (place: Locator): Locator => place.locator('[data-slot="card-number"]');

/**
 * Every card drawn shows its own number, as `#N`, and no two show one; where
 * every card of the board is drawn (`whole`), they are 1 to the number of cards.
 */
async function everyCardNumbered(page: Page, cards: Locator, whole: boolean): Promise<void> {
  const numbers = new Map((await cardsOfBoard(page)).map((one) => [one.id, one.number]));
  await expect(cards.first()).toBeVisible();
  const drawn = await cards.evaluateAll((elements) =>
    elements.map((element) => ({
      id: element.getAttribute('data-card') ?? '',
      shown: element.querySelector('[data-slot="card-number"] [aria-hidden="true"]')?.textContent,
    })),
  );
  expect(drawn.length).toBeGreaterThan(0);
  for (const { id, shown } of drawn) expect(shown, id).toBe(`#${String(numbers.get(id))}`);
  const shown = drawn.map((one) => Number(one.shown?.slice(1))).sort((a, b) => a - b);
  expect(new Set(shown).size).toBe(drawn.length);
  if (whole) expect(shown).toEqual(Array.from({ length: numbers.size }, (_, at) => at + 1));
}

const bodyOf = (page: Page): Locator => page.getByTestId('card-body');
const offered = (page: Page): Locator => page.locator('[data-suggestions="card-number"]');

/** Opens a card of the kanban by its title, and waits for its body's editor. */
async function openCard(page: Page, title: string): Promise<Locator> {
  await page.locator('[data-card]').filter({ hasText: title }).first().click();
  await expect(dialog(page)).toBeVisible();
  await expect(bodyOf(page)).toHaveAttribute('data-editor', 'ready');
  return bodyOf(page).locator('[contenteditable]').first();
}

async function openSearchOfView(page: Page): Promise<Locator> {
  const header = page.getByTestId('view-header');
  await header.locator('[data-tool="search"]').click();
  return header.getByRole('searchbox', { name: 'Search cards' });
}

test('every view shows a card’s number and says it, and an open card shows it at its head', async ({
  page,
}, info) => {
  await openView(page, info, WELCOME, 'Board');
  // A phone shows one column of the kanban at a time.
  await everyCardNumbered(page, boardPage(page).locator('[data-card]'), false);
  const first = boardPage(page).locator('[data-card]').first();
  const id = (await first.getAttribute('data-card')) ?? '';
  const named = (await cardsOfBoard(page)).find((one) => one.id === id);
  if (named?.number === undefined) throw new Error('the first card has no number');
  // Said in words with the title: the face names the card.
  await expect(first).toHaveAccessibleName(
    new RegExp(`^number ${String(named.number)}\\s+.*${named.title}`),
  );

  await first.click();
  await expect(dialog(page)).toBeVisible();
  await expect(numberIn(page.getByTestId('card-location'))).toHaveAttribute(
    'data-number',
    String(named.number),
  );
  await expect(page.getByTestId('card-location')).toContainText(`#${String(named.number)}`);

  await openView(page, info, WELCOME, 'Table');
  await everyCardNumbered(page, page.locator('[role="row"][data-card]'), true);
});

test('a card of the calendar and of the chart shows its number, and its bar says it', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'the views are drawn the same at every width');
  await page.clock.setFixedTime(new Date('2026-11-09T16:00:00Z'));
  await openView(page, info, WELCOME, 'Calendar');
  const { id, number } = await cardTitled(page, 'Open a card');
  await page.evaluate(async (cardId) => {
    interface Store {
      board: { cardProperties: { id: string; name: string }[] };
      setCardProperty(cardId: string, propertyId: string, value: unknown): Promise<void>;
    }
    const store = (window as unknown as { fvociDev: { workspace: { board: Store } } }).fvociDev
      .workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Day');
    if (!due) throw new Error('no Day');
    await store.setCardProperty(cardId, due.id, { from: '2026-11-12' });
  }, id);
  const bar = page.getByTestId('month-grid').locator(`a[data-bar="${id}"]`);
  await expect(numberIn(bar)).toHaveText(new RegExp(`#${String(number)}`));
  await expect(bar).toHaveAccessibleName(new RegExp(`^number ${String(number)}, Open a card`));

  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add Gantt view' }).click();
  const row = page.getByTestId('gantt').locator(`[data-row="${id}"]`);
  await expect(numberIn(row)).toHaveText(new RegExp(`#${String(number)}`));
  await expect(row.locator(`[data-bar="${id}"]`)).toHaveAccessibleName(
    new RegExp(`^number ${String(number)}, Open a card`),
  );
});

test('`#` and a number in a body put that card of the board as a chip, and the body holds its identifier', async ({
  page,
}, info) => {
  await openView(page, info, WELCOME, 'Board');
  const target = await cardTitled(page, 'Filter, sort and group');
  const source = await cardTitled(page, 'Describe cards with properties');
  const editor = await openCard(page, source.title);
  await editor.click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(` #${String(target.number)}`);
  const option = offered(page).getByRole('option').first();
  // Its number, its icon and its title.
  await expect(option).toHaveText(new RegExp(`^#${String(target.number)} .*${target.title}$`));
  await expect(option).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
  await expect(offered(page)).toHaveCount(0);

  const chip = bodyOf(page).locator('[data-reference]').last();
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveText(new RegExp(`${target.title}$`));
  await expect(editor).not.toContainText(`#${String(target.number)}`);
  const stored = async (): Promise<string> =>
    page.evaluate(
      async ({ cardId }) => {
        const { fvociDev } = window as unknown as {
          fvociDev: Dev & { workspace: { board: { boardId: string } } };
        };
        const read = await fvociDev.backend.card(fvociDev.workspace.board.boardId, cardId);
        return JSON.stringify(read.body ?? null);
      },
      { cardId: source.id },
    );
  await expect.poll(stored).toContain(`{"type":"reference","attrs":{"id":"${target.id}"}}`);
  expect(await stored()).not.toContain(`#${String(target.number)}`);

  // The words of a title after `#` offer the cards of the board that have them.
  await page.keyboard.type(' #wiki');
  const wiki = await cardTitled(page, 'Keep your documents in a wiki');
  await expect(offered(page).getByRole('option')).toHaveText([
    new RegExp(`^#${String(wiki.number)} .*Keep your documents in a wiki$`),
  ]);
  await page.keyboard.press('Escape');
});

test('the search of the workspace and the search of a view find a card by its number', async ({
  page,
}, info) => {
  await openView(page, info, WELCOME, 'Board');
  const target = await cardTitled(page, 'Filter, sort and group');
  await expect(page.getByTestId('open-search')).toBeVisible();
  await page.keyboard.press(await primaryKey(page, 'k'));
  const search = page.getByTestId('search-dialog');
  await page.getByTestId('search-field').fill(`#${String(target.number)}`);
  await expect(search).toHaveAttribute('data-status', 'found');
  const first = search.getByRole('option').first();
  await expect(first).toHaveAttribute('data-search-hit', target.id);
  await expect(first.locator('[data-hit-title]')).toHaveText(target.title);
  await expect(numberIn(first)).toContainText(`#${String(target.number)}`);
  await page.keyboard.press('Enter');
  await expect(search).toHaveCount(0);
  await expect(page.getByTestId('card-title')).toHaveValue(target.title);
  await page.getByTestId(info.project.name === 'mobile' ? 'card-back' : 'card-close').click();
  await expect(dialog(page)).toHaveCount(0);

  const field = await openSearchOfView(page);
  await field.fill(`#${String(target.number)}`);
  await expect(boardPage(page).locator('[data-card]')).toHaveCount(1);
  await expect(boardPage(page).locator(`[data-card="${target.id}"]`)).toBeVisible();
});

test('`#N` finds matching cards across boards with the current board first, and works from a wiki', async ({
  page,
}, info) => {
  await openView(page, info, WELCOME, 'Board');
  const welcomeFirst = (await cardsOfBoard(page)).find((one) => one.number === 1);
  if (!welcomeFirst) throw new Error('no #1 card on welcome board');

  const other = await page.evaluate(async () => {
    interface DevExtra {
      workspace: { board: { boardId: string } };
      backend: {
        workspaces(): Promise<{ id: string }[]>;
        boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
        cards(boardId: string): Promise<{ id: string; title: string; number?: number }[]>;
      };
    }
    const { workspace, backend } = (window as unknown as { fvociDev: DevExtra }).fvociDev;
    const [ws] = await backend.workspaces();
    const boards = await backend.boards(ws?.id ?? '');
    for (const board of boards) {
      if (board.id === workspace.board.boardId) continue;
      const cards = await backend.cards(board.id);
      const first = cards.find((card) => card.number === 1);
      if (first) return { boardTitle: board.title, cardId: first.id, cardTitle: first.title };
    }
    throw new Error('no other board with a #1 card');
  });

  await expect(page.getByTestId('open-search')).toBeVisible();
  await page.keyboard.press(await primaryKey(page, 'k'));
  const search = page.getByTestId('search-dialog');
  await page.getByTestId('search-field').fill('#1');
  await expect(search).toHaveAttribute('data-status', 'found');
  const options = search.getByRole('option');
  await expect(options.first()).toHaveAttribute('data-search-hit', welcomeFirst.id);
  await expect(options.first().locator('[data-hit-container]')).toHaveText(WELCOME);
  await expect(numberIn(options.first())).toContainText('#1');
  const otherOption = search.locator(`[data-search-hit="${other.cardId}"]`);
  await expect(otherOption).toBeVisible();
  await expect(otherOption.locator('[data-hit-container]')).toHaveText(other.boardTitle);
  await expect(numberIn(otherOption)).toContainText('#1');
  await page.keyboard.press('Escape');
  await expect(search).toHaveCount(0);

  // Off any board (on a wiki screen), `#0001` still finds matching cards and navigates directly.
  await newWiki(page, 'Cross-board notes');
  await page.keyboard.press(await primaryKey(page, 'k'));
  await page.getByTestId('search-field').fill('#0001');
  await expect(search).toHaveAttribute('data-status', 'found');
  const welcomeFromWiki = search.locator(`[data-search-hit="${welcomeFirst.id}"]`);
  await expect(welcomeFromWiki).toBeVisible();
  await expect(welcomeFromWiki.locator('[data-hit-container]')).toHaveText(WELCOME);
  await expect(numberIn(welcomeFromWiki)).toContainText('#1');
  await welcomeFromWiki.click();
  await expect(search).toHaveCount(0);
  await expect(page.getByTestId('card-title')).toHaveValue(welcomeFirst.title);
  await expect(page.getByTestId('card-location')).toContainText('#1');
});

test.describe('screens', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');

  /** A wide window, and a phone 390 pixels wide. */
  const SIZES = [
    { name: 'wide', width: 1440, height: 900 },
    { name: '390', width: 390, height: 844 },
  ] as const;

  async function picture(page: Page, info: TestInfo, name: string): Promise<void> {
    await page.screenshot({ path: `${screensDir('card-numbers')}/card-numbers-${name}.png` });
    expect(info.errors).toEqual([]);
  }

  for (const size of SIZES) {
    for (const scheme of ['light', 'dark'] as const) {
      test(`the board, a card and \`#\`, ${size.name}, ${scheme}`, async ({ page }, info) => {
        test.skip(info.project.name !== 'desktop', 'one set of pictures, at the sizes given');
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        /** A view at the size of the pictures: `openView` fits the board to the project's. */
        const show = async (view: string): Promise<void> => {
          await openView(page, info, WELCOME, view);
          await page.setViewportSize({ width: size.width, height: size.height });
        };
        await show('Board');
        await expect(numberIn(boardPage(page)).first()).toBeVisible();
        await picture(page, info, `board-${size.name}-${scheme}`);

        await show('Table');
        await expect(numberIn(page.locator('[role="row"][data-card]')).first()).toBeVisible();
        await picture(page, info, `table-${size.name}-${scheme}`);

        await show('Board');
        const editor = await openCard(page, 'Describe cards with properties');
        await editor.click();
        await documentBoundary(page, 'end');
        await page.keyboard.type(' #1');
        await expect(offered(page).getByRole('option').first()).toBeVisible();
        await picture(page, info, `card-${size.name}-${scheme}`);
      });
    }
  }
});
