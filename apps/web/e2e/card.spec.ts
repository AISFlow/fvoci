/**
 * The card detail, end to end, on the in-memory backend. Every test runs in
 * the three projects, which are the three stages of the page: `desktop`
 * (expanded: the card is a dialog), `middle` (medium: a panel at the right),
 * `mobile` (compact: the whole screen).
 *
 * A card is reached by its address, which does not depend on how the board
 * behind it draws its cards. The tests that are about coming from the board
 * (back closes, the focus returns) press a card on the board instead.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of the card, into
 * `target/screens/card/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { ON_A_CARD, boardPath, viewAddress } from './address.ts';
import { screensDir } from './screens.ts';
import {
  clipboardPermissions,
  primaryKey,
  primaryName,
  documentBoundary,
} from './keyboard.support.ts';
import { scrollTo } from './scroll.support.ts';
import { pressInBar } from './shell.support.ts';

// The longer tests here take a dozen steps each. On a machine that is busy with
// other work they have run past the default of thirty seconds without anything
// being wrong, so every test of this file gets three times that.
test.describe.configure({ mode: 'parallel', timeout: 90_000 });

// ---------------------------------------------------------------- support

/** What `window.fvociDev` offers a test (src/app/backend.ts). Only what is used here. */
interface DevBackend {
  latency: number;
  me(): Promise<{ id: string }>;
  addMember(boardId: string, userId: string, role: 'editor'): Promise<unknown>;
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  card(
    boardId: string,
    cardId: string,
  ): Promise<{
    title: string;
    updatedAt: number;
    body: unknown;
    fields: { icon?: string; properties: Record<string, unknown> };
  }>;
  container(containerId: string): Promise<{
    cardProperties: { id: string; name: string; type: string; options: { label: string }[] }[];
  }>;
  patchCard(boardId: string, cardId: string, patch: object): Promise<unknown>;
  writeCardBody(boardId: string, cardId: string, write: object): Promise<unknown>;
  createComment(boardId: string, cardId: string, body: object): Promise<unknown>;
  comments(boardId: string, cardId: string): Promise<{ body: unknown }[]>;
  attachments(boardId: string, cardId: string): Promise<{ name: string }[]>;
  deleteBlock(boardId: string, blockId: string): Promise<void>;
  failNext(method: string, error: unknown): void;
}
interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}
declare global {
  interface Window {
    fvociDev: Dev;
  }
}

interface Place {
  boardId: string;
  viewId: string;
  cardId: string;
}

const paragraph = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** The identifiers of a board, its first view and one of its cards, by their titles. */
async function find(page: Page, board: string, card: string): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  return page.evaluate(
    async ([boardTitle, cardTitle]) => {
      const { backend } = window.fvociDev;
      const [workspace] = await backend.workspaces();
      const boards = await backend.boards(workspace?.id ?? '');
      const found = boards.find((each) => each.title.startsWith(boardTitle ?? ''));
      if (!found) throw new Error(`no board ${boardTitle}`);
      const views = await backend.views(found.id);
      const cards = await backend.cards(found.id);
      const wanted = cards.find((each) => each.title === cardTitle);
      if (!wanted) throw new Error(`no card ${cardTitle}`);
      const first =
        views.find((each) => each.title === 'Status board') ??
        [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
      return { boardId: found.id, viewId: first?.id ?? '', cardId: wanted.id };
    },
    [board, card],
  );
}

const addressOf = (place: Place): string => boardPath(place.boardId, place.viewId, place.cardId);

/** Opens a card by its address and waits until its body, attachments and comments are there. */
async function openCard(page: Page, board: string, card: string): Promise<Place> {
  const place = await find(page, board, card);
  await page.goto(addressOf(place));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
  return place;
}

/** A card of the board everything can be tried on: it has a select, a person, a date, a checkbox and a number. */
const openWelcomeCard = (page: Page): Promise<Place> =>
  openCard(page, 'Project schedule', 'Agree on the scope');
/** A card with a body, comments and seven properties: enough to scroll. */
const openTaskCard = (page: Page): Promise<Place> =>
  openCard(page, 'Project schedule', 'Agree on the scope');

const dialog = (page: Page): Locator => page.getByTestId('card-dialog');
const scroller = (page: Page): Locator => page.getByTestId('card-scroll');
const commentField = (page: Page): Locator => page.getByTestId('comment-field');
const saveState = (page: Page): Locator => page.getByTestId('card-save-state');

/** The editor of the body, once the piece of the app that holds it has arrived. */
async function bodyEditor(page: Page): Promise<Locator> {
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

/** The line of a property, by the name of the property. */
function property(page: Page, name: string): Locator {
  return dialog(page)
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: new RegExp(`^\\s*${name}\\s*$`) }) });
}

/** Chooses a line of the card's menu. */
async function cardMenu(page: Page, action: string): Promise<void> {
  await page.getByTestId('card-menu').click();
  await page.locator(`[data-action="${action}"]`).click();
}

/** Closes the picker that is open: a sheet has a button for it, a popover takes Escape. */
async function closePicker(page: Page): Promise<void> {
  // Only the sheet that is open: one that was just closed is still on its way out.
  const sheet = page.locator('[data-testid="bottom-sheet"][data-state="open"]');
  if ((await sheet.count()) > 0) await sheet.last().getByRole('button', { name: 'Done' }).click();
  else await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
}

/**
 * Lets the page lay itself out again after a test changed a style by hand.
 * A measurement taken right after such a change can still be the old one, and one
 * taken while the card still opens is where the card is on its way.
 */
async function laidOut(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const ending = document
      .getAnimations()
      .filter((animation) =>
        Number.isFinite(Number(animation.effect?.getComputedTiming().endTime)),
      );
    await Promise.all(ending.map((animation) => animation.finished.catch(() => undefined)));
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            resolve();
          });
        });
      }),
  );
}

/** The stored card, as the backend has it. */
const stored = (page: Page, place: Place): ReturnType<DevBackend['card']> =>
  page.evaluate(({ boardId, cardId }) => window.fvociDev.backend.card(boardId, cardId), place);

/** The plain text of a stored body. */
function textOf(body: unknown): string {
  const node = (body ?? {}) as { text?: string; content?: unknown[] };
  return (node.text ?? '') + (node.content ?? []).map(textOf).join('');
}

type StageName = 'compact' | 'medium' | 'expanded';
const STAGE_OF_PROJECT: Record<string, StageName> = {
  mobile: 'compact',
  middle: 'medium',
  desktop: 'expanded',
};
const FRAME_OF_STAGE: Record<StageName, string> = {
  compact: 'full',
  medium: 'panel',
  expanded: 'dialog',
};

// ------------------------------------------------------------------ frame

