/**
 * The card relation property, end to end, on the in-memory backend: made
 * from the card, cards picked and taken out, shown in the card and in the
 * table, a chip that opens the card it names, and a named card that is
 * deleted and brought back. Every test runs at the three stages.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { pressInBar } from './shell.support.ts';
import { boardPath } from './address.ts';

interface StoredCard {
  id: string;
  title: string;
  fields: { isTemplate: boolean; properties: Record<string, unknown> };
}

interface Dev {
  backend: {
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
    views(boardId: string): Promise<{ id: string; title: string }[]>;
    cards(boardId: string): Promise<StoredCard[]>;
    container(containerId: string): Promise<{ cardProperties: { id: string; name: string }[] }>;
  };
}

interface Place {
  boardId: string;
  viewId: string;
  /** Three cards with titles of their own: the one under test and two to name. */
  cards: StoredCard[];
}

/** The table "Task table" of the board "Project schedule", and three of its cards. */
async function openTable(page: Page): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('app-shell')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const boardId = boards.find((each) => each.title === 'Project schedule')?.id ?? '';
    const views = await backend.views(boardId);
    const cards = (await backend.cards(boardId)).filter((card) => !card.fields.isTemplate);
    const titled = cards.filter(
      (card, at) => card.title !== '' && cards.findIndex((o) => o.title === card.title) === at,
    );
    return {
      boardId,
      viewId: views.find((each) => each.title === 'Task table')?.id ?? '',
      cards: titled.slice(0, 3),
    };
  });
  expect(place.cards).toHaveLength(3);
  await page.goto(boardPath(place.boardId, place.viewId));
  await expect(page.getByTestId('table')).toBeVisible();
  return place;
}

const dialog = (page: Page): Locator => page.getByTestId('card-dialog');

function property(page: Page, name: string): Locator {
  return dialog(page)
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: new RegExp(`^\\s*${name}\\s*$`) }) });
}

/** Closes the picker that is open: a sheet has a button for it, a popover takes Escape. */
async function closePicker(page: Page): Promise<void> {
  const sheet = page.locator('[data-testid="bottom-sheet"][data-state="open"]');
  if ((await sheet.count()) > 0) await sheet.last().getByRole('button', { name: 'Done' }).click();
  else await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
}

async function storedValue(page: Page, place: Place, cardId: string): Promise<unknown> {
  return page.evaluate(
    async ({ boardId, cardId: id }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const board = await backend.container(boardId);
      const after = board.cardProperties.find((each) => each.name === 'After')?.id ?? '';
      const card = (await backend.cards(boardId)).find((each) => each.id === id);
      return card?.fields.properties[after];
    },
    { boardId: place.boardId, cardId },
  );
}

async function propertyId(page: Page, place: Place): Promise<string> {
  return page.evaluate(async ({ boardId }) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const board = await backend.container(boardId);
    return board.cardProperties.find((each) => each.name === 'After')?.id ?? '';
  }, place);
}

/** Makes the relation property "After" from the card that is open. */
async function addRelation(page: Page): Promise<void> {
  await page.getByTestId('add-property').click();
  await expect(page.locator('[data-action="relation"]')).toHaveText('Card relation');
  await page.locator('[data-action="relation"]').click();
  const nameField = page
    .getByTestId('property-menu')
    .getByRole('textbox', { name: 'Property name' });
  await expect(nameField).toHaveValue('Card relation');
  await nameField.fill('After');
  await nameField.press('Enter');
  await expect(property(page, 'After')).toHaveAttribute('data-property-type', 'relation');
}

/** Picks a card into the relation by typing its title, with the keyboard. */
async function pick(page: Page, title: string): Promise<void> {
  await property(page, 'After').locator('[data-slot="relation-edit"]').click();
  const search = page.getByRole('combobox', { name: 'After' });
  await search.fill(title);
  await expect(page.getByRole('listbox', { name: 'After' }).getByRole('option')).toHaveCount(1);
  await search.press('Enter');
  await closePicker(page);
}

const chips = (scope: Locator): Locator => scope.locator('[data-slot="relation-chip"]');

