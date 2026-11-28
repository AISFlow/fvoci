/**
 * The quick filters of a board and the filter of a view, on the app with its
 * in-memory backend: chips that narrow every view for the reader alone, a
 * date condition within a period, whether a card is done, and groups of
 * conditions; on a phone 390 pixels wide as on a desktop.
 *
 * The board is the seed's "Project schedule", seen through "Task table" (a
 * plain list of its six cards). Its rule of done is the status "Done" (only
 * "Agree on the scope" has it), its due date is "Period", and the clock is
 * held at noon on Wednesday 16 December 2026 in Seoul, so that each card's
 * period is the same distance from today on every run:
 *
 * | card                 | period             | due           |
 * |----------------------|--------------------|---------------|
 * | Agree on the scope   | 1 to 3 December    | done          |
 * | Design the screens   | 4 to 10 December   | overdue       |
 * | Build                | 11 to 24 December  | later         |
 * | Write the help pages | 11 to 18 December  | this week     |
 * | Test                 | 28 Dec to 5 Jan    | later         |
 * | Launch               | 7 January          | later         |
 *
 * The week of the 16th is the 14th to the 20th from Monday and the 13th to
 * the 19th from Sunday: the same cards are due in it either way.
 */
import { readFileSync } from 'node:fs';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { placeOf } from './address.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

/** Noon on Wednesday 16 December 2026 in Seoul. */
const NOW = new Date('2026-12-16T03:00:00Z');

interface Store {
  boardId: string;
  board: {
    cardProperties: { id: string; name: string; options: { id: string; label: string }[] }[];
    doneWhen?: unknown;
  };
  cards: { id: string; title: string }[];
  setCardProperty(cardId: string, propertyId: string, value: unknown): Promise<void>;
}
interface Dev {
  backend: {
    views(boardId: string): Promise<{ id: string; fields: { filter: unknown } }[]>;
    patchContainer(boardId: string, patch: object): Promise<unknown>;
    latency: number;
  };
  workspace: { board: Store; me: { id: string }; undoLast(): Promise<void> };
}

const header = (page: Page): Locator => page.getByTestId('view-header');
const tool = (page: Page, name: string): Locator => header(page).locator(`[data-tool="${name}"]`);
const cards = (page: Page): Locator => page.locator('[data-card]');
const quick = (page: Page): Locator => page.getByTestId('quick-filters');
const chip = (page: Page, name: string): Locator =>
  quick(page).getByRole('button', { name, exact: true });
const clearChips = (page: Page): Locator => quick(page).locator('[data-quick-clear]');
const filterEditor = (page: Page): Locator => page.getByTestId('filter-editor');
const filterRows = (scope: Locator): Locator => scope.locator('[data-filter-row]');

async function showView(page: Page, name: string): Promise<void> {
  await tool(page, 'views').click();
  await page.getByRole('menuitem', { name, exact: true }).click();
  await expect(tool(page, 'views')).toHaveText(name);
}

/**
 * The first board of the seed, through "Task table", on the held clock. The
 * seed's file names its rule of done and its due date; the world in memory
 * does not read them from it, and they are given to the board here.
 */
async function open(page: Page): Promise<void> {
  await page.clock.setFixedTime(NOW);
  await page.goto('/');
  await expect(header(page)).toBeVisible();
  await showView(page, 'Task table');
  await expect(cards(page)).toHaveCount(6);
  await giveRules(page);
}

/** Gives the open board its rule of done and its due date, as the seed's file names them. */
async function giveRules(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const { backend, workspace } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const { board, boardId } = workspace.board;
    const named = (name: string) => board.cardProperties.find((property) => property.name === name);
    const status = named('Status');
    const done = status?.options.find((option) => option.label === 'Done');
    const period = named('Period');
    if (!status || !done || !period) throw new Error('no Status, Done or Period');
    await backend.patchContainer(boardId, {
      dueDatePropertyId: period.id,
      doneWhen: { kind: 'option', propertyId: status.id, optionIds: [done.id] },
    });
  });
  await expect(quick(page).locator('[data-quick-filter]')).toHaveCount(4);
}

/**
 * The titles of the cards the view shows, top to bottom. A line starts with
 * its card's icon and title; the titles of other cards may follow it ("Comes
 * after"), so the title is the one that comes first in it.
 */