test('the card has the frame of its stage', async ({ page }, testInfo) => {
  await openTaskCard(page);
  const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
  await expect(dialog(page)).toHaveAttribute('data-frame', FRAME_OF_STAGE[stage]);

  const box = await dialog(page).boundingBox();
  const viewport = page.viewportSize();
  if (!box || !viewport) throw new Error('nothing to measure');
  if (stage === 'compact') {
    // The whole screen, with a back arrow at the top left and no close button.
    expect(box).toEqual({ x: 0, y: 0, width: viewport.width, height: viewport.height });
    await expect(page.getByTestId('card-back')).toBeVisible();
    await expect(page.getByTestId('card-close')).toHaveCount(0);
  } else if (stage === 'medium') {
    // A panel of 480 pixels at the right edge, as high as the screen.
    expect(Math.round(box.width)).toBe(480);
    expect(Math.round(box.x + box.width)).toBe(viewport.width);
    expect(box.y).toBe(0);
    expect(box.height).toBe(viewport.height);
    await expect(page.getByTestId('card-close')).toBeVisible();
    await expect(page.getByTestId('card-back')).toHaveCount(0);
  } else {
    // In the middle, 768 pixels wide, with room above and below.
    expect(Math.round(box.width)).toBe(768);
    expect(Math.round(box.x + box.width / 2)).toBe(viewport.width / 2);
    expect(Math.round(box.y + box.height / 2)).toBe(viewport.height / 2);
    expect(box.y).toBeGreaterThanOrEqual(16);
    await expect(page.getByTestId('card-close')).toBeVisible();
  }
  // The top row says where the card belongs.
  await expect(page.getByTestId('card-location')).toContainText('Project schedule');
});

test('the parts come in the order title, properties, body, attachments, comments, and the comment field is the last row', async ({
  page,
}) => {
  await openTaskCard(page);
  const tops = await Promise.all(
    ['card-heading', 'card-properties', 'card-body', 'card-attachments', 'card-comments'].map(
      async (name) => (await page.getByTestId(name).boundingBox())?.y ?? -1,
    ),
  );
  expect([...tops].sort((a, b) => a - b)).toEqual(tops);

  // The field is not inside what scrolls, and ends where the card ends.
  const composer = await page.getByTestId('comment-composer').boundingBox();
  const frame = await dialog(page).boundingBox();
  const scrolled = await scroller(page).boundingBox();
  if (!composer || !frame || !scrolled) throw new Error('nothing to measure');
  expect(Math.round(composer.y + composer.height)).toBe(Math.round(frame.y + frame.height));
  expect(Math.round(scrolled.y + scrolled.height)).toBe(Math.round(composer.y));
});

test('closing: Escape and the button for a panel or a dialog, the arrow for a full screen, a tap beside a panel', async ({
  page,
}, testInfo) => {
  const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
  const place = await openTaskCard(page);
  const board = viewAddress(place.boardId, place.viewId);

  await page.getByTestId(stage === 'compact' ? 'card-back' : 'card-close').click();
  await expect(dialog(page)).toBeHidden();
  await expect(page).toHaveURL(board);

  await page.goto(addressOf(place));
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await expect(page).toHaveURL(board);

  if (stage === 'compact') return;
  // What is left of the board beside the card is dimmed, and pressing it closes the card.
  await page.goto(addressOf(place));
  await expect(dialog(page)).toBeVisible();
  await page.mouse.click(20, 300);
  await expect(dialog(page)).toBeHidden();
  await expect(page).toHaveURL(board);
});

test('the menu of a card, put away without a choice, gives the focus back to its button', async ({
  page,
}) => {
  await openTaskCard(page);
  const button = page.getByTestId('card-menu');
  // A press, not the keyboard: Safari does not focus a button it presses.
  await button.click();
  const line = page.locator('[data-action="delete"]');
  await expect(line).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(line).toHaveCount(0);
  await expect(dialog(page)).toBeVisible();
  await expect(button).toBeFocused();
});

test('opened from the board: the dialog has the focus, back closes it, and the focus returns to the card', async ({
  page,
}) => {
  await page.goto('/');
  const onBoard = page.locator('[data-card]').first();
  await expect(onBoard).toBeVisible();
  const board = page.url();
  await onBoard.click();
  await expect(dialog(page)).toBeVisible();
  // Not the title: a focused field would raise the on-screen keyboard.
  await expect(dialog(page)).toBeFocused();

  await page.goBack();
  await expect(dialog(page)).toBeHidden();
  expect(page.url()).toBe(board);
  await expect(onBoard).toBeFocused();
});

test('a card that was just made starts with the caret in its title', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Task table', exact: true }).click();
  await page.getByRole('button', { name: 'New', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByTestId('card-title')).toBeFocused();
  await page.keyboard.type('Written at once');
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-card]', { hasText: 'Written at once' })).toHaveCount(1);
});

// ------------------------------------------------- title, icon, properties

test('the title is edited in place, shows on the board, and can be undone', async ({ page }) => {
  const place = await openTaskCard(page);
  const title = page.getByTestId('card-title');
  await expect(title).toHaveValue('Agree on the scope');
  // The title as the board draws it: a card there also shows properties and badges.
  const onBoard = page.locator(`[data-card="${place.cardId}"] [data-slot="card-title-text"]`);

  await title.fill('Agree on every scope');
  // Stored when the typing pauses, without leaving the field.
  await expect(onBoard).toHaveText('Agree on every scope');
  await expect(title).toBeFocused();
  await expect.poll(async () => (await stored(page, place)).title).toBe('Agree on every scope');

  // Enter ends the editing instead of starting a line; Escape puts the stored title back and keeps the card open.
  await title.pressSequentially(' now');
  await page.keyboard.press('Enter');
  await expect(title).not.toBeFocused();
  await expect(onBoard).toHaveText('Agree on every scope now');
  await title.click();
  await title.pressSequentially(' and then some');
  await page.keyboard.press('Escape');
  await expect(title).toHaveValue('Agree on every scope now');
  await expect(dialog(page)).toBeVisible();

  await cardMenu(page, 'undo');
  await expect(title).toHaveValue('Agree on every scope');
  await cardMenu(page, 'undo');
  await expect(title).toHaveValue('Agree on the scope');
  await expect(onBoard).toHaveText('Agree on the scope');
  await cardMenu(page, 'redo');
  await expect(title).toHaveValue('Agree on every scope');
});

test('the icon: one is added at random, drawn again, and removed', async ({ page }) => {
  const place = await openCard(page, 'Meeting notes', 'Design review');
  const icon = async (): Promise<string> => (await stored(page, place)).fields.icon ?? '';
  const had = await icon();
  if (had !== '') {
    await page.getByTestId('card-icon').click();
    await page.locator('[data-action="remove-icon"]').click();
  }
  await expect(page.getByTestId('card-icon')).toHaveCount(0);

  await page.getByTestId('card-add-icon').click();
  await expect(page.getByTestId('card-icon')).toBeVisible();
  await expect.poll(icon).not.toBe('');

  await page.getByTestId('card-icon').click();
  await page.locator('[data-action="remove-icon"]').click();
  await expect(page.getByTestId('card-add-icon')).toBeVisible();
  await expect.poll(icon).toBe('');
});

