/**
 * References in a body, end to end, on the in-memory backend: put with `[[`
 * and by pasting an address, drawn as a chip that opens what it names,
 * listed on the card that is named, and never a title for what the reader
 * may not see. Every test runs at the three stages.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { documentBoundary } from './keyboard.support.ts';

interface StoredCard {
  id: string;
  title: string;
  updatedAt: number;
  body?: unknown;
  fields: { isTemplate?: boolean };
}

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<StoredCard[]>;
  card(boardId: string, cardId: string): Promise<StoredCard>;
  patchCard(boardId: string, cardId: string, patch: object): Promise<unknown>;
  writeCardBody(boardId: string, cardId: string, write: object): Promise<unknown>;
  createComment(boardId: string, cardId: string, body: object): Promise<unknown>;
  batchDeleteCards(boardId: string, cardIds: string[]): Promise<void>;
}
interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

interface Place {
  boardId: string;
  viewId: string;
  /** The card that names, and two cards of the same board to be named. */
  source: StoredCard;
  target: StoredCard;
  spare: StoredCard;
}

const UNKNOWN = '01930000-0000-7000-8000-00000000dead';

const naming = (...ids: string[]): object => ({
  type: 'doc',
  content: [
    {
      type: 'paragraph',
      content: [
        { type: 'text', text: 'See ' },
        ...ids.map((id) => ({ type: 'reference', attrs: { id } })),
      ],
    },
  ],
});

/** A board of the world, its first view and three of its cards whose titles are their own. */
async function find(page: Page): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  return page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title === 'Issue tracker');
    if (!board) throw new Error('no board');
    const views = await backend.views(board.id);
    const cards = await backend.cards(board.id);
    // Pick three cards that have unique titles and no seeded references pointing to or from them.
    const byTitle = (title: string): StoredCard | undefined =>
      cards.find((card) => !card.fields.isTemplate && card.title === title);
    const source = byTitle('The exported total differs from the screen');
    const target = byTitle('Search misses a word with an ending attached');
    const spare = byTitle('Remove the settings nobody uses');
    if (!source || !target || !spare) throw new Error('too few cards');
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: board.id, viewId: first?.id ?? '', source, target, spare };
  });
}

