/**
 * A card that repeats, and the activity of a card, end to end on the
 * in-memory backend. On a board with a rule of done that names its due date
 * (the project schedule, given Status is Done and Period: the seed's boards
 * have no rules in memory), a card is set to repeat weekly
 * under its due date; made done in its open dialog, it tells "Made the next
 * card #N", and the notice's undo makes it not done and repeating again and
 * takes the next card away, the dialog still open. On a board that names no
 * due date the repeat is off and says why, and the board's manager names it
 * right there, under the date. Its activity is folded
 * until it is opened, and then says what was done, two rows a line, under a
 * header button 44px tall on a phone.
 */
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';

import { boardPath } from './address.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

/** What `window.fvociDev` offers these tests (src/app/backend.ts). */
interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  card(
    boardId: string,
    cardId: string,
  ): Promise<{ fields: { recurrence?: unknown; recurredTo?: string } }>;
  container(id: string): Promise<{
    dueDatePropertyId?: string | null;
    cardProperties: { id: string; name: string; options: { id: string; label: string }[] }[];
  }>;
  patchContainer(id: string, patch: object): Promise<unknown>;
}

interface Place {
  boardId: string;
  viewId: string;
  cardId: string;
}

const BOARD = 'Project schedule';
const CARD = 'Build';
const WEEKLY = { every: 1, unit: 'week', anchor: 'due' };

/** An iPhone's width, on the phone of the tests. */
async function phoneWidth(page: Page, info: TestInfo): Promise<void> {
  if (info.project.name === 'mobile') await page.setViewportSize({ width: 390, height: 844 });
}

/** Opens the card by its address, from the board's status board. */
async function openCard(page: Page, info: TestInfo): Promise<Place> {
  await phoneWidth(page, info);
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(
    async ([boardTitle, cardTitle]) => {
      const backend = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev.backend;
      const [workspace] = await backend.workspaces();
      const boards = await backend.boards(workspace?.id ?? '');
      const board = boards.find((each) => each.title.startsWith(boardTitle));
      if (!board) throw new Error(`no board ${boardTitle}`);
      const view = (await backend.views(board.id)).find((each) => each.title === 'Status board');
      const card = (await backend.cards(board.id)).find((each) => each.title === cardTitle);
      if (!view || !card) throw new Error(`no card ${cardTitle}`);
      return { boardId: board.id, viewId: view.id, cardId: card.id };
    },
    [BOARD, CARD] as const,
  );
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
  return place;
}

const dialog = (page: Page): Locator => page.getByTestId('card-dialog');

/** The line of a property, by the name of the property. */
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

/** Gives the board its rule of done (Status is Done) and, when `named`, Period as its due date. */
async function ruleTheBoard(page: Page, place: Place, named: boolean): Promise<void> {
  await page.evaluate(
    async ({ boardId, named }) => {
      const { backend } = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev;
      const board = await backend.container(boardId);
      const status = board.cardProperties.find((prop) => prop.name === 'Status');
      const done = status?.options.find((option) => option.label === 'Done');
      const period = board.cardProperties.find((prop) => prop.name === 'Period');
      if (!status || !done || !period) throw new Error('the board has no Status, Done or Period');
      await backend.patchContainer(boardId, {
        doneWhen: { kind: 'option', propertyId: status.id, optionIds: [done.id] },
        dueDatePropertyId: named ? period.id : null,
      });
    },
    { boardId: place.boardId, named },
  );
}

/** How the card repeats, as the backend has it. */
function recurrenceOf(page: Page, place: Place): Promise<unknown> {
  return page.evaluate(
    async ({ boardId, cardId }) =>
      (
        await (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev.backend.card(
          boardId,
          cardId,
        )
      ).fields.recurrence ?? null,
    place,
  );
}

/** How many cards of the board are called as the card is. */
function namesakes(page: Page, place: Place): Promise<number> {
  return page.evaluate(
    async ([boardId, title]) =>
      (
        await (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev.backend.cards(
          boardId,
        )
      ).filter((each) => each.title === title).length,
    [place.boardId, CARD] as const,
  );
}

test('a card set to repeat tells of its next card when it is done, and the undo takes it back', async ({
  page,
}, info) => {
  const place = await openCard(page, info);
  await ruleTheBoard(page, place, true);

  // Weekly, chosen under the due date.
  await property(page, 'Period').locator('[data-slot="property-date"]').click();
  const repeat = page.locator('[data-slot="recurrence"] select').first();
  await expect(repeat).toBeEnabled();
  await repeat.selectOption('weekly');
  await expect.poll(() => recurrenceOf(page, place)).toEqual(WEEKLY);
  await closePicker(page);

  // Done in the open card: the next card is made, and told.
  const status = property(page, 'Status');
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'Done' }).click();
  const toast = page.getByTestId('next-card-toast');
  await expect(toast).toContainText(/Made the next card #\d+/);
  await expect.poll(() => namesakes(page, place)).toBe(2);
  await expect.poll(() => recurrenceOf(page, place)).toBeNull();

  // Its undo: the card is not done and repeats again, the next card goes, the card stays open.
  await toast.getByRole('button', { name: 'Undo' }).click();
  await expect(toast).toHaveCount(0);
  await expect(dialog(page)).toBeVisible();
  await expect(status.locator('dd')).not.toContainText('Done');
  await expect.poll(() => namesakes(page, place)).toBe(1);
  await expect.poll(() => recurrenceOf(page, place)).toEqual(WEEKLY);
});

test('a board that names no due date says why its cards do not repeat, and names it there', async ({
  page,
}, info) => {
  const place = await openCard(page, info);
  await ruleTheBoard(page, place, false);

  await property(page, 'Period').locator('[data-slot="property-date"]').click();
  const repeat = page.locator('[data-slot="recurrence"] select').first();
  await expect(repeat).toBeDisabled();
  await expect(page.locator('[data-slot="recurrence-needs-rules"]')).toBeVisible();
  const rules = page.getByTestId('board-today-rules');
  await rules.getByTestId('board-due-date-property').selectOption({ label: 'Period' });

  await expect(repeat).toBeEnabled();
  await expect(rules).toHaveCount(0);
  const named = await page.evaluate(
    async ({ boardId }) =>
      (
        await (
          window as unknown as { fvociDev: { backend: DevBackend } }
        ).fvociDev.backend.container(boardId)
      ).dueDatePropertyId,
    place,
  );
  expect(named).toBeTruthy();
});

test('the activity of a card is read when it is opened, two rows a line', async ({
  page,
}, info) => {
  await openCard(page, info);
  const status = property(page, 'Status');
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'Blocked' }).click();
  await expect(status.locator('dd')).toContainText('Blocked');

  const activity = page.getByTestId('card-activity');
  const header = activity.getByRole('button', { name: 'Activity' });
  await header.scrollIntoViewIfNeeded();
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  await expect(activity.locator('li')).toHaveCount(0);
  await header.click();
  await expect(header).toHaveAttribute('aria-expanded', 'true');
  const changed = activity.locator('li[data-kind="property"]').first();
  await expect(changed).toContainText('Status');
  await expect(changed).toContainText('Blocked');

  // A change while it is open is read again.
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'In progress' }).click();
  await expect(changed).toContainText('In progress');

  if (info.project.name === 'mobile') {
    const box = await header.boundingBox();
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
    const rows = await changed.evaluate((item) =>
      [...item.children].map((row) => row.getBoundingClientRect().height),
    );
    expect(rows).toHaveLength(2);
    expect(rows[0] ?? 0).toBeLessThan(32);
  }
});