test('values are edited with the shared editors: select, person, date, checkbox, text; and undone', async ({
  page,
}) => {
  const place = await openWelcomeCard(page);
  const values = async (): Promise<Record<string, unknown>> =>
    (await stored(page, place)).fields.properties;
  const idOf = async (name: string): Promise<string> =>
    (await property(page, name).getAttribute('data-property')) ?? '';

  // Select: the whole value is what is pressed; the list opens, and a choice closes it.
  const status = property(page, 'Status');
  await status.locator('[data-slot="property-select"]').click();
  await page.getByRole('option', { name: 'Blocked' }).click();
  await expect(status.locator('dd')).toContainText('Blocked');
  // Closed, and gone: the person's options below are not taken from a list on its way out.
  await expect(page.getByRole('option')).toHaveCount(0);
  const statusId = await idOf('Status');
  await expect.poll(async () => typeof (await values())[statusId]).toBe('string');

  // Person: the people to choose from are the members of the board.
  const owner = property(page, 'Owner');
  const before = await owner.locator('dd').innerText();
  await owner.locator('[data-slot="property-person"]').click();
  const person = page.getByRole('option').filter({ hasNotText: before.trim() }).first();
  const name = (await person.innerText()).trim();
  await person.click();
  await expect(owner.locator('dd')).toContainText(name);

  // Date: a day of the month that is shown.
  const period = property(page, 'Period');
  await period.locator('[data-slot="property-date"]').click();
  await page.locator('[data-slot="date-picker"] [data-value$="-15"]').first().click();
  await closePicker(page);
  const periodId = await idOf('Period');
  await expect
    .poll(async () => JSON.stringify((await values())[periodId]))
    .toMatch(/"from":"\d{4}-\d{2}-15"/);
  await expect(period.locator('dd')).toContainText('15');

  // Checkbox: pressing it changes it at once.
  const milestone = property(page, 'Milestone');
  const box = milestone.getByRole('checkbox');
  await expect(box).not.toBeChecked();
  await box.click();
  await expect(box).toBeChecked();
  const milestoneId = await idOf('Milestone');
  await expect.poll(async () => (await values())[milestoneId]).toBe('true');

  // Number input: typed in place, stored by Enter.
  const hours = property(page, 'Estimated hours');
  await hours.locator('input').fill('42');
  await page.keyboard.press('Enter');
  const hoursId = await idOf('Estimated hours');
  await expect.poll(async () => (await values())[hoursId]).toBe('42');

  // Undo takes the last change back, then the one before it.
  await cardMenu(page, 'undo');
  await expect.poll(async () => (await values())[hoursId]).toBe('12');
  await cardMenu(page, 'undo');
  await expect(box).not.toBeChecked();
  await expect.poll(async () => (await values())[milestoneId]).toBeUndefined();
});

test('a property is added, named, changed to another kind after a warning, and deleted after a question', async ({
  page,
}) => {
  const place = await openWelcomeCard(page);
  const definitions = (): Promise<{ name: string; type: string; options: { label: string }[] }[]> =>
    page.evaluate(
      async ({ boardId }) => (await window.fvociDev.backend.container(boardId)).cardProperties,
      place,
    );
  const count = (await definitions()).length;

  // Add: the kinds to choose from; the new property is the last line and its menu is open for naming it.
  await page.getByTestId('add-property').click();
  await expect(page.locator('[data-action="number"]')).toHaveText('Number');
  await expect(page.locator('[data-action="multiSelect"]')).toHaveText('Multi select');
  await expect(page.locator('[data-action="updatedBy"]')).toHaveText('Last updated by');
  await page.locator('[data-action="text"]').click();
  await expect(dialog(page).locator('[data-property]')).toHaveCount(count + 1);
  const menu = page.getByTestId('property-menu');
  await expect(menu).toBeVisible();
  // The menu says what kind the property is.
  await expect(menu.locator('[data-action="type"]')).toContainText('Text');
  const nameField = menu.getByRole('textbox', { name: 'Property name' });
  await expect(nameField).toHaveValue('Text');
  await nameField.fill('Notes');
  await nameField.press('Enter');
  await expect(menu).toBeHidden();
  await expect(property(page, 'Notes')).toHaveCount(1);
  await expect.poll(async () => (await definitions()).at(-1)?.name).toBe('Notes');

  // A value, so that changing the kind has something to lose or keep.
  await property(page, 'Notes').locator('input').fill('Remember this');
  await page.keyboard.press('Enter');
  await expect
    .poll(async () => JSON.stringify((await stored(page, place)).fields))
    .toContain('Remember this');

  // To a select: the text becomes an option. The reader is told what happens to the values and confirms.
  await property(page, 'Notes').getByTestId('property-name').click();
  await page.getByTestId('property-menu').locator('[data-action="type"]').click();
  await page.getByTestId('property-types').locator('[data-type="select"]').click();
  const confirm = page.getByTestId('confirm-dialog');
  await expect(confirm).toContainText('from "Text" to "Select"');
  await expect(confirm).toContainText('Values kept: 1. Values removed: 0.');
  await confirm.getByRole('button', { name: 'Change property' }).click();
  await expect(property(page, 'Notes')).toHaveAttribute('data-property-type', 'select');
  await expect(property(page, 'Notes').locator('dd')).toContainText('Remember this');
  await expect
    .poll(async () => (await definitions()).at(-1)?.options.map((option) => option.label))
    .toEqual(['Remember this']);

  // To a date: nothing of an option can be a date. The reader is told, and may still say no.
  await property(page, 'Notes').getByTestId('property-name').click();
  await expect(page.getByTestId('property-menu').locator('[data-action="type"]')).toContainText(
    'Select',
  );
  await page.getByTestId('property-menu').locator('[data-action="type"]').click();
  await expect(page.getByTestId('property-types').locator('[data-type="select"]')).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await page.getByTestId('property-types').locator('[data-type="date"]').click();
  await expect(confirm).toContainText('Values kept: 0. Values removed: 1.');
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(property(page, 'Notes')).toHaveAttribute('data-property-type', 'select');

  // One undo takes the whole change of kind back: the definition and the value.
  await cardMenu(page, 'undo');
  await expect(property(page, 'Notes')).toHaveAttribute('data-property-type', 'text');
  await expect(property(page, 'Notes').locator('input')).toHaveValue('Remember this');

  // Delete: asked first; "Cancel" keeps it, "Delete" removes the line.
  await property(page, 'Notes').getByTestId('property-name').click();
  await page.getByTestId('property-menu').locator('[data-action="delete"]').click();
  await expect(confirm).toContainText('delete the property "Notes"');
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(property(page, 'Notes')).toHaveCount(1);
  await property(page, 'Notes').getByTestId('property-name').click();
  await page.getByTestId('property-menu').locator('[data-action="delete"]').click();
  await confirm.getByRole('button', { name: 'Delete' }).click();
  await expect(property(page, 'Notes')).toHaveCount(0);
  await expect.poll(async () => (await definitions()).length).toBe(count);
});

test('a name typed into the menu is kept when a kind is chosen right after it', async ({
  page,
}) => {
  await openWelcomeCard(page);
  // Add an unused link property so changing its kind needs no confirmation.
  await page.getByTestId('add-property').click();
  await page.locator('[data-action="url"]').click();
  const menu = page.getByTestId('property-menu');
  await menu.getByRole('textbox', { name: 'Property name' }).fill('Source');
  await menu.locator('[data-action="type"]').click();
  await page.getByTestId('property-types').locator('[data-type="text"]').click();
  await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
  await expect(property(page, 'Source')).toHaveAttribute('data-property-type', 'text');
  await expect(property(page, 'URL')).toHaveCount(0);
});