async function openCard(page: Page, place: Place, cardId: string): Promise<void> {
  await page.goto(boardPath(place.boardId, place.viewId, cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
}

/** The editor of the body, once the piece of the app that holds it has arrived. */
async function bodyEditor(page: Page): Promise<Locator> {
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

const chips = (page: Page): Locator => page.getByTestId('card-body').locator('[data-reference]');
const offered = (page: Page): Locator => page.locator('[data-suggestions="reference"]');

/**
 * Somebody else writes the body of a card. Once the card is open, never
 * before: the data lives in memory, and a page that loads starts from the
 * seed again.
 */
async function writeBody(page: Page, place: Place, cardId: string, body: object): Promise<void> {
  await page.evaluate(
    async ({ boardId, cardId: id, body: written }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const now = await dev.backend.card(boardId, id);
      await dev.someoneElse().writeCardBody(boardId, id, { body: written, readAt: now.updatedAt });
    },
    { boardId: place.boardId, cardId, body },
  );
}

/** What the stored body of a card names. */
function named(page: Page, place: Place, cardId: string): Promise<string[]> {
  return page.evaluate(
    async ({ boardId, cardId: id }) => {
      const found: string[] = [];
      const walk = (node: { type?: string; attrs?: { id?: string }; content?: unknown[] }) => {
        if (node.type === 'reference' && node.attrs?.id) found.push(node.attrs.id);
        for (const child of node.content ?? []) walk(child as typeof node);
      };
      const card = await (window as unknown as { fvociDev: Dev }).fvociDev.backend.card(
        boardId,
        id,
      );
      if (card.body) walk(card.body);
      return found;
    },
    { boardId: place.boardId, cardId },
  );
}

test('`[[` offers cards by title; the one taken is a chip that opens it, and the card named lists what names it', async ({
  page,
}) => {
  const place = await find(page);
  await openCard(page, place, place.source.id);
  const editor = await bodyEditor(page);
  await editor.click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' [[');
  // Before anything is typed: the cards of the board.
  await expect(offered(page).getByRole('option').first()).toBeVisible();
  // A title has spaces: the list follows them.
  await page.keyboard.type(place.target.title.toLowerCase());
  const option = offered(page).getByRole('option', { name: place.target.title });
  await expect(option).toHaveCount(1);
  await expect(option).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
  await expect(offered(page)).toHaveCount(0);

  const chip = chips(page).last();
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveText(new RegExp(`${place.target.title}$`));
  await expect(editor).not.toContainText('[[');
  await expect.poll(() => named(page, place, place.source.id)).toEqual([place.target.id]);

  // The chip opens the card it names, over the view that is shown.
  await chip.getByRole('link').click();
  await expect(page).toHaveURL(
    new RegExp(`${boardPath(place.boardId, place.viewId, place.target.id)}$`),
  );
  await expect(page.getByTestId('card-title')).toHaveValue(place.target.title);
  // Which lists what names it, and leads back.
  const backlinks = page.getByTestId('card-backlinks');
  await expect(backlinks.getByRole('link')).toHaveText([new RegExp(place.source.title)]);
  await backlinks.getByRole('link').click();
  await expect(page).toHaveURL(
    new RegExp(`${boardPath(place.boardId, place.viewId, place.source.id)}$`),
  );
  await expect(page.getByTestId('card-title')).toHaveValue(place.source.title);
  // A card nothing names has no such part at all.
  await expect(page.getByTestId('card-backlinks')).toHaveCount(0);
});

test('a finger on a chip in a body not being written opens the card, and the body never takes the focus', async ({
  page,
  hasTouch,
}) => {
  test.skip(!hasTouch, 'a finger needs a touch screen');
  const place = await find(page);
  await openCard(page, place, place.source.id);
  const editor = await bodyEditor(page);
  await writeBody(page, place, place.source.id, naming(place.target.id));
  const chip = chips(page).first();
  await expect(chip).toHaveAttribute('data-state', 'shown');
  // Whether the body had the focus at any moment: on a phone even a moment is a keyboard.
  await editor.evaluate((root) => {
    (document.activeElement as HTMLElement | null)?.blur();
    const seen = { focused: false };
    root.addEventListener('focus', () => {
      seen.focused = true;
    });
    (window as unknown as { seen: typeof seen }).seen = seen;
  });

  await chip.getByRole('link').tap();
  await expect(page).toHaveURL(
    new RegExp(`${boardPath(place.boardId, place.viewId, place.target.id)}$`),
  );
  await expect(page.getByTestId('card-title')).toHaveValue(place.target.title);
  expect(
    await page.evaluate(() => (window as unknown as { seen: { focused: boolean } }).seen.focused),
  ).toBe(false);
});

test('a chip shows the title as it is now, and no title for what is gone or was never there', async ({
  page,
}) => {
  const place = await find(page);
  await openCard(page, place, place.source.id);
  await bodyEditor(page);
  await writeBody(page, place, place.source.id, naming(place.target.id, place.spare.id, UNKNOWN));
  await expect(chips(page)).toHaveCount(3);
  await expect(chips(page).nth(0)).toHaveAttribute('data-state', 'shown');
  await expect(chips(page).nth(0)).toContainText(place.target.title);
  await expect(chips(page).nth(1)).toContainText(place.spare.title);
  await expect(chips(page).nth(2)).toHaveAttribute('data-state', 'unseen');
  await expect(chips(page).nth(2)).toHaveText("Link you can't see");
  await expect(chips(page).nth(2).getByRole('link')).toHaveCount(0);

  // Somebody else renames one card and deletes the other.
  await page.evaluate(
    async ({ boardId, target, spare }) => {
      const other = (window as unknown as { fvociDev: Dev }).fvociDev.someoneElse();
      await other.patchCard(boardId, target, { title: 'Renamed elsewhere' });
      await other.batchDeleteCards(boardId, [spare]);
    },
    { boardId: place.boardId, target: place.target.id, spare: place.spare.id },
  );
  await expect(chips(page).nth(0)).toContainText('Renamed elsewhere');
  await expect(chips(page).nth(1)).toHaveAttribute('data-state', 'unseen');
  await expect(chips(page).nth(1)).toHaveText("Link you can't see");
  await expect(page.getByTestId('card-body')).not.toContainText(place.spare.title);
});

test('a reference in a comment is a chip that opens what it names', async ({ page }) => {
  const place = await find(page);
  await openCard(page, place, place.source.id);
  await page.evaluate(
    async ({ boardId, cardId, body }) => {
      const other = (window as unknown as { fvociDev: Dev }).fvociDev.someoneElse();
      await other.createComment(boardId, cardId, body);
    },
    { boardId: place.boardId, cardId: place.source.id, body: naming(place.target.id) },
  );
  const chip = page.getByTestId('card-comments').locator('[data-reference]').last();
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await chip.getByRole('link').click();
  await expect(page).toHaveURL(
    new RegExp(`${boardPath(place.boardId, place.viewId, place.target.id)}$`),
  );
  // The comment names it: the card it is on is what names.
  await expect(page.getByTestId('card-backlinks').getByRole('link')).toHaveText([
    new RegExp(place.source.title),
  ]);
});

/** Pastes text into what has the focus, as a browser reports it. */
async function paste(page: Page, text: string): Promise<void> {
  await page.evaluate((pasted) => {
    const data = new DataTransfer();
    data.setData('text/plain', pasted);
    document.activeElement?.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    );
  }, text);
}

test('the pasted address of a card the reader may see becomes a reference; any other stays an address', async ({
  page,
  baseURL,
}) => {
  const place = await find(page);
  await openCard(page, place, place.source.id);
  const editor = await bodyEditor(page);
  await editor.click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' ');
  await paste(page, `${baseURL ?? ''}${boardPath(place.boardId, place.viewId, place.target.id)}`);
  await expect(chips(page)).toHaveCount(1);
  await expect(chips(page).first()).toHaveAttribute('data-state', 'shown');
  await expect(chips(page).first()).toContainText(place.target.title);
  await expect.poll(() => named(page, place, place.source.id)).toEqual([place.target.id]);

  // An address that names no card the reader may see is pasted as what it is.
  const nowhere = `${baseURL ?? ''}${boardPath(place.boardId, place.viewId, UNKNOWN)}`;
  await paste(page, nowhere);
  await expect(editor).toContainText(nowhere);
  await expect(chips(page)).toHaveCount(1);
});

test('at 390 pixels a long title and the list of `[[` stay inside the screen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const place = await find(page);
  const long = 'A title that goes on for much longer than a narrow screen is wide, and then some';
  await openCard(page, place, place.source.id);
  const editor = await bodyEditor(page);
  await page.evaluate(
    async ({ boardId, target, title }) => {
      await (window as unknown as { fvociDev: Dev }).fvociDev.backend.patchCard(boardId, target, {
        title,
      });
    },
    { boardId: place.boardId, target: place.target.id, title: long },
  );
  await writeBody(page, place, place.source.id, naming(place.target.id, UNKNOWN));
  await expect(chips(page).first()).toHaveAttribute('data-state', 'shown');
  for (const chip of await chips(page).all()) {
    const box = await chip.boundingBox();
    expect(box && box.x >= 0 && box.x + box.width <= 390).toBe(true);
  }
  // The chip fills the line and is a link: the caret is put without a click.
  await editor.focus();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' [[a title that');
  await expect(offered(page).getByRole('option').first()).toBeVisible();
  const list = await offered(page).boundingBox();
  expect(list && list.x >= 0 && list.x + list.width <= 390).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});