async function shown(page: Page): Promise<string[]> {
  const texts = await cards(page).allTextContents();
  const titles = [
    'Agree on the scope',
    'Design the screens',
    'Build',
    'Write the help pages',
    'Test',
    'Launch',
  ];
  return texts.map((text) => {
    const found = titles.filter((title) => text.includes(title));
    found.sort((one, other) => text.indexOf(one) - text.indexOf(other));
    return found[0] ?? text.trim();
  });
}

/** The filter of the view the address names, as the backend has it. */
function storedFilter(page: Page): Promise<unknown> {
  return page.evaluate(async ({ boardId, viewId }) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const view = (await backend.views(boardId)).find((each) => each.id === viewId);
    if (!view) throw new Error('the address names no view');
    return view.fields.filter;
  }, placeOf(page.url()));
}

/** The identifier of a property of the open board, by its name. */
function propertyId(page: Page, name: string): Promise<string> {
  return page.evaluate((wanted) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const found = store.board.cardProperties.find((property) => property.name === wanted);
    if (!found) throw new Error(`no property ${wanted}`);
    return found.id;
  }, name);
}

/** The chips kept in this browser's storage, by their keys. */
function keptChips(page: Page): Promise<Record<string, string>> {
  return page.evaluate(() =>
    Object.fromEntries(
      Object.entries(localStorage).filter(([key]) => key.startsWith('fvoci:quick-filters:')),
    ),
  );
}

/** The key the reader's chips of the open board are kept under. */
function keyOfChips(page: Page): Promise<string> {
  return page.evaluate(() => {
    const { workspace } = (window as unknown as { fvociDev: Dev }).fvociDev;
    return `fvoci:quick-filters:${workspace.me.id}:${workspace.board.boardId}`;
  });
}

/** Makes the reader the owner of one card and of no other, through the board store. */
async function ownOnly(page: Page, title: string): Promise<void> {
  await page.evaluate(async (wanted) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const store = dev.workspace.board;
    const owner = store.board.cardProperties.find((property) => property.name === 'Owner');
    if (!owner || !store.cards.some((each) => each.title === wanted)) {
      throw new Error(`no Owner, or no card "${wanted}"`);
    }
    for (const card of store.cards) {
      await store.setCardProperty(
        card.id,
        owner.id,
        card.title === wanted ? dev.workspace.me.id : undefined,
      );
    }
  }, title);
}

/** Undoes the last change, once the backend has answered everything sent before. */
async function undo(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const { backend, workspace } = (window as unknown as { fvociDev: Dev }).fvociDev;
    await new Promise((resolve) => setTimeout(resolve, backend.latency + 20));
    await workspace.undoLast();
  });
}

/** Gives a line of the filter its property, and its condition when one is given. */
async function setRow(
  page: Page,
  row: Locator,
  property: string,
  condition?: string,
): Promise<void> {
  await row.locator('[data-slot="filter-property"]').click();
  await page.getByRole('menuitem', { name: property, exact: true }).click();
  await expect(row.locator('[data-slot="filter-property"]')).toContainText(property);
  if (condition === undefined) return;
  await row.locator('[data-slot="filter-condition"]').click();
  await page.getByRole('menuitem', { name: condition, exact: true }).click();
  await expect(row.locator('[data-slot="filter-condition"]')).toContainText(condition);
}

async function addCondition(page: Page, property: string, condition?: string): Promise<Locator> {
  const rows = filterRows(filterEditor(page));
  const before = await rows.count();
  await filterEditor(page).locator('[data-tool="add-filter"]').click();
  await expect(rows).toHaveCount(before + 1);
  const row = rows.nth(before);
  await setRow(page, row, property, condition);
  return row;
}

async function choosePeriod(page: Page, row: Locator, period: string): Promise<void> {
  await row.locator('[data-slot="filter-period"]').click();
  await page.getByRole('menuitem', { name: period, exact: true }).click();
  await expect(row.locator('[data-slot="filter-period"]')).toContainText(period);
}