test('menus that open and close inside the card leave the history alone: one step back still closes it', async ({
  page,
}) => {
  await page.goto('/');
  const onBoard = page.locator('[data-card]').first();
  await expect(onBoard).toBeVisible();
  const board = page.url();
  await onBoard.click();
  await expect(page.getByTestId('card-comments')).toBeVisible();
  const opened = page.url();

  // The card's menu, then a property added from one menu while the next opens by itself:
  // on a compact page these are sheets, and each takes a step of the history while it is open.
  await cardMenu(page, 'copy-link');
  await page.getByTestId('add-property').click();
  await page.locator('[data-action="checkbox"]').click();
  await expect(page.getByTestId('property-menu')).toBeVisible();
  await closePicker(page);
  await expect(page.getByTestId('property-menu')).toBeHidden();
  expect(page.url()).toBe(opened);
  // Let a step back that a closing sheet asked for arrive before the reader's own.
  await expect.poll(() => page.evaluate(() => history.length)).toBeGreaterThan(1);
  await page.waitForTimeout(300);

  await page.goBack();
  await expect(dialog(page)).toBeHidden();
  expect(page.url()).toBe(board);
});

// ------------------------------------------------------------------- body

test('the editor is a piece of its own that is fetched when a card is first opened', async ({
  page,
}) => {
  const scripts: string[] = [];
  const addresses: string[] = [];
  page.on('request', (request) => {
    const name = new URL(request.url()).pathname.split('/').pop() ?? '';
    if (!name.endsWith('.js')) return;
    scripts.push(name);
    addresses.push(request.url());
  });
  const isEditor = (name: string): boolean => name.startsWith('BodyEditor-');
  /** Whether a script carries the editing library: its class names are in the code as text. */
  const carriesEditor = async (address: string): Promise<boolean> =>
    (await (await page.request.get(address)).text()).includes('ProseMirror');

  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await page.waitForLoadState('networkidle');
  // The board is there, and the editor was not asked for: not by its name, and not inside another file.
  expect(scripts.length).toBeGreaterThan(0);
  expect(scripts.filter(isEditor)).toEqual([]);
  const first = [...addresses];
  for (const address of first) expect(await carriesEditor(address), address).toBe(false);

  await page.locator('[data-card]').first().click();
  await bodyEditor(page);
  expect(scripts.filter(isEditor)).toHaveLength(1);
  const later = addresses.filter((address) => !first.includes(address));
  expect(await Promise.all(later.map(carriesEditor))).toContain(true);

  // Another card uses the piece that is already here.
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await page.locator('[data-card]').nth(1).click();
  await bodyEditor(page);
  expect(scripts.filter(isEditor)).toHaveLength(1);
});

test('the body is sent as it is typed, and is there when the card is opened again', async ({
  page,
}, testInfo) => {
  const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
  const place = await openTaskCard(page);
  const editor = await bodyEditor(page);
  await expect(saveState(page)).toHaveText('');

  await editor.click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('Typed and kept. ');
  await expect(saveState(page)).toHaveText('Saving…');
  await expect(saveState(page)).toHaveText('Saved');
  // The card's body is written from the room's document a little later.
  await expect
    .poll(async () => textOf((await stored(page, place)).body))
    .toContain('Typed and kept');

  // Closing sends what the pause has not sent yet.
  await page.keyboard.type('And this too. ');
  await page.getByTestId(stage === 'compact' ? 'card-back' : 'card-close').click();
  await expect(dialog(page)).toBeHidden();
  await expect.poll(async () => textOf((await stored(page, place)).body)).toContain('And this too');

  await page.locator(`[data-card="${place.cardId}"]`).click();
  await expect(await bodyEditor(page)).toContainText('Typed and kept. And this too.');
});

test('two tabs edit one body together: each sees what the other types, and both stay', async ({
  page,
  context,
}) => {
  const place = await openTaskCard(page);
  const editor = await bodyEditor(page);
  const other = await context.newPage();
  await other.goto(addressOf(place));
  const theirs = await bodyEditor(other);

  await editor.click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('From the first tab. ');
  await theirs.click();
  await documentBoundary(other, 'end');
  await other.keyboard.type(' From the second tab.');

  for (const shown of [editor, theirs]) {
    await expect(shown).toContainText('From the first tab. ');
    await expect(shown).toContainText(' From the second tab.');
  }
  await expect(saveState(page)).toHaveText('Saved');
  await expect(saveState(other)).toHaveText('Saved');
  expect(await theirs.textContent()).toBe(await editor.textContent());
  await other.close();
});

test('a body written whole elsewhere while the reader types: the new text shows, and what was lost is said', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const editor = await bodyEditor(page);
  await replacedWhileTyping(page, place, editor);
  const notice = page.getByTestId('body-lost');
  await expect(notice).toContainText('replaced elsewhere');
  await expect(await bodyEditor(page)).toHaveText('Theirs');
  await notice.getByRole('button', { name: 'Close' }).click();
  await expect(notice).toHaveCount(0);

  // The editor takes typing again, and it is kept.
  await (await bodyEditor(page)).click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' and mine after');
  await expect(saveState(page)).toHaveText('Saved');
  await expect
    .poll(async () => textOf((await stored(page, place)).body))
    .toBe('Theirs and mine after');
});

test('somebody else changes only a property while the reader writes: both are kept, without a notice', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const editor = await bodyEditor(page);
  await page.evaluate(() => {
    window.fvociDev.backend.latency = 1000;
  });
  await editor.click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('Written meanwhile. ');
  await page.evaluate(async ({ boardId, cardId }) => {
    await window.fvociDev.someoneElse().patchCard(boardId, cardId, { title: 'Renamed by them' });
  }, place);

  await expect(saveState(page)).toHaveText('Saved', { timeout: 15_000 });
  await expect(page.getByTestId('body-lost')).toHaveCount(0);
  await expect(page.getByTestId('card-title')).toHaveValue('Renamed by them');
  await expect
    .poll(async () => textOf((await stored(page, place)).body))
    .toContain('Written meanwhile');
  expect((await stored(page, place)).title).toBe('Renamed by them');
});

/**
 * The reader types, and before that reaches the room somebody else writes
 * the body whole (as a client of the server's API does): the room gets a new
 * document and the reader's typing is dropped with the old one. The reader's
 * backend is slowed down for the moment, so that the other write comes first.
 */