test('a relation is made, cards are picked and taken out, and it shows in the card and the table', async ({
  page,
}) => {
  const place = await openTable(page);
  const [own, first, second] = place.cards as [StoredCard, StoredCard, StoredCard];
  await page.goto(boardPath(place.boardId, place.viewId, own.id));
  await expect(dialog(page)).toBeVisible();
  await addRelation(page);

  await pick(page, first.title);
  await pick(page, second.title);
  await expect(chips(property(page, 'After'))).toHaveText([first.title, second.title]);
  await expect
    .poll(() => storedValue(page, place, own.id))
    .toEqual([
      { card: first.id, kind: 'fs', lagDays: 0 },
      { card: second.id, kind: 'fs', lagDays: 0 },
    ]);

  // The card itself is not offered.
  await property(page, 'After').locator('[data-slot="relation-edit"]').click();
  await page.getByRole('combobox', { name: 'After' }).fill(own.title);
  await expect(
    page
      .getByRole('listbox', { name: 'After' })
      .getByRole('option', { name: own.title, exact: true }),
  ).toHaveCount(0);
  await page.getByRole('combobox', { name: 'After' }).fill('');
  // Taken out with the button by its title.
  await page.getByRole('button', { name: `Remove ${second.title}` }).click();
  await closePicker(page);
  await expect(chips(property(page, 'After'))).toHaveText([first.title]);
  await expect
    .poll(() => storedValue(page, place, own.id))
    .toEqual([{ card: first.id, kind: 'fs', lagDays: 0 }]);

  // In the table: a new column, and the chip in the row of the card.
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  const column = await propertyId(page, place);
  const cell = page
    .getByTestId('table')
    .locator(`[data-nav-row][data-row="${own.id}"] [data-col="${column}"]`);
  await expect(chips(cell)).toHaveText([first.title]);

  // The chip opens the card it names.
  await cell.getByRole('button', { name: `Open ${first.title}` }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/${first.id}$`));
});

test('a named card that is deleted is not shown, and is shown again when it is brought back', async ({
  page,
}) => {
  const place = await openTable(page);
  const [own, named] = place.cards as [StoredCard, StoredCard];
  await page.goto(boardPath(place.boardId, place.viewId, own.id));
  await expect(dialog(page)).toBeVisible();
  await addRelation(page);
  await pick(page, named.title);
  await expect
    .poll(() => storedValue(page, place, own.id))
    .toEqual([{ card: named.id, kind: 'fs', lagDays: 0 }]);

  // Deleted from its own card, which the chip opens (going to its address would start the app afresh).
  await property(page, 'After')
    .getByRole('button', { name: `Open ${named.title}` })
    .click();
  await expect(page).toHaveURL(new RegExp(`/${named.id}$`));
  await page.getByTestId('card-menu').click();
  await page.locator('[data-action="delete"]').click();
  await expect(dialog(page)).toBeHidden();

  const column = await propertyId(page, place);
  const cell = page
    .getByTestId('table')
    .locator(`[data-nav-row][data-row="${own.id}"] [data-col="${column}"]`);
  await expect(chips(cell)).toHaveCount(0);
  // The value still names it.
  expect(await storedValue(page, place, own.id)).toEqual([
    { card: named.id, kind: 'fs', lagDays: 0 },
  ]);

  await pressInBar(page, 'undo');
  await expect(chips(cell)).toHaveText([named.title]);
});

test('the card a relation names lists the cards that name it, and opens them', async ({ page }) => {
  const place = await openTable(page);
  const [own, named] = place.cards as [StoredCard, StoredCard];
  await page.goto(boardPath(place.boardId, place.viewId, own.id));
  await expect(dialog(page)).toBeVisible();
  await addRelation(page);
  // Nothing names the card yet.
  await expect(property(page, 'After').locator('[data-slot="relation-referrers"]')).toHaveCount(0);
  await pick(page, named.title);

  await property(page, 'After')
    .getByRole('button', { name: `Open ${named.title}` })
    .click();
  await expect(page).toHaveURL(new RegExp(`/${named.id}$`));
  const referrers = property(page, 'After').getByRole('list', {
    name: 'Cards that point to this card',
  });
  await expect(referrers.locator('[data-slot="relation-referrer"]')).toHaveText([own.title]);
  // Its own value is still empty: the list is the other end, not a value of its own.
  await expect(chips(property(page, 'After'))).toHaveCount(0);

  await referrers.getByRole('button', { name: `Open ${own.title}` }).click();
  await expect(page).toHaveURL(new RegExp(`/${own.id}$`));
  await expect(property(page, 'After').locator('[data-slot="relation-referrers"]')).toHaveCount(0);
});