test.describe('quick filters', () => {
  test.beforeEach(async ({ page }) => {
    await open(page);
  });

  test('narrow every view for the reader alone, all of them at once, and stay on after a reload', async ({
    page,
  }) => {
    await expect(quick(page)).toHaveAccessibleName('Quick filters');
    await expect(quick(page).locator('[data-quick-filter]')).toHaveText([
      'Mine',
      'Not done',
      'Overdue',
      'Due this week',
    ]);
    const before = await storedFilter(page);

    await chip(page, 'Overdue').click();
    await expect(chip(page, 'Overdue')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => shown(page)).toEqual(['Design the screens']);
    await chip(page, 'Due this week').click();
    await expect(cards(page)).toHaveCount(0);
    await chip(page, 'Overdue').click();
    await expect(chip(page, 'Overdue')).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(() => shown(page)).toEqual(['Write the help pages']);
    await expect(clearChips(page)).toHaveAccessibleName('Clear quick filters (1 on)');
    // Kept in this browser under the reader and the board: their names, nothing else.
    expect(await keptChips(page)).toEqual({ [await keyOfChips(page)]: '["dueThisWeek"]' });
    await chip(page, 'Not done').click();
    await expect.poll(() => shown(page)).toEqual(['Write the help pages']);
    await expect(clearChips(page)).toContainText('2');
    // The view is not changed: its filter is everyone's.
    expect(await storedFilter(page)).toEqual(before);
    await expect(tool(page, 'filter').locator('[data-count]')).toHaveCount(0);

    // Another visit to the board finds them on.
    await page.reload();
    await expect(header(page)).toBeVisible();
    // The world in memory is made anew with the page.
    await giveRules(page);
    await expect(chip(page, 'Due this week')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => shown(page)).toEqual(['Write the help pages']);

    // Another view of the board shows the cards they leave.
    await showView(page, 'Status board');
    await expect(cards(page).filter({ hasText: 'Write the help pages' })).toBeVisible();
    await expect(cards(page)).toHaveCount(1);

    await clearChips(page).click();
    await expect(cards(page)).toHaveCount(6);
    await expect(clearChips(page)).toHaveCount(0);
    // The focus is on the first chip: the button pressed is gone.
    await expect(chip(page, 'Mine')).toBeFocused();
    expect(await keptChips(page)).toEqual({});
  });

  test('"Mine" leaves the reader’s cards, and a card made while chips are on stays', async ({
    page,
  }) => {
    await ownOnly(page, 'Build');
    await chip(page, 'Mine').click();
    await expect.poll(() => shown(page)).toEqual(['Build']);
    await chip(page, 'Overdue').click();
    await expect(cards(page)).toHaveCount(0);

    // A new card is nobody's and has no period: it is not given values to meet the chips, and stays.
    await page.locator('[data-testid="new-card"]:visible').first().click();
    await expect(page.getByTestId('card-dialog')).toBeVisible();
    await expect(page.getByTestId('card-title')).toBeFocused();
    await page.getByTestId('card-title').fill('Fresh');
    await page
      .locator('[data-testid="card-close"]:visible, [data-testid="card-back"]:visible')
      .click();
    await expect(page.getByTestId('card-dialog')).toHaveCount(0);
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page)).toContainText('Fresh');
    await expect(chip(page, 'Mine')).toHaveAttribute('aria-pressed', 'true');
  });

  test('the file of the view has the cards the chips leave', async ({ page }) => {
    await chip(page, 'Overdue').click();
    await expect(cards(page)).toHaveCount(1);
    await tool(page, 'more').click();
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('menuitem', { name: 'Export to CSV' }).click(),
    ]);
    const saved = test.info().outputPath(download.suggestedFilename());
    await download.saveAs(saved);
    // A line ends in CRLF: the header, the card, and nothing after them.
    const lines = readFileSync(saved, 'utf8').slice(1).split('\r\n');
    expect(lines).toHaveLength(3);
    expect(lines[1]).toMatch(/^"Design the screens",/);
  });

  test('on a phone 390 pixels wide are one line that scrolls sideways, and the page does not', async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await chip(page, 'Mine').click();
    await chip(page, 'Not done').click();
    await expect(clearChips(page)).toBeVisible();
    const boxes = await quick(page)
      .locator('button')
      .evaluateAll((buttons) =>
        buttons.map((each) => {
          const { top, height } = each.getBoundingClientRect();
          return { top: Math.round(top), height };
        }),
      );
    // One line: every button on the same row.
    expect(new Set(boxes.map((box) => box.top)).size).toBe(1);
    if (testInfo.project.name !== 'desktop') {
      // A finger's height on a touch screen.
      for (const box of boxes) expect(box.height).toBeGreaterThanOrEqual(44);
    }
    const page390 = await page.evaluate(() => ({
      page: document.documentElement.scrollWidth,
      window: window.innerWidth,
    }));
    expect(page390.page).toBeLessThanOrEqual(page390.window);
    // What does not fit is reached by scrolling the row.
    await clearChips(page).scrollIntoViewIfNeeded();
    await expect(clearChips(page)).toBeInViewport();
    await clearChips(page).click();
    await expect(cards(page)).toHaveCount(6);
  });
});