test('the reader stays in the body when the room sends it anew, with the caret where it was', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const editor = await bodyEditor(page);
  await editor.click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('Mine ');
  // In the card already: what follows loses nothing.
  await expect.poll(async () => textOf((await stored(page, place)).body)).toContain('Mine ');
  await otherJoins(page, place);
  await writtenWholeElsewhere(page, place, paragraph('Theirs, written whole'));
  await expect(await bodyEditor(page)).toHaveText('Theirs, written whole');
  await expect(page.getByTestId('body-lost')).toHaveCount(0);
  // Not pressed again: the new editor has the focus, and the caret near where it was.
  await page.keyboard.type('Z');
  await expect(await bodyEditor(page)).toContainText('Z');
  // Where it was in the line (after "Mine "), not at the start the new editor begins with.
  expect((await (await bodyEditor(page)).textContent())?.startsWith('Z')).toBe(false);
  expect(await page.evaluate(() => document.activeElement?.closest('.ProseMirror') !== null)).toBe(
    true,
  );
});

/**
 * Somebody else becomes a member of the card's board: a card is read by its
 * board's members only. Done before anything is slowed down, so that their
 * write then takes no longer than the write itself.
 */
async function otherJoins(page: Page, place: Place): Promise<void> {
  await page.evaluate(async ({ boardId }) => {
    const other = await window.fvociDev.someoneElse().me();
    await window.fvociDev.backend.addMember(boardId, other.id, 'editor');
  }, place);
}

/** Somebody else, a member (`otherJoins`), writes the card's body whole as another client of the API does. */
async function writtenWholeElsewhere(page: Page, place: Place, body: object): Promise<void> {
  await page.evaluate(
    async ({ place: at, body: written }) => {
      const other = window.fvociDev.someoneElse();
      const card = await other.card(at.boardId, at.cardId);
      await other.writeCardBody(at.boardId, at.cardId, { body: written, readAt: card.updatedAt });
    },
    { place, body },
  );
}

async function replacedWhileTyping(page: Page, place: Place, editor: Locator): Promise<void> {
  await otherJoins(page, place);
  await page.evaluate(() => {
    window.fvociDev.backend.latency = 1500;
  });
  await editor.click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('Mine. ');
  await writtenWholeElsewhere(page, place, paragraph('Theirs'));
  await expect(page.getByTestId('body-lost')).toBeVisible({ timeout: 15_000 });
  await page.evaluate(() => {
    window.fvociDev.backend.latency = 40;
  });
}

// ----------------------------------------- the window changes under a card

test('the window changes size under an open card: the text, the caret, the focus and the scroll position stay', async ({
  page,
  browserName,
}) => {
  const SIZES = [
    { width: 360, height: 780, frame: 'full' },
    { width: 626, height: 890, frame: 'panel' },
    { width: 890, height: 626, frame: 'dialog' },
    { width: 1280, height: 720, frame: 'dialog' },
    { width: 360, height: 780, frame: 'full' },
  ] as const;
  const [start, ...rest] = SIZES;

  await page.setViewportSize(start);
  await openCard(page, 'Getting started with fvoci', 'Write in the body');
  await expect(dialog(page)).toHaveAttribute('data-frame', 'full');
  const editor = await bodyEditor(page);

  // A comment that is being written, then the caret in the middle of what was typed into the body.
  await commentField(page).fill('half a comment');
  await editor.locator('p').first().click();
  await documentBoundary(page, 'start');
  await page.keyboard.type('ABCDEF ');
  for (let step = 0; step < 4; step += 1) await page.keyboard.press('ArrowLeft');
  await expect(editor).toContainText('ABCDEF');

  // Marks on the elements themselves: an element that was built again would not carry one.
  const KEPT = ['card-dialog', 'card-scroll', 'comment-field', 'card-title'];
  await page.evaluate((names) => {
    for (const name of names) {
      const element = document.querySelector<HTMLElement>(`[data-testid="${name}"]`);
      if (element) element.dataset.kept = 'yes';
    }
    const editable = document.querySelector<HTMLElement>(
      '[data-testid="card-body"] [contenteditable]',
    );
    if (editable) editable.dataset.kept = 'yes';
  }, KEPT);
  const sameElements = async (): Promise<void> => {
    for (const name of KEPT)
      await expect(page.getByTestId(name)).toHaveAttribute('data-kept', 'yes');
    await expect(editor).toHaveAttribute('data-kept', 'yes');
  };

  const scrollTop = (): Promise<number> => scroller(page).evaluate((element) => element.scrollTop);
  const scrollMax = (): Promise<number> =>
    scroller(page).evaluate((element) => element.scrollHeight - element.clientHeight);
  await scrollTo(scroller(page), 0, 120);
  expect(await scrollTop()).toBe(120);
  // WebKit's scroll anchoring moves the place when the frame changes (styles/app.css, `webkit:`);
  // the other engines keep theirs.
  expect(await scroller(page).evaluate((element) => getComputedStyle(element).overflowAnchor)).toBe(
    browserName === 'webkit' ? 'none' : 'auto',
  );

  // Round the sizes with the caret in the body. A letter typed after each change lands at the caret.
  let typed = '';
  for (const [index, size] of rest.entries()) {
    await page.setViewportSize(size);
    await expect(dialog(page)).toHaveAttribute('data-frame', size.frame);
    await sameElements();
    await expect(editor).toBeFocused();
    await expect(commentField(page)).toHaveValue('half a comment');
    // The same pixel, or as near to it as this size can scroll.
    await expect.poll(scrollTop).toBe(Math.min(120, await scrollMax()));
    typed += String(index + 1);
    await page.keyboard.type(String(index + 1));
    await expect(editor).toContainText(`ABC${typed}DEF`);
  }
  // Back at the first size the card is where it was scrolled to.
  await expect.poll(scrollTop).toBe(120);
  await expect(editor).toContainText('ABC1234DEF');

  // The same round with the caret in the comment field, in the middle of the draft.
  await commentField(page).focus();
  await commentField(page).evaluate((element: HTMLTextAreaElement) => {
    element.setSelectionRange(4, 4);
  });
  typed = '';
  for (const [index, size] of rest.entries()) {
    await page.setViewportSize(size);
    await expect(dialog(page)).toHaveAttribute('data-frame', size.frame);
    await sameElements();
    await expect(commentField(page)).toBeFocused();
    typed += String(index + 1);
    await page.keyboard.type(String(index + 1));
    await expect(commentField(page)).toHaveValue(`half${typed} a comment`);
    await expect(editor).toContainText('ABC1234DEF');
  }
  await expect.poll(scrollTop).toBe(120);

  // One step back still closes the card: the changes of size added nothing to the history.
  await expect(dialog(page)).toBeVisible();
});

test('another card is shown in the same dialog', async ({ page }) => {
  const place = await openTaskCard(page);
  await dialog(page).evaluate((element) => {
    element.dataset.kept = 'yes';
  });
  // "Duplicate" makes a copy and shows it in place of this card.
  await cardMenu(page, 'duplicate');
  await expect(page).not.toHaveURL(new RegExp(`${place.cardId}$`));
  await expect(page).toHaveURL(ON_A_CARD);
  await expect(dialog(page)).toHaveAttribute('data-kept', 'yes');
  await expect(page.getByTestId('card-title')).toHaveValue('Agree on the scope');
  // The copy has the body and none of the comments.
  await expect(await bodyEditor(page)).toContainText('The new sign-up screen');
  await expect(page.getByTestId('card-comments')).toContainText('No comments yet.');
});

// --------------------------------------------------- comments, attachments

