/**
 * Cards on the real server, on a board's kanban: making, changing, moving,
 * deleting them, and bringing a deleted one back from the trash. Each test
 * makes its own board through the API as its owner, who then works on the
 * page.
 */
import { expect, test, type Page } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

function column(page: Page, name: string) {
  return page.locator('[data-column]').filter({
    has: page.locator('h3').filter({ hasText: new RegExp(`^\\s*${name}\\s*$`) }),
  });
}

test('a card made in a column is the server’s, with the value of that column', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('maker');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Errands' });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('board-page')).toBeVisible();

  await page.getByRole('button', { name: 'New card in Doing' }).click();
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  // The card opens whole: its comments and attachments are read too.
  await expect(dialog.getByTestId('card-comments')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  // A body written in the new card is the server's too.
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await page.getByTestId('card-body').locator('[contenteditable]').first().click();
  await page.keyboard.type('Bring a list');
  await expect
    .poll(async () => {
      const [made] = (await (
        await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
          params: { perPage: 1000 },
        })
      ).json()) as Schemas['Card'][];
      return made?.hasBody;
    })
    .toBe(true);
  await page.goBack();
  await expect(dialog).toBeHidden();
  await expect(column(page, 'Doing').locator('[data-card]')).toHaveCount(1);

  // On the server, in that column.
  const cards = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
      params: { perPage: 1000 },
    })
  ).json()) as Schemas['Card'][];
  expect(cards).toHaveLength(1);
  expect(cards[0]?.properties).toEqual({ status: board.options.Doing });

  await page.reload();
  await expect(column(page, 'Doing').locator('[data-card]')).toHaveCount(1);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a card’s title and values are changed on the server, the last value taken away too', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('editor');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'House',
    cards: [
      { title: 'Paint the shed', status: 'To do' },
      { title: 'Fix the gate', status: 'To do', note: 'the latch' },
    ],
  });
  const cardId = board.cards['Paint the shed'] ?? '';
  const stored = async (): Promise<Schemas['Card']> =>
    (await (
      await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
    ).json()) as Schemas['Card'];

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await column(page, 'To do').getByText('Paint the shed').click();
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();

  await page.getByTestId('card-title').fill('Paint the whole shed');
  await expect.poll(async () => (await stored()).title).toBe('Paint the whole shed');

  const status = dialog
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: /^\s*Status\s*$/ }) });
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'Done' }).click();
  await expect
    .poll(async () => (await stored()).properties)
    .toEqual({ status: board.options.Done });

  // The last value of the card goes too: the patch names it to clear.
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('button', { name: 'Clear' }).click();
  await expect.poll(async () => (await stored()).properties).toEqual({});
  expect((await stored()).title).toBe('Paint the whole shed');

  await page.goBack();
  await expect(dialog).toBeHidden();

  // A card with another value: changing one keeps the other.
  const gate = board.cards['Fix the gate'] ?? '';
  await column(page, 'To do').getByText('Fix the gate').click();
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'Doing' }).click();
  await expect
    .poll(
      async () =>
        (
          (await (
            await api.get(`/api/v1/w/${board.workspaceId}/cards/${gate}`)
          ).json()) as Schemas['Card']
        ).properties,
    )
    .toEqual({ status: board.options.Doing, note: 'the latch' });
  await page.goBack();

  await page.reload();
  await expect(page.getByText('Paint the whole shed')).toBeVisible();
  await expect(column(page, 'To do').locator('[data-card]')).toHaveCount(0);
  await expect(column(page, 'Done').locator('[data-card]')).toHaveCount(0);
  await expect(column(page, 'Doing').getByText('Fix the gate')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a card carried to another column and a place in it is moved on the server', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('mover');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Moves',
    cards: [
      { title: 'Sweep', status: 'To do' },
      { title: 'Mop', status: 'To do' },
      { title: 'Dust', status: 'Done' },
    ],
  });
  const card = (title: string) =>
    page.locator('[data-card]').filter({ has: page.getByText(title, { exact: true }) });
  const titlesIn = (name: string) =>
    column(page, name)
      .locator('[data-card] [data-slot="card-title-text"]')
      .evaluateAll((titles) => titles.map((element) => element.textContent.trim()));

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect(card('Sweep')).toBeVisible();

  // With the keyboard: To do, two columns to the right (Done), above Dust.
  await card('Sweep').focus();
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Space');
  await expect.poll(() => titlesIn('Done')).toEqual(['Sweep', 'Dust']);

  // On the server: the card's value, and its place in the view's order.
  const sweep = board.cards.Sweep ?? '';
  const dust = board.cards.Dust ?? '';
  await expect
    .poll(
      async () =>
        (
          (await (
            await api.get(`/api/v1/w/${board.workspaceId}/cards/${sweep}`)
          ).json()) as Schemas['Card']
        ).properties,
    )
    .toEqual({ status: board.options.Done });
  await expect
    .poll(async () => {
      const [view] = (await (
        await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
          params: { blockId: board.viewId },
        })
      ).json()) as Schemas['Block'][];
      const order = (view?.fields.cardOrder ?? []) as string[];
      return order.includes(sweep) && order.indexOf(sweep) < order.indexOf(dust);
    })
    .toBe(true);

  // Within a column: Dust above Sweep. Only the view's order changes.
  await card('Dust').focus();
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Space');
  await expect.poll(() => titlesIn('Done')).toEqual(['Dust', 'Sweep']);
  await expect
    .poll(async () => {
      const [view] = (await (
        await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
          params: { blockId: board.viewId },
        })
      ).json()) as Schemas['Block'][];
      const order = (view?.fields.cardOrder ?? []) as string[];
      return order.includes(dust) && order.indexOf(dust) < order.indexOf(sweep);
    })
    .toBe(true);

  await page.reload();
  await expect.poll(() => titlesIn('Done')).toEqual(['Dust', 'Sweep']);
  expect(await titlesIn('To do')).toEqual(['Mop']);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a card deleted goes to the server’s trash, and undo brings it back from there', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('tidier');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Tidy',
    cards: [
      { title: 'Keep me', status: 'To do' },
      { title: 'Throw me out', status: 'To do' },
    ],
  });
  const gone = board.cards['Throw me out'] ?? '';
  const onServer = async (): Promise<string[]> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
          params: { perPage: 1000 },
        })
      ).json()) as Schemas['Card'][]
    ).map((card) => card.title);

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect(column(page, 'To do').locator('[data-card]')).toHaveCount(2);

  await page.getByRole('button', { name: 'Card menu: Throw me out' }).click();
  await page.locator('[data-action="delete"]').click();
  await expect(column(page, 'To do').locator('[data-card]')).toHaveCount(1);
  await expect.poll(onServer).toEqual(['Keep me']);
  // Gone from the board: the server no longer gives it.
  expect((await api.get(`/api/v1/w/${board.workspaceId}/cards/${gone}`)).status()).toBe(404);

  await page.getByRole('button', { name: 'Undo' }).click();
  await expect(column(page, 'To do').getByText('Throw me out')).toBeVisible();
  await expect.poll(async () => (await onServer()).sort()).toEqual(['Keep me', 'Throw me out']);

  await page.reload();
  await expect(column(page, 'To do').locator('[data-card]')).toHaveCount(2);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});