test('a public link offers the chips but "Mine", and keeps them for the visit alone', async ({
  page,
}) => {
  const boardId = '01930000-0000-7000-8000-00000000bb01';
  // Chips this browser keeps for the board, of a member, and under an empty reader.
  const KEPT = {
    [`fvoci:quick-filters:someone:${boardId}`]: '["incomplete"]',
    [`fvoci:quick-filters::${boardId}`]: '["overdue"]',
  };
  await page.addInitScript((kept) => {
    for (const [key, value] of Object.entries(kept)) localStorage.setItem(key, value);
  }, KEPT);
  const base = `/api/v1/shared/${boardId}`;
  const STAMPS = { createdBy: '', modifiedBy: '', createdAt: 1, updatedAt: 1 };
  await page.route('**/api/v1/**', async (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname === base) {
      await route.fulfill({
        json: {
          ...STAMPS,
          id: boardId,
          workspaceId: '',
          kind: 'board',
          title: 'Open plan',
          visibility: 'private',
          icon: '',
          description: '',
          cardProperties: [
            { id: 'status', name: 'Status', type: 'select', options: [] },
            { id: 'owner', name: 'Owner', type: 'person', options: [] },
            { id: 'due', name: 'Due', type: 'date', options: [] },
          ],
          dueDatePropertyId: 'due',
          doneWhen: { propertyId: 'status', optionIds: ['finished'] },
          properties: {},
        },
      });
    } else if (pathname === `${base}/blocks`) {
      await route.fulfill({
        json: [
          {
            ...STAMPS,
            id: '01930000-0000-7000-8000-00000000bb02',
            containerId: boardId,
            parentId: '',
            type: 'view',
            title: 'Table',
            body: null,
            fields: { viewType: 'table' },
          },
        ],
      });
    } else if (pathname === `${base}/cards`) {
      await route.fulfill({ json: [] });
    } else {
      await route.fulfill({ status: 404, json: { detail: 'not found' } });
    }
  });
  await page.goto(`/shared/${boardId}?r=tok`);
  await expect(header(page)).toBeVisible();
  await expect(quick(page).locator('[data-quick-filter]')).toHaveText([
    'Not done',
    'Overdue',
    'Due this week',
  ]);
  // None of them is read: the reader of the link is nobody this browser knows.
  for (const each of await quick(page).locator('[data-quick-filter]').all()) {
    await expect(each).toHaveAttribute('aria-pressed', 'false');
  }
  await chip(page, 'Due this week').click();
  await expect(chip(page, 'Due this week')).toHaveAttribute('aria-pressed', 'true');
  // Nor is anything written.
  expect(await keptChips(page)).toEqual(KEPT);
});