test('comments: written with the button and with Ctrl+Enter, lines become paragraphs, and the own ones can be deleted', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const list = page.getByTestId('card-comments').locator('[data-comment]');
  const count = await list.count();
  expect(count).toBeGreaterThan(0);
  // The seeded ones are from the others on the board: no delete button on them.
  const send = page.getByTestId('comment-send');
  await expect(send).toBeDisabled();

  await commentField(page).fill('First line\n\nSecond line');
  await send.click();
  await expect(list).toHaveCount(count + 1);
  await expect(commentField(page)).toHaveValue('');
  const mine = list.last();
  await expect(mine.locator('p').filter({ hasText: 'First line' })).toHaveText('First line');
  await expect(mine.locator('p').filter({ hasText: 'Second line' })).toHaveText('Second line');
  await expect(mine).toContainText('김하나 (@hana)');
  const bodies = await page.evaluate(
    async ({ boardId, cardId }) =>
      (await window.fvociDev.backend.comments(boardId, cardId)).map((each) => each.body),
    place,
  );
  expect(bodies).toContainEqual({
    type: 'doc',
    content: [
      { type: 'paragraph', content: [{ type: 'text', text: 'First line' }] },
      { type: 'paragraph', content: [{ type: 'text', text: 'Second line' }] },
    ],
  });

  const modifier = await primaryName(page);
  await expect(commentField(page)).toHaveAttribute(
    'title',
    modifier === 'Meta' ? '⌘Enter sends' : 'Ctrl+Enter sends',
  );
  await expect(commentField(page)).toHaveAttribute('aria-keyshortcuts', `${modifier}+Enter`);
  await commentField(page).fill('The other modifier does not send');
  await commentField(page).press(`${modifier === 'Meta' ? 'Control' : 'Meta'}+Enter`);
  await expect(list).toHaveCount(count + 1);
  await expect(commentField(page)).not.toHaveValue('');
  await commentField(page).fill('With the keyboard');
  await commentField(page).press(await primaryKey(page, 'Enter'));
  await expect(list).toHaveCount(count + 2);
  await expect(list.last()).toContainText('With the keyboard');
  // Sent after the other modifier was pressed, and alone: that press sent nothing.
  await expect(list.filter({ hasText: 'The other modifier does not send' })).toHaveCount(0);
  // The newest comment is in sight.
  await expect(list.last()).toBeInViewport();

  await list.last().getByRole('button', { name: 'Delete the comment' }).click();
  await expect(list).toHaveCount(count + 1);
  await cardMenu(page, 'undo');
  await expect(list).toHaveCount(count + 2);
});

test('a comment changed twice says what each text changed from the one before, the first as it was', async ({
  page,
}) => {
  await openTaskCard(page);
  const list = page.getByTestId('card-comments').locator('[data-comment]');
  const count = await list.count();
  await commentField(page).fill('Buy milk today');
  await page.getByTestId('comment-send').click();
  await expect(list).toHaveCount(count + 1);
  const mine = list.last();
  for (const text of ['Buy oat milk today', 'Buy oat milk tomorrow']) {
    await mine.locator('[data-action="edit"]').click();
    const field = mine.getByRole('textbox', { name: 'Comment text' });
    await field.fill(text);
    await field.press(await primaryKey(page, 'Enter'));
    await expect(mine).toContainText(text);
  }
  await mine.locator('[data-action="history"]').click();
  const texts = mine.getByTestId('comment-history').locator('[data-version]');
  await expect(texts).toHaveCount(3);
  await expect(texts.nth(0)).toContainText('Buy milk today');
  await expect(texts.nth(0).locator('ins, del')).toHaveCount(0);
  await expect(texts.nth(1).locator('ins')).toHaveText(['oat ']);
  await expect(texts.nth(1).locator('del')).toHaveCount(0);
  await expect(texts.nth(1).getByTestId('comment-diff-counts')).toHaveText('1 added · 0 removed');
  await expect(texts.nth(2)).toHaveAttribute('data-version', 'now');
  await expect(texts.nth(2).locator('del')).toHaveText(['today']);
  await expect(texts.nth(2).locator('ins')).toHaveText(['tomorrow']);
  // Heard as well as seen: each mark says what it is.
  await expect(texts.nth(2).locator('ins')).toHaveAttribute('data-diff-label', 'Added');
});

test('a comment that is being written is not lost by leaving: Escape leaves the field first, and the draft waits for the card', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  await commentField(page).fill('Not sent yet');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeVisible();
  await expect(commentField(page)).not.toBeFocused();
  await expect(commentField(page)).toHaveValue('Not sent yet');

  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await page.locator(`[data-card="${place.cardId}"]`).click();
  await expect(commentField(page)).toHaveValue('Not sent yet');

  // Sent, it is not a draft any more.
  await page.getByTestId('comment-send').click();
  await expect(commentField(page)).toHaveValue('');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await page.locator(`[data-card="${place.cardId}"]`).click();
  await expect(commentField(page)).toHaveValue('');
});

test('a comment somebody else writes appears by itself', async ({ page }) => {
  const place = await openTaskCard(page);
  const list = page.getByTestId('card-comments').locator('[data-comment]');
  const count = await list.count();
  await page.evaluate(
    async ({ place: at, body }) => {
      await window.fvociDev.someoneElse().createComment(at.boardId, at.cardId, body);
    },
    { place, body: paragraph('Seen from elsewhere') },
  );
  await expect(list).toHaveCount(count + 1);
  await expect(list.last()).toContainText('Seen from elsewhere');
  // Not the reader's, but the reader is the board's admin, who deletes
  // anyone's comment (`deleteBlock` in the contract).
  await expect(list.last().getByRole('button', { name: /^Delete the comment/ })).toHaveCount(1);
});

test('attachments: several files at once, a link to each, deleting and bringing back', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const section = page.getByTestId('card-attachments');
  const rows = section.locator('[data-attachment]');
  await expect(rows).toHaveCount(0);

  await page.getByTestId('attach-input').setInputFiles([
    { name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('some notes') },
    { name: 'figures.csv', mimeType: 'text/csv', buffer: Buffer.alloc(3 * 1024, 'x') },
  ]);
  await expect(rows).toHaveCount(2);
  const notes = rows.filter({ hasText: 'notes.txt' });
  await expect(notes).toContainText('10 B');
  await expect(rows.filter({ hasText: 'figures.csv' })).toContainText('3.0 KiB');
  await expect(notes.getByRole('link', { name: 'notes.txt' })).toHaveAttribute(
    'download',
    'notes.txt',
  );
  await expect(notes.getByRole('link')).toHaveAttribute('href', /^blob:/);
  await expect(section.locator('[data-upload]')).toHaveCount(0);

  await notes.getByRole('button', { name: 'Delete notes.txt' }).click();
  await expect(rows).toHaveCount(1);
  await cardMenu(page, 'undo');
  await expect(rows).toHaveCount(2);
  const names = await page.evaluate(
    async ({ boardId, cardId }) =>
      (await window.fvociDev.backend.attachments(boardId, cardId)).map((each) => each.name),
    place,
  );
  expect([...names].sort()).toEqual(['figures.csv', 'notes.txt']);
});

