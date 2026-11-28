/**
 * A card relation on the real server: the server takes a property definition
 * of the kind `relation` and its value as a list of card identifiers, and the
 * value picked in the card is there after the page is loaded again.
 *
 * The first test makes the definition through the API; the second makes it
 * on the page, from the card, as a reader does. The third copies a board: the
 * copy's relation names the copy's cards.
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

test.use({ locale: 'en-US', timezoneId: 'UTC' });

function property(page: Page, name: string) {
  return page
    .getByTestId('card-dialog')
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: new RegExp(`^\\s*${name}\\s*$`) }) });
}

async function closePicker(page: Page): Promise<void> {
  const sheet = page.locator('[data-testid="bottom-sheet"][data-state="open"]');
  if ((await sheet.count()) > 0) await sheet.last().getByRole('button', { name: 'Done' }).click();
  else await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
}

test('cards picked into a relation are stored as a list of identifiers and stay after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('relater');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Relations',
    cards: [
      { title: 'Pour the foundation', status: 'To do' },
      { title: 'Order the bricks', status: 'To do' },
      { title: 'Build the walls', status: 'To do' },
    ],
  });
  const defined = await api.patch(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`, {
    data: {
      updatedCardProperties: [{ id: 'after', name: 'After', type: 'relation', options: [] }],
    },
  });
  expect(defined.ok(), await defined.text()).toBe(true);

  const walls = board.cards['Build the walls'] ?? '';
  const foundation = board.cards['Pour the foundation'] ?? '';
  const bricks = board.cards['Order the bricks'] ?? '';
  const values = async (): Promise<Schemas['Card']['properties']> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${walls}`)
      ).json()) as Schemas['Card']
    ).properties;

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, walls));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  const after = property(page, 'After');
  await expect(after).toHaveAttribute('data-property-type', 'relation');

  for (const title of ['Pour the foundation', 'Order the bricks']) {
    await after.locator('[data-slot="relation-edit"]').click();
    const search = page.getByRole('combobox', { name: 'After' });
    await search.fill(title);
    await expect(page.getByRole('listbox', { name: 'After' }).getByRole('option')).toHaveCount(1);
    await search.press('Enter');
    await closePicker(page);
  }
  await expect
    .poll(async () => (await values()).after)
    .toEqual([
      { card: foundation, kind: 'fs', lagDays: 0 },
      { card: bricks, kind: 'fs', lagDays: 0 },
    ]);

  await page.reload();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(after.locator('[data-slot="relation-chip"]')).toHaveText([
    'Pour the foundation',
    'Order the bricks',
  ]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a relation made on the page from a card, and the cards picked into it, stay after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('relation-maker');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Relations made here',
    cards: [
      { title: 'Dig the trench', status: 'To do' },
      { title: 'Lay the pipe', status: 'To do' },
      { title: 'Fill the trench', status: 'To do' },
    ],
  });
  const fill = board.cards['Fill the trench'] ?? '';
  /** The definition of "After" as the server keeps it, once the page has made it. */
  const defined = async (): Promise<{ id: string; type: string } | undefined> => {
    const answer = await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`);
    const properties = ((await answer.json()) as Schemas['Container']).cardProperties as {
      id: string;
      name: string;
      type: string;
    }[];
    return properties.find((each) => each.name === 'After');
  };

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, fill));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('card-dialog')).toBeVisible();

  // The property is made from the card: "add a property", the kind, its name.
  await page.getByTestId('add-property').click();
  await page.locator('[data-action="relation"]').click();
  const nameField = page
    .getByTestId('property-menu')
    .getByRole('textbox', { name: 'Property name' });
  await expect(nameField).toHaveValue('Card relation');
  await nameField.fill('After');
  await nameField.press('Enter');
  const after = property(page, 'After');
  await expect(after).toHaveAttribute('data-property-type', 'relation');
  await expect.poll(async () => (await defined())?.type).toBe('relation');
  const propertyId = (await defined())?.id ?? '';
  expect(propertyId).not.toBe('');

  for (const title of ['Dig the trench', 'Lay the pipe']) {
    await after.locator('[data-slot="relation-edit"]').click();
    const search = page.getByRole('combobox', { name: 'After' });
    await search.fill(title);
    await expect(page.getByRole('listbox', { name: 'After' }).getByRole('option')).toHaveCount(1);
    await search.press('Enter');
    await closePicker(page);
  }
  const value = async (): Promise<unknown> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${fill}`)
      ).json()) as Schemas['Card']
    ).properties[propertyId];
  await expect.poll(value).toEqual([
    { card: board.cards['Dig the trench'], kind: 'fs', lagDays: 0 },
    { card: board.cards['Lay the pipe'], kind: 'fs', lagDays: 0 },
  ]);

  // After a reload the property and what was picked are there, as the server keeps them.
  await page.reload();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(after).toHaveAttribute('data-property-type', 'relation');
  await expect(after.locator('[data-slot="relation-chip"]')).toHaveText([
    'Dig the trench',
    'Lay the pipe',
  ]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a copy of a board names its own cards in the relation its Gantt chart reads', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('relation-copier');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Copied relations',
    cards: [{ title: 'Lay the floor' }, { title: 'Paint the walls' }],
  });
  const defined = await api.patch(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`, {
    data: {
      updatedCardProperties: [{ id: 'after', name: 'After', type: 'relation', options: [] }],
    },
  });
  expect(defined.ok(), await defined.text()).toBe(true);
  const floor = board.cards['Lay the floor'] ?? '';
  const paint = board.cards['Paint the walls'] ?? '';
  const named = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${paint}`, {
    data: { updatedProperties: { after: [{ card: floor, kind: 'ss', lagDays: -2 }] } },
  });
  expect(named.ok(), await named.text()).toBe(true);
  const gantt = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/views`, {
    data: {
      title: 'Plan',
      viewType: 'gantt',
      fields: { viewType: 'gantt', dateProperty: 'due', dependencyPropertyId: 'after' },
    },
  });
  expect(gantt.ok(), await gantt.text()).toBe(true);

  const copied = await api.post(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/duplicate`,
  );
  expect(copied.ok(), await copied.text()).toBe(true);
  const copy = (await copied.json()) as Schemas['ContainersAndBlocks'];
  const copyId = copy.containers[0]?.id ?? '';
  const read = async <T>(path: string) =>
    (await (await api.get(`/api/v1/w/${board.workspaceId}/boards/${copyId}/${path}`)).json()) as T;
  const cards = await read<Schemas['Card'][]>('cards');
  const byTitle = (title: string) => cards.find((card) => card.title === title);
  const copiedFloor = byTitle('Lay the floor')?.id ?? '';
  const copiedPaint = byTitle('Paint the walls')?.id ?? '';
  expect(copiedFloor).not.toBe('');
  expect(copiedFloor).not.toBe(floor);
  expect(byTitle('Paint the walls')?.properties.after).toEqual([
    { card: copiedFloor, kind: 'ss', lagDays: -2 },
  ]);
  const plan = (await read<Schemas['Block'][]>('views')).find((view) => view.title === 'Plan');
  expect(plan?.fields.dependencyPropertyId).toBe('after');

  // On the page, the copy's card names the copy's card, and opens it.
  const errors = watchErrors(page);
  await page.goto(
    boardPath({ workspaceId: board.workspaceId, boardId: copyId }, plan?.id, copiedPaint),
  );
  await signInOnPage(page, owner);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  const chip = property(page, 'After').locator('[data-slot="relation-chip"]');
  await expect(chip).toHaveText(['Lay the floor SS -2d']);
  await expect(chip.locator('[data-slot="relation-badge"]')).toHaveText('SS -2d');
  await expect(chip).toHaveAttribute('data-card-id', copiedFloor);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});
