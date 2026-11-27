/**
 * The gallery view on the in-memory backend: the cards of the view as a
 * grid whose number of columns follows the width it has, opening and making
 * cards, and the menu of a card.
 */
import { expect, test, type Page } from '@playwright/test';

import { ON_A_CARD, placeOf, viewIdOf } from './address.ts';
import { boardPage, card, choose, dialog, fitPane, openView, undo } from './kanban.support.ts';
import { screensDir } from './screens.ts';

const WELCOME = ['Getting started with fvoci', 'Gallery'] as const;

const cards = (page: Page) => page.getByTestId('gallery-grid').locator('[data-card]');

/** The titles of the cards, in the order of the document. */
async function titles(page: Page): Promise<string[]> {
  return page
    .getByTestId('gallery-grid')
    .locator('[data-slot="card-title-text"]')
    .evaluateAll((all) => all.map((element) => element.textContent.trim()));
}

/** How many cards stand side by side: the number of different left edges. */
async function columns(page: Page): Promise<number> {
  const lefts = await cards(page).evaluateAll((all) =>
    all.map((element) => Math.round(element.getBoundingClientRect().left)),
  );
  return new Set(lefts).size;
}

test('the cards of the view stand in a grid, in the order of the view', async ({ page }, info) => {
  await openView(page, info, ...WELCOME);
  await expect(cards(page)).toHaveCount(12);
  const expected = await page.evaluate((id) => {
    const { workspace } = (
      window as unknown as {
        fvociDev: {
          workspace: {
            board: { view(id: string): object; cardsOf(view: object): { title: string }[] };
          };
        };
      }
    ).fvociDev;
    return workspace.board.cardsOf(workspace.board.view(id)).map((shown) => shown.title);
  }, viewIdOf(page.url()));
  expect(await titles(page)).toEqual(expected);
  // Every card of the seed has a body, and the face says so.
  await expect(cards(page).first().locator('[data-slot="card-has-body"]')).toBeAttached();
  // No card is wider than the grid, and none is cut off.
  const widths = await page.getByTestId('gallery').evaluate((element) => ({
    scroll: element.scrollWidth,
    client: element.clientWidth,
  }));
  expect(widths.scroll).toBe(widths.client);
});

test('the number of columns follows the width of the gallery', async ({ page }, info) => {
  await openView(page, info, ...WELCOME);
  // A card is at least 224 pixels wide; 12 between cards and at the sides.
  await fitPane(page, 300);
  expect(await columns(page)).toBe(1);
  await fitPane(page, 412);
  expect(await columns(page)).toBe(1);
  await fitPane(page, 520);
  expect(await columns(page)).toBe(2);
  await fitPane(page, 760);
  expect(await columns(page)).toBe(3);
  // Narrower than a card: the card takes the width there is.
  await fitPane(page, 200);
  expect(await columns(page)).toBe(1);
  const box = await cards(page).first().boundingBox();
  expect(box?.width).toBe(200 - 24);
});

test('a press on a card opens it, and so does Enter', async ({ page }, info) => {
  await openView(page, info, ...WELCOME);
  await card(page, 'Open a card').click();
  await expect(dialog(page)).toContainText('Open a card');
  await expect(page).toHaveURL(ON_A_CARD);
  await page.goBack();
  await expect(dialog(page)).toBeHidden();
  await card(page, 'Filter, sort and group').focus();
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toContainText('Filter, sort and group');
});

test('"+ New" makes a card and opens it', async ({ page }, info) => {
  await openView(page, info, ...WELCOME);
  await page.getByTestId('gallery-new').click();
  await expect(dialog(page)).toBeVisible();
  await page.goBack();
  await expect(dialog(page)).toBeHidden();
  await expect(cards(page)).toHaveCount(13);
  await undo(page);
  await expect(cards(page)).toHaveCount(12);
});

test('the menu of a card duplicates and deletes it, and has nothing about columns', async ({
  page,
}, info) => {
  await openView(page, info, ...WELCOME);
  await page.getByRole('button', { name: 'Card menu: Open a card' }).click();
  await expect(page.locator('[data-action]')).toHaveCount(3);
  await expect(page.locator('[data-action="move"]')).toHaveCount(0);
  await expect(page.locator('[data-action="select"]')).toHaveCount(0);
  await choose(page, 'duplicate');
  await expect(card(page, 'Open a card')).toHaveCount(2);
  await expect(cards(page)).toHaveCount(13);

  await page.getByRole('button', { name: 'Card menu: Filter, sort and group' }).click();
  await choose(page, 'delete');
  await expect(card(page, 'Filter, sort and group')).toHaveCount(0);
  await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
  await undo(page);
  await expect(card(page, 'Filter, sort and group')).toHaveCount(1);
});

test('a body another person writes is followed by the card: its preview and its tasks', async ({
  page,
}, info) => {
  await openView(page, info, ...WELCOME);
  const face = card(page, 'Open a card');
  const task = (checked: boolean, text: string) => ({
    type: 'taskItem',
    attrs: { checked },
    content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
  });
  const write = async (tasks: object[], words: string): Promise<void> => {
    const body = {
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: words }] },
        { type: 'taskList', content: tasks },
      ],
    };
    await page.evaluate(
      async ([boardId, written]) => {
        const dev = (
          window as unknown as {
            fvociDev: {
              backend: Pick<DevBackend, 'cards'>;
              someoneElse(): DevBackend;
            };
          }
        ).fvociDev;
        const found = (await dev.backend.cards(boardId)).find((one) => one.title === 'Open a card');
        if (!found) throw new Error('no card');
        await dev
          .someoneElse()
          .writeCardBody(boardId, found.id, { body: written, readAt: found.updatedAt });
      },
      [placeOf(page.url()).boardId, body] as const,
    );
  };

  // Told as the server tells it: the summary of the body, not the body.
  await write([task(false, 'one'), task(false, 'two')], 'Written by someone else');
  await expect(face.locator('[data-slot="card-preview"]')).toContainText('Written by someone else');
  await expect(face.locator('[data-slot="card-tasks"]')).toContainText('0/2');
  await write([task(true, 'one'), task(false, 'two')], 'And one is done');
  await expect(face.locator('[data-slot="card-preview"]')).toContainText('And one is done');
  await expect(face.locator('[data-slot="card-tasks"]')).toContainText('1/2');
});

/** What the test asks of a backend in the page. */
interface DevBackend {
  cards(boardId: string): Promise<{ id: string; title: string; updatedAt: number }[]>;
  writeCardBody(
    boardId: string,
    cardId: string,
    write: { body: object; readAt: number },
  ): Promise<unknown>;
}

test.describe('screens', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');

  const STAGE: Record<string, string> = {
    mobile: 'compact',
    middle: 'medium',
    desktop: 'expanded',
  };

  for (const scheme of ['light', 'dark'] as const) {
    test(`the gallery, ${scheme}`, async ({ page }, info) => {
      const stage = STAGE[info.project.name] ?? 'expanded';
      await page.emulateMedia({ colorScheme: scheme });
      await openView(page, info, ...WELCOME);
      await expect(boardPage(page)).toBeVisible();
      await expect(cards(page)).toHaveCount(12);
      await page.screenshot({ path: `${screensDir('gallery')}/gallery-${stage}-${scheme}.png` });
    });
  }
});