test('an upload shows how far it is, in place and in the stack, and can be given up; one that fails stays until dismissed', async ({
  page,
}) => {
  await openTaskCard(page);
  // Wait for the body room and editor to settle before slowing backend latency for the upload,
  // or the slow-join status line can appear mid-click and shift the attachments section down.
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  const section = page.getByTestId('card-attachments');
  const uploads = section.locator('[data-upload]');
  const file = (name: string): { name: string; mimeType: string; buffer: Buffer } => ({
    name,
    mimeType: 'text/plain',
    buffer: Buffer.from('x'.repeat(64)),
  });

  // Slow enough to be seen on its way.
  await page.evaluate(() => {
    window.fvociDev.backend.latency = 1500;
  });
  await page.getByTestId('attach-input').setInputFiles([file('slow.txt')]);
  const slow = uploads.filter({ hasText: 'slow.txt' });
  const stacked = page.getByTestId('transfers').locator('[data-transfer="upload"]');
  await expect(slow.getByRole('progressbar', { name: 'Uploading slow.txt' })).toBeVisible();
  await expect(stacked.filter({ hasText: 'slow.txt' })).toBeVisible();
  // Given up in place, it is given up in the stack too: the same upload.
  await slow.getByRole('button', { name: 'Cancel the upload of slow.txt' }).click();
  await expect(uploads).toHaveCount(0);
  await expect(stacked).toHaveCount(0);
  await page.evaluate(() => {
    window.fvociDev.backend.latency = 40;
  });

  // The next upload is refused as too large.
  await page.evaluate(async () => {
    const { backend } = window.fvociDev;
    const refusal = await backend.card('nowhere', 'nothing').catch((error: unknown) => error);
    const BackendError = (refusal as { constructor: new (status: number, body: object) => unknown })
      .constructor;
    backend.failNext('upload', new BackendError(413, { limit: 1024 }));
  });
  await page.getByTestId('attach-input').setInputFiles([file('refused.txt')]);
  const refused = uploads.filter({ hasText: 'refused.txt' });
  await expect(refused).toContainText('Not uploaded');
  await expect(refused).toHaveAttribute('data-failed', '');
  // Why is told where every upload is, and not again where problems are.
  await expect(stacked.filter({ hasText: 'refused.txt' })).toContainText('It is larger than');
  await expect(page.getByTestId('problems').locator('[data-problem]')).toHaveCount(0);
  await refused.getByRole('button', { name: 'Dismiss refused.txt' }).click();
  await expect(uploads).toHaveCount(0);
  await expect(stacked).toHaveCount(0);
  // Given up and refused: neither was attached.
  await expect(section.locator('[data-attachment]')).toHaveCount(0);
});

test('files dropped on the attachments are attached', async ({ page }) => {
  await openTaskCard(page);
  const section = page.getByTestId('card-attachments');
  const dataTransfer = await page.evaluateHandle(() => {
    const transfer = new DataTransfer();
    transfer.items.add(new File(['dropped'], 'dropped.txt', { type: 'text/plain' }));
    transfer.items.add(new File(['too'], 'second.txt', { type: 'text/plain' }));
    return transfer;
  });
  await section.dispatchEvent('dragenter', { dataTransfer });
  await expect(section).toContainText('Drop files here to attach them');
  await section.dispatchEvent('dragover', { dataTransfer });
  await section.dispatchEvent('drop', { dataTransfer });
  await expect(section).not.toContainText('Drop files here');
  await expect(section.locator('[data-attachment]')).toHaveCount(2);
  await expect(section).toContainText('dropped.txt');
  await expect(section).toContainText('second.txt');
});

// ------------------------------------------- keyboard and the device edges

test('where the on-screen keyboard covers the page, the card ends above it and the focused field stays in sight', async ({
  page,
}) => {
  await openTaskCard(page);
  const viewport = page.viewportSize();
  if (!viewport) throw new Error('no viewport');
  const KEYBOARD = 300;
  const bottomOf = async (locator: Locator): Promise<number> => {
    const box = await locator.boundingBox();
    if (!box) throw new Error('not on the page');
    return box.y + box.height;
  };

  // A field near the bottom of what is in sight, then the keyboard.
  const field = property(page, 'Estimated hours').locator('input');
  await scroller(page).evaluate((element) => {
    element.scrollTop = 0;
  });
  await field.focus();
  await page.evaluate((height) => {
    document.documentElement.style.setProperty('--keyboard', `${height}px`);
  }, KEYBOARD);
  await laidOut(page);

  const edge = viewport.height - KEYBOARD;
  await expect.poll(() => bottomOf(dialog(page))).toBeLessThanOrEqual(edge);
  expect(await bottomOf(page.getByTestId('comment-composer'))).toBeLessThanOrEqual(edge);
  await expect(page.getByTestId('comment-send')).toBeInViewport();
  // The field that has the focus is inside what is left of the scrolling box.
  await expect(field).toBeFocused();
  await expect.poll(() => bottomOf(field)).toBeLessThanOrEqual(await bottomOf(scroller(page)));
  const scrolled = await scroller(page).boundingBox();
  const focused = await field.boundingBox();
  expect(focused?.y ?? -1).toBeGreaterThanOrEqual(scrolled?.y ?? 0);

  // The comment field itself, with the keyboard up.
  await commentField(page).focus();
  expect(await bottomOf(commentField(page))).toBeLessThanOrEqual(edge);

  // The keyboard goes away: the card takes the room back.
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--keyboard', '0px');
  });
  await expect.poll(() => bottomOf(page.getByTestId('comment-composer'))).toBeGreaterThan(edge);
});

test('the card keeps clear of the edges the device keeps', async ({ page }, testInfo) => {
  const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
  await openTaskCard(page);
  const viewport = page.viewportSize();
  if (!viewport) throw new Error('no viewport');
  await page.evaluate(() => {
    const root = document.documentElement.style;
    root.setProperty('--safe-top', '30px');
    root.setProperty('--safe-right', '20px');
    root.setProperty('--safe-bottom', '34px');
    root.setProperty('--safe-left', '10px');
  });
  await laidOut(page);
  const header = await page.getByTestId('card-header').boundingBox();
  const send = await page.getByTestId('comment-send').boundingBox();
  const frame = await dialog(page).boundingBox();
  if (!header || !send || !frame) throw new Error('nothing to measure');
  // Whatever the frame, nothing to press is under an edge.
  expect(send.x + send.width).toBeLessThanOrEqual(viewport.width - 20);
  if (stage === 'expanded') {
    expect(frame.x).toBeGreaterThanOrEqual(10);
    expect(frame.x + frame.width).toBeLessThanOrEqual(viewport.width - 20);
    return;
  }
  // A full screen and a panel reach the edges, and keep what is in them inside.
  expect(header.y).toBeGreaterThanOrEqual(30);
  expect(send.y + send.height).toBeLessThanOrEqual(viewport.height - 34);
  if (stage === 'compact') expect(header.x).toBeGreaterThanOrEqual(10);
});

// ------------------------------------------- reading only, and no card