test.describe('the filter of a view', () => {
  test.beforeEach(async ({ page }) => {
    await open(page);
    await tool(page, 'filter').click();
    await expect(filterEditor(page)).toBeVisible();
  });

  test('a date within a period of the reader’s, and whether a card is done', async ({ page }) => {
    const period = await addCondition(page, 'Period', 'is within');
    await expect(period.locator('[data-slot="filter-period"]')).toContainText('Choose a period');
    await expect(cards(page)).toHaveCount(6);
    await choosePeriod(page, period, 'This week');
    // A period is within the week when it has a day of it.
    await expect.poll(() => shown(page)).toEqual(['Build', 'Write the help pages']);
    expect(await storedFilter(page)).toEqual({
      operation: 'and',
      filters: [
        {
          propertyId: await propertyId(page, 'Period'),
          condition: 'isWithin',
          values: ['thisWeek'],
        },
      ],
    });
    await choosePeriod(page, period, 'Last 7 days');
    await expect
      .poll(() => shown(page))
      .toEqual(['Design the screens', 'Build', 'Write the help pages']);
    await choosePeriod(page, period, 'Next week');
    await expect.poll(() => shown(page)).toEqual(['Build']);

    // Whether a card is done, by the board's rule.
    await setRow(page, period, 'Done');
    await expect(period.locator('[data-slot="filter-condition"]')).toContainText('is done');
    await expect.poll(() => shown(page)).toEqual(['Agree on the scope']);
    await period.locator('[data-slot="filter-condition"]').click();
    await expect(page.getByRole('menuitem')).toHaveText(['is done', 'is not done']);
    await page.getByRole('menuitem', { name: 'is not done', exact: true }).click();
    await expect(cards(page)).toHaveCount(5);
    expect(await storedFilter(page)).toEqual({
      operation: 'and',
      filters: [{ propertyId: 'done', condition: 'isNotSet', values: [] }],
    });
  });

  test('a group: not done, and this week or a title with "Test"; taken apart, then back by undo', async ({
    page,
  }) => {
    await addCondition(page, 'Done', 'is not done');
    await expect(cards(page)).toHaveCount(5);
    await filterEditor(page).locator('[data-tool="add-group"]').click();
    const box = filterEditor(page).locator('[data-filter-group]');
    await expect(box).toHaveCount(1);
    const first = filterRows(box).first();
    await setRow(page, first, 'Period', 'is within');
    await choosePeriod(page, first, 'This week');
    await expect.poll(() => shown(page)).toEqual(['Build', 'Write the help pages']);

    await box.locator('[data-slot="add-to-group"]').click();
    await expect(filterRows(box)).toHaveCount(2);
    const second = filterRows(box).nth(1);
    await setRow(page, second, 'Title', 'contains');
    const text = second.getByRole('textbox', { name: 'Value for Title' });
    await text.fill('Test');
    await text.press('Enter');
    // The group asks any of its conditions; the filter, all of its parts.
    await expect(
      box.getByRole('radiogroup', { name: 'What a card has to meet in group 1' }),
    ).toBeVisible();
    await expect.poll(() => shown(page)).toEqual(['Build', 'Write the help pages', 'Test']);
    await expect(tool(page, 'filter').locator('[data-count]')).toHaveText('3');

    // Taken apart, its conditions are the filter's: all have to hold.
    await box.locator('[data-slot="group-menu"]').click();
    await page.getByRole('menuitem', { name: 'Ungroup', exact: true }).click();
    await expect(box).toHaveCount(0);
    await expect(filterRows(filterEditor(page))).toHaveCount(3);
    await expect(cards(page)).toHaveCount(0);

    await undo(page);
    await expect(box).toHaveCount(1);
    await expect.poll(() => shown(page)).toEqual(['Build', 'Write the help pages', 'Test']);
  });
});

test.describe('on a phone 390 pixels wide', () => {
  // The window is that size from the start: an emulated iPhone resized under an open sheet
  // leaves the sheet where the page no longer takes a press.
  test.use({ viewport: { width: 390, height: 844 } });

  test('a group and its lines fold to fit, none wider than the screen', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'mobile', 'a phone');
    await open(page);
    await tool(page, 'filter').click();
    await expect(filterEditor(page)).toBeVisible();
    await filterEditor(page).locator('[data-tool="add-group"]').click();
    const box = filterEditor(page).locator('[data-filter-group]');
    const row = filterRows(box).first();
    await setRow(page, row, 'Period', 'is within');
    await choosePeriod(page, row, 'Next 30 days');
    await box.locator('[data-slot="add-to-group"]').click();
    await expect(filterRows(box)).toHaveCount(2);

    const editor = await filterEditor(page).evaluate((element) => ({
      right: element.getBoundingClientRect().right,
      fits: element.scrollWidth <= element.clientWidth,
    }));
    expect(editor.fits).toBe(true);
    expect(editor.right).toBeLessThanOrEqual(390);
    for (const each of await box.locator('[data-filter-row], [data-slot]').all()) {
      const bounds = await each.boundingBox();
      if (bounds) expect(bounds.x + bounds.width).toBeLessThanOrEqual(editor.right + 0.5);
    }
    // A line of the group folds in two: the property and the condition, then the value.
    const property = await row.locator('[data-slot="filter-property"]').boundingBox();
    const value = await row.locator('[data-slot="filter-value"]').boundingBox();
    expect(value?.y ?? 0).toBeGreaterThan(property?.y ?? 0);
  });
});
