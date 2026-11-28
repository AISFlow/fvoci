/**
 * Cards selected in a view and changed together from the bar of the
 * selection (features/views/BulkBar.svelte), on the in-memory backend: on
 * the board and in the table, one undo for the whole change, the values
 * that differ, and where the bar sits on a phone. In every project: with a
 * mouse a card is selected with Ctrl or ⌘, under a finger through "Select"
 * in its menu.
 */
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';

import { primaryName } from './keyboard.support.ts';
import { card, choose, openView, titlesIn, touch } from './kanban.support.ts';

const BOARD = 'Project schedule';
const STATUS = '6c9349df-a282-5b45-9fff-80ccf96accd6';
const PLANNED = ['Build', 'Write the help pages', 'Test', 'Launch'];

const bar = (page: Page): Locator => page.getByTestId('selection-bar');
const statusButton = (page: Page): Locator => bar(page).locator(`[data-bulk-property="${STATUS}"]`);
const row = (page: Page, title: string): Locator =>
  page
    .getByTestId('table')
    .locator('[data-card]')
    .filter({ has: page.locator('[data-slot="row-title"]', { hasText: title }) });

/** Takes cards into the selection: Ctrl or ⌘ with a mouse; "Select" in the first one's menu, then presses, under a finger. */
async function select(
  page: Page,
  info: TestInfo,
  targets: readonly Locator[],
  openMenu: (target: Locator) => Promise<void>,
): Promise<void> {
  const [first, ...rest] = targets;
  if (!first) return;
  if (touch(info)) {
    await openMenu(first);
    await choose(page, 'select');
    for (const target of rest) await target.click();
  } else {
    const modifier = await primaryName(page);
    for (const target of targets) await target.click({ modifiers: [modifier] });
  }
  await expect(bar(page)).toContainText(`${targets.length} selected`);
}

/** Picks an option for every selected card from the bar's button of the status. */
async function setStatus(page: Page, option: string): Promise<void> {
  await statusButton(page).click();
  await page.getByRole('option', { name: option, exact: true }).click();
  await expect(page.locator('[data-testid="bottom-sheet"][data-state="open"]')).toHaveCount(0);
}

/** The shell's undo, by its keys: what was changed together is taken back together. */
async function undoByKeys(page: Page): Promise<void> {
  await page.keyboard.press(`${await primaryName(page)}+z`);
}

test('cards selected on the board move to another status together, and one undo takes them back', async ({
  page,
}, info) => {
  await openView(page, info, BOARD, 'Status board');
  await select(page, info, [card(page, 'Build'), card(page, 'Test')], async () => {
    await page.getByRole('button', { name: 'Card menu: Build' }).click();
  });
  await setStatus(page, 'Blocked');
  await expect.poll(() => titlesIn(page, 'Blocked')).toEqual(['Build', 'Test']);
  expect(await titlesIn(page, 'Planned')).toEqual(['Write the help pages', 'Launch']);
  // The cards are still selected where they went.
  await expect(bar(page)).toContainText('2 selected');

  await undoByKeys(page);
  await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
  expect(await titlesIn(page, 'Blocked')).toEqual([]);
});

test('rows selected in the table show where their values differ, and change in one step', async ({
  page,
}, info) => {
  await openView(page, info, BOARD, 'Task table');
  await expect(page.getByTestId('table')).toBeVisible();
  const titleCell = (title: string): Locator => row(page, title).locator('[data-kind="title"]');
  await select(
    page,
    info,
    [titleCell('Agree on the scope'), titleCell('Build')],
    async (target) => {
      await target.locator('[data-more]').click();
    },
  );
  await expect(statusButton(page)).toHaveAttribute('data-mixed', '');

  await setStatus(page, 'In progress');
  const status = (title: string): Locator => row(page, title).locator(`[data-col="${STATUS}"]`);
  await expect(status('Agree on the scope')).toHaveText('In progress');
  await expect(status('Build')).toHaveText('In progress');
  await expect(statusButton(page)).not.toHaveAttribute('data-mixed', '');

  await undoByKeys(page);
  await expect(status('Agree on the scope')).toHaveText('Done');
  await expect(status('Build')).toHaveText('Planned');

  // Escape lets the selection go.
  await page.keyboard.press('Escape');
  await expect(bar(page)).toHaveCount(0);
});

test('the bar fits the screen, stays clear of the add button, and gives a finger 44 pixels', async ({
  page,
}, info) => {
  await openView(page, info, BOARD, 'Status board');
  await select(page, info, [card(page, 'Build'), card(page, 'Test')], async () => {
    await page.getByRole('button', { name: 'Card menu: Build' }).click();
  });
  const box = await bar(page).boundingBox();
  const viewport = page.viewportSize();
  if (!box || !viewport) throw new Error('the bar is not on the page');
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(viewport.width);
  expect(box.y + box.height).toBeLessThanOrEqual(viewport.height);
  const sideways = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(sideways).toBe(0);
  expect(await bar(page).evaluate((element) => element.scrollWidth - element.clientWidth)).toBe(0);

  const add = page.getByTestId('new-card').last();
  if (await add.isVisible()) {
    const button = await add.boundingBox();
    if (!button) throw new Error('the add button has no box');
    const apart =
      box.y + box.height <= button.y ||
      button.y + button.height <= box.y ||
      box.x + box.width <= button.x ||
      button.x + button.width <= box.x;
    expect(apart).toBe(true);
  }

  if (touch(info)) {
    for (const button of await bar(page).getByRole('button').all()) {
      if (!(await button.isVisible())) continue;
      const size = await button.boundingBox();
      expect(size?.width).toBeGreaterThanOrEqual(44);
      expect(size?.height).toBeGreaterThanOrEqual(44);
    }
  }
});

test('a property chosen from the menu of the bar opens its editor, and where its button is hidden Tab passes it', async ({
  page,
}, info) => {
  await openView(page, info, BOARD, 'Status board');
  await select(page, info, [card(page, 'Build'), card(page, 'Test')], async () => {
    await page.getByRole('button', { name: 'Card menu: Build' }).click();
  });
  await bar(page).locator('[data-bulk-menu]').click();
  const chosen = page.locator('[data-action^="property-"]').first();
  const id = ((await chosen.getAttribute('data-action')) ?? '').replace('property-', '');
  await chosen.click();
  const opener = bar(page).locator(`[data-bulk-property="${id}"]`);
  await expect(opener).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('Escape');
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
  // Where the view is narrow ("More") the button the editor opens from is hidden, and no stop for
  // Tab: the focus goes back to "More". Where it is wide ("Properties") it is a button of the bar
  // like the others.
  if (await opener.evaluate((element) => element.classList.contains('sr-only'))) {
    await expect(opener).toHaveAttribute('tabindex', '-1');
    await expect(bar(page).locator('[data-bulk-menu]')).toBeFocused();
  } else {
    await expect(opener).toBeVisible();
    await expect(opener).not.toHaveAttribute('tabindex', '-1');
  }
});