test('on a board the reader may only look at, nothing can be changed', async ({ page }) => {
  await openCard(page, 'Study notes', 'Big O notation');

  // The title is a heading, the names of the properties are not buttons, the values are not fields.
  await expect(page.getByTestId('card-title')).toHaveText('Big O notation');
  expect(await page.getByTestId('card-title').evaluate((element) => element.tagName)).toBe('H2');
  await expect(dialog(page).locator('[data-property]').first()).toBeVisible();
  await expect(page.getByTestId('property-name')).toHaveCount(0);
  await expect(page.getByTestId('add-property')).toHaveCount(0);
  await expect(page.getByTestId('card-add-icon')).toHaveCount(0);
  await expect(
    page.getByTestId('card-properties').locator('input:not(:disabled), button:not(:disabled)'),
  ).toHaveCount(0);

  // The body is read in its room (others' changes come in), never edited, and says why.
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading');
  await expect(page.getByTestId('card-body').locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(page.getByTestId('body-room-note')).toHaveText(
    'You can read this description but not change it.',
  );

  // No attaching, no commenting; the menu offers the link only.
  await expect(page.getByTestId('attach')).toHaveCount(0);
  await expect(page.getByTestId('comment-composer')).toHaveCount(0);
  await page.getByTestId('card-menu').click();
  await expect(page.locator('[data-action="copy-link"]')).toBeVisible();
  await expect(page.locator('[data-action="delete"]')).toHaveCount(0);
  await expect(page.locator('[data-action="duplicate"]')).toHaveCount(0);
});

test('a card that is not there says so and can be closed', async ({ page }, testInfo) => {
  const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
  const place = await find(page, 'Project schedule', 'Design the screens');
  await page.goto(boardPath(place.boardId, place.viewId, 'no-such-card'));
  const missing = page.getByTestId('card-missing');
  await expect(missing.getByRole('alert')).toHaveText(
    "This card doesn't exist or is inaccessible.",
  );
  // The frame is the one of the stage, with its way out.
  await expect(page.getByTestId(stage === 'compact' ? 'card-back' : 'card-close')).toBeVisible();
  await missing.getByRole('button', { name: 'Close' }).click();
  await expect(dialog(page)).toBeHidden();
  await expect(page).toHaveURL(viewAddress(place.boardId, place.viewId));
});

test('somebody else deletes the card while it is open: the same notice', async ({ page }) => {
  const place = await openTaskCard(page);
  await expect(page.getByTestId('card-title')).toBeVisible();
  await page.evaluate(async ({ boardId, cardId }) => {
    await window.fvociDev.someoneElse().deleteBlock(boardId, cardId);
  }, place);
  await expect(page.getByTestId('card-missing').getByRole('alert')).toHaveText(
    "This card doesn't exist or is inaccessible.",
  );
  await expect(page.getByTestId('card-title')).toHaveCount(0);
  await expect(page.getByTestId('comment-composer')).toHaveCount(0);
  await page.getByTestId('card-missing').getByRole('button', { name: 'Close' }).click();
  await expect(dialog(page)).toBeHidden();
});

// ------------------------------------------------------------------- menu

test('the menu deletes the card without asking, the card closes, and undo brings it back', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const onBoard = page.locator(`[data-card="${place.cardId}"]`);
  await expect(onBoard).toHaveCount(1);

  await cardMenu(page, 'delete');
  // No question in between: the card closes by the choice alone. (A question
  // would keep the card open, and the next line would fail.)
  await expect(page.getByTestId('confirm-dialog')).toBeHidden();
  await expect(dialog(page)).toBeHidden();
  await expect(onBoard).toHaveCount(0);
  await expect(page).toHaveURL(viewAddress(place.boardId, place.viewId));

  await pressInBar(page, 'undo');
  await expect(onBoard).toHaveCount(1);
});

test('the menu copies the link to the card', async ({ page, context, browserName }) => {
  await context.grantPermissions(clipboardPermissions(browserName));
  const place = await openTaskCard(page);
  await cardMenu(page, 'copy-link');
  await expect(saveState(page)).toHaveText('Copied!');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(
    new RegExp(`${addressOf(place)}$`),
  );
});

// ------------------------------------------------------------------ Korean

test.describe('in Korean', () => {
  test.use({ locale: 'ko-KR' });

  test('the card speaks Korean', async ({ page }) => {
    await openTaskCard(page);
    await expect(commentField(page)).toHaveAttribute('placeholder', '댓글 추가...');
    await expect(page.getByTestId('comment-send')).toHaveText('보내기');
    await expect(page.getByTestId('card-attachments')).toContainText('첨부 파일');
    await expect(page.getByTestId('add-property')).toHaveText('+ 속성 추가');
    const editor = await bodyEditor(page);
    await editor.click();
    await page.keyboard.type('한글 ');
    await expect(saveState(page)).toHaveText('저장됨');
  });
});

// ------------------------------------------------------------ the pictures

test.describe('pictures of the card', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');

  const SCREENS = screensDir('card');

  for (const scheme of ['light', 'dark'] as const) {
    test(`the card, its menu, a picker and a loss of typing (${scheme})`, async ({
      page,
    }, testInfo) => {
      const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
      const shot = async (name: string): Promise<void> => {
        await page.screenshot({ path: path.join(SCREENS, `${name}-${stage}-${scheme}.png`) });
      };
      // Nothing fades or slides: a picture is not taken halfway through.
      await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
      const place = await openWelcomeCard(page);
      const editor = await bodyEditor(page);
      await page.waitForLoadState('networkidle');
      await page.evaluate(() => document.fonts.ready);
      await shot('card');

      // Further down: attachments, uploads and comments.
      await page
        .getByTestId('attach-input')
        .setInputFiles([
          { name: 'kick-off-notes.txt', mimeType: 'text/plain', buffer: Buffer.alloc(2300, 'x') },
        ]);
      await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toHaveCount(
        1,
      );
      await commentField(page).fill('다음 주 화요일까지 초안을 올릴게요.');
      await page.getByTestId('comment-send').click();
      await commentField(page).fill('A comment that is being written');
      await page.getByTestId('card-comments').scrollIntoViewIfNeeded();
      await shot('card-comments');

      // The select picker of a property: a sheet on a compact page, a popover otherwise.
      await scroller(page).evaluate((element) => {
        element.scrollTop = 0;
      });
      await property(page, 'Status').locator('[data-slot="property-select"]').click();
      await expect(page.getByRole('option').first()).toBeVisible();
      await shot('card-select');
      await closePicker(page);

      // The menu of a property's definition.
      await property(page, 'Phase').getByTestId('property-name').click();
      await expect(page.getByTestId('property-menu')).toBeVisible();
      await shot('card-property-menu');
      await closePicker(page);

      // The card's own menu.
      await page.getByTestId('card-menu').click();
      await expect(page.locator('[data-action="copy-link"]')).toBeVisible();
      await shot('card-menu');
      await closePicker(page);

      // What the reader typed was dropped with a body written whole elsewhere.
      await replacedWhileTyping(page, place, editor);
      await page.getByTestId('body-lost').scrollIntoViewIfNeeded();
      await shot('card-body-lost');
    });
  }
});
