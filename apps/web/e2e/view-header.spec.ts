/**
 * The header of a view, on the app with its in-memory backend: the whole row
 * and the folded one, the views and what is done to them, the properties a
 * view shows, grouping, the filter, the sort, and search.
 *
 * The three projects are the three stages of a page. Unless a test says
 * otherwise it runs in all of them, and reaches each tool the way that
 * layout offers it: by its own button in the whole row, through "…" in the
 * folded one.
 *
 * The seed is the same on every load. The first board of the sidebar is
 * "Project schedule": six cards, the views "Gantt" (first in the list),
 * "Milestones", "Month calendar", "Status board" (a board) and "Task table"
 * (a table without grouping, where the cards are a plain list; the tests that
 * count and order cards use it).
 */
import { mkdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';
import { boardPath, placeOf } from './address.ts';
import { screensDir } from './screens.ts';

interface StoredView {
  id: string;
  title: string;
  fields: {
    viewType: string;
    groupBy?: string;
    shownProperties: string[];
    visibleGroups: string[];
    hiddenGroups: string[];
    sort: { propertyId: string; reversed: boolean }[];
    filter: { operation: string; filters: unknown[] };
    columnWidths: Record<string, number>;
    /** Its place among the board's views, once one of them was moved. */
    order?: string;
  };
}

interface Dev {
  backend: {
    workspaces: () => Promise<{ id: string }[]>;
    boards: (workspaceId: string) => Promise<{ id: string; title: string }[]>;
    container: (containerId: string) => Promise<{
      cardProperties: {
        id: string;
        name: string;
        type: string;
        options: { id: string; label: string }[];
      }[];
    }>;
    views: (boardId: string) => Promise<StoredView[]>;
    patchView: (boardId: string, viewId: string, patch: object) => Promise<unknown>;
    deleteBlock: (boardId: string, blockId: string) => Promise<void>;
    /** How long it takes to answer, in milliseconds. */
    latency: number;
  };
  workspace: { undoLast: () => Promise<void> };
}

const header = (page: Page): Locator => page.getByTestId('view-header');
const tool = (page: Page, name: string): Locator => header(page).locator(`[data-tool="${name}"]`);
const cards = (page: Page): Locator => page.locator('[data-card]');
const layout = (page: Page): Promise<string | null> => header(page).getAttribute('data-layout');

/**
 * Written while the shell was a stand-in that kept its list of boards beside
 * the page at every width. The shell that is there now makes the list a
 * drawer on a compact page, so this finds nothing to take out; it stays for
 * a page that should ever lay the list beside a pane that narrow.
 */
async function givePaneItsRoom(page: Page): Promise<void> {
  await page.evaluate(() => {
    const shell = document.querySelector('[data-testid="app-shell"]');
    const main = shell?.querySelector('main');
    const list = shell?.querySelector<HTMLElement>(':scope > nav');
    if (!main || !list) return;
    if (list.dataset.takenOut !== undefined) {
      list.style.display = '';
      delete list.dataset.takenOut;
    }
    if (window.innerWidth < 600 && main.clientWidth < window.innerWidth - 100) {
      list.style.display = 'none';
      list.dataset.takenOut = '';
    }
  });
}

async function open(page: Page, address = '/'): Promise<void> {
  await page.goto(address);
  await expect(header(page)).toBeVisible();
  await givePaneItsRoom(page);
}

async function resize(page: Page, width: number, height: number): Promise<void> {
  await page.setViewportSize({ width, height });
  await givePaneItsRoom(page);
}

/** Makes the window as wide as gives the pane this width, whatever the sidebar takes. */
async function setPaneWidth(page: Page, width: number): Promise<void> {
  const size = page.viewportSize();
  if (!size) throw new Error('no viewport');
  const pane = await header(page).evaluate((element) => element.getBoundingClientRect().width);
  await page.setViewportSize({ width: Math.round(size.width + width - pane), height: size.height });
  await expect
    .poll(() => header(page).evaluate((element) => element.getBoundingClientRect().width))
    .toBe(width);
}

/** The address of a board of the seed, found by its title. */
async function boardAddress(page: Page, title: string): Promise<string> {
  const id = await page.evaluate(async (wanted) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    if (!workspace) throw new Error('no workspace');
    return (await backend.boards(workspace.id)).find((board) => board.title === wanted)?.id;
  }, title);
  if (!id) throw new Error(`no board called ${title}`);
  return boardPath(id);
}

/** The view the address names, as the backend has it. */
function storedView(page: Page): Promise<StoredView> {
  return page.evaluate(async ({ boardId, viewId }) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const view = (await backend.views(boardId)).find((each) => each.id === viewId);
    if (!view) throw new Error('the address names no view');
    return view;
  }, placeOf(page.url()));
}

/** The properties of the board the address names, by name. */
function properties(
  page: Page,
): Promise<Record<string, { id: string; type: string; options: Record<string, string> }>> {
  return page.evaluate(async (boardId) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const board = await backend.container(boardId);
    return Object.fromEntries(
      board.cardProperties.map((property) => [
        property.name,
        {
          id: property.id,
          type: property.type,
          options: Object.fromEntries(property.options.map((option) => [option.label, option.id])),
        },
      ]),
    );
  }, placeOf(page.url()).boardId);
}

/** Changes the view the address names as the backend, without the screen: for setting a scene. */
async function patchView(page: Page, updatedFields: object): Promise<void> {
  await page.evaluate(
    async ({ fields, boardId, viewId }) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      await backend.patchView(boardId, viewId, { updatedFields: fields });
    },
    { fields: updatedFields, ...placeOf(page.url()) },
  );
}

/**
 * Undoes the last change. A change can be undone once the backend has
 * answered it, and the in-memory one takes its time to answer; a timer of the
 * page set now, for longer than that, rings after the answers of everything
 * that was sent before.
 */
async function undo(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const { backend, workspace } = (window as unknown as { fvociDev: Dev }).fvociDev;
    await new Promise((resolve) => setTimeout(resolve, backend.latency + 20));
    await workspace.undoLast();
  });
}

/** Shows another view of the board, through the list behind the button of the views. */
async function showView(page: Page, name: string): Promise<void> {
  await tool(page, 'views').click();
  await page.getByRole('menuitem', { name, exact: true }).click();
  await expect(tool(page, 'views')).toHaveText(name);
}

/** The views of the first board of the seed, by title: nobody has moved one. */
const SEED_VIEWS = ['Gantt', 'Milestones', 'Month calendar', 'Status board', 'Task table'];

/** The views in the open list: links to them. */
const listed = (page: Page): Locator =>
  page.getByRole('menu', { name: 'Views' }).locator('a[role="menuitem"]');

/**
 * The views of the board the address names as the backend keeps them, by
 * their places: every one has a place once one of them was moved.
 */
function storedOrder(page: Page): Promise<(string | undefined)[]> {
  return page.evaluate(async (boardId) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const views = await backend.views(boardId);
    if (views.some((view) => view.fields.order === undefined)) return [undefined];
    const by = (view: StoredView): string => `${view.fields.order} ${view.id}`;
    return views.sort((one, other) => (by(one) < by(other) ? -1 : 1)).map((view) => view.title);
  }, placeOf(page.url()).boardId);
}

/** Presses Tab until the element has the focus, a few times at most. */
async function tabTo(page: Page, target: Locator): Promise<void> {
  for (let press = 0; press < 16; press += 1) {
    if (await target.evaluate((element) => element === document.activeElement)) return;
    await page.keyboard.press('Tab');
  }
  await expect(target).toBeFocused();
}

/** Opens a tool: by its own button where the row has one, through "…" otherwise. */
async function openTool(
  page: Page,
  name: 'properties' | 'group' | 'filter' | 'sort',
): Promise<void> {
  if (name === 'filter' || (await layout(page)) === 'row') {
    await tool(page, name).click();
  } else {
    await tool(page, 'more').click();
    await page.locator(`[role="menuitem"][data-action="${name}"]`).click();
  }
}

/** A line of the list of views that changes the view shown. */
async function viewCommand(page: Page, name: 'rename' | 'duplicate' | 'delete'): Promise<void> {
  await tool(page, 'views').click();
  await page.locator(`[role="menuitem"][data-action="${name}"]`).click();
}

/** Closes what is open on top: a popover, a menu or a sheet. */
async function closeTop(page: Page): Promise<void> {
  await page.keyboard.press('Escape');
}

const sheet = (page: Page): Locator => page.getByTestId('bottom-sheet');
const filterEditor = (page: Page): Locator => page.getByTestId('filter-editor');
const sortEditor = (page: Page): Locator => page.getByTestId('sort-editor');
const filterRows = (page: Page): Locator => filterEditor(page).locator('[data-filter-row]');

/** Adds a condition to the open filter editor and puts it on a property. */
async function addCondition(page: Page, property: string, condition?: string): Promise<Locator> {
  const before = await filterRows(page).count();
  await filterEditor(page).locator('[data-tool="add-filter"]').click();
  await expect(filterRows(page)).toHaveCount(before + 1);
  const row = filterRows(page).nth(before);
  await row.locator('[data-slot="filter-property"]').click();
  await page.getByRole('menuitem', { name: property, exact: true }).click();
  await expect(row.locator('[data-slot="filter-property"]')).toContainText(property);
  if (condition !== undefined) {
    await row.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: condition, exact: true }).click();
    await expect(row.locator('[data-slot="filter-condition"]')).toContainText(condition);
  }
  return row;
}

async function addSort(page: Page, property: string): Promise<void> {
  await sortEditor(page).locator('[data-tool="add-sort"]').click();
  await page.getByRole('menuitem', { name: property, exact: true }).click();
  await expect(sortEditor(page).locator('[data-sort]').filter({ hasText: property })).toBeVisible();
}

/** The titles of the cards the view shows, top to bottom. */
async function titles(page: Page): Promise<string[]> {
  return (await cards(page).allTextContents()).map((text) => text.trim());
}

/** A card is found by its title in whatever else its line shows. */
const containing = (text: string): unknown => expect.stringContaining(text);

const ALL = [
  'Agree on the scope',
  'Design the screens',
  'Build',
  'Write the help pages',
  'Test',
  'Launch',
];

test.describe('the two rows', () => {
  test('a pane of 704 pixels has the whole header in one row, and nothing spills', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'needs a window that can be this wide');
    await open(page);
    await showView(page, 'Status board');
    // 1280 wide, the sidebar beside it: the pane is 1024.
    await expect(header(page)).toHaveAttribute('data-layout', 'row');

    // The name of the view on its button, whole.
    const cut = (): Promise<boolean> =>
      tool(page, 'views')
        .locator('span')
        .evaluate((name) => name.scrollWidth > name.clientWidth);
    await setPaneWidth(page, 992);
    await expect(tool(page, 'views')).toHaveText('Status board');
    expect(await cut()).toBe(false);

    await setPaneWidth(page, 704);
    await expect(header(page)).toHaveAttribute('data-layout', 'row');
    for (const name of ['views', 'properties', 'group', 'filter', 'sort', 'search']) {
      await expect(tool(page, name)).toBeVisible();
    }
    // No tabs, and every tool of a board has its own button in the row: "…" holds only the file.
    await expect(header(page).getByRole('link')).toHaveCount(0);
    await tool(page, 'more').click();
    await expect(page.getByRole('menuitem')).toHaveText(['Export to CSV']);
    await page.keyboard.press('Escape');
    await expect(tool(page, 'views')).toHaveText('Status board');
    expect(await cut()).toBe(false);
    // "New" is in the row, and the row is as wide as its content.
    await expect(header(page).getByRole('button', { name: 'New', exact: true })).toBeVisible();
    const spill = await header(page).evaluate((element) => {
      const edge = element.getBoundingClientRect().right;
      const beyond = [...element.querySelectorAll('a, button, input')].filter(
        (each) => each.getBoundingClientRect().right > edge + 0.5,
      );
      return { scroll: element.scrollWidth - element.clientWidth, beyond: beyond.length };
    });
    expect(spill).toEqual({ scroll: 0, beyond: 0 });
    expect(await header(page).evaluate((element) => element.getBoundingClientRect().height)).toBe(
      48,
    );

    // One pixel less and it folds.
    await setPaneWidth(page, 703);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
  });

  test('views of long names are each whole in the list, and the one shown is named on the button', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'needs a window that can be this wide');
    await open(page);
    await setPaneWidth(page, 992);
    // Long names for all five views of the board.
    const { boardId, viewId } = placeOf(page.url());
    const given = await page.evaluate(async (id) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const titles: Record<string, string> = {};
      for (const [at, view] of (await backend.views(id)).entries()) {
        titles[view.id] = `A longer view name ${String(at + 1)}`;
        await backend.patchView(id, view.id, { title: titles[view.id] });
      }
      return titles;
    }, boardId);
    const names = [1, 2, 3, 4, 5].map((at) => `A longer view name ${String(at)}`);
    await expect(tool(page, 'views')).toHaveText(given[viewId] ?? '');
    await tool(page, 'views').click();
    await expect(listed(page)).toHaveText(names);
    // No line is cut, and nothing spills.
    const fit = await header(page).evaluate((element) => ({
      cut: [...document.querySelectorAll('[role="menu"] a[role="menuitem"] span')].filter(
        (each) => each.scrollWidth > each.clientWidth,
      ).length,
      scroll: element.scrollWidth - element.clientWidth,
    }));
    expect(fit).toEqual({ cut: 0, scroll: 0 });

    // The last view, from the list: the button names it, and the list marks it.
    await listed(page).last().click();
    await expect(tool(page, 'views')).toHaveText('A longer view name 5');
    await tool(page, 'views').click();
    await expect(listed(page).last()).toHaveAttribute('aria-current', 'page');
    await expect(listed(page).first()).not.toHaveAttribute('aria-current');
  });

  test('a narrower pane folds the header, and "New" becomes a round button at the bottom', async ({
    page,
  }, testInfo) => {
    await open(page);
    await showView(page, 'Status board');
    // A pane of 644: a window of 900 with the sidebar beside the board, or a device as
    // wide as an upright tablet (WebKit's "middle" is 834) made that much narrower.
    if (testInfo.project.name === 'desktop') await resize(page, 900, 720);
    else if ((await layout(page)) === 'row') await setPaneWidth(page, 644);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');

    await expect(tool(page, 'views')).toContainText('Status board');
    for (const name of ['search', 'filter', 'more']) await expect(tool(page, name)).toBeVisible();
    for (const name of ['properties', 'group', 'sort']) await expect(tool(page, name)).toBeHidden();
    await expect(header(page).getByRole('link')).toHaveCount(0);

    const add = page.getByTestId('new-card');
    await expect(add).toBeVisible();
    await expect(add).toHaveAccessibleName('New');
    const place = await add.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const board = element.closest('[data-testid="board-page"]')?.getBoundingClientRect();
      return {
        width: box.width,
        height: box.height,
        round: getComputedStyle(element).borderRadius,
        fromRight: (board?.right ?? 0) - box.right,
        fromBottom: (board?.bottom ?? 0) - box.bottom,
        inHeader: element.closest('[data-testid="view-header"]') !== null,
      };
    });
    expect(place).toMatchObject({ width: 56, height: 56, fromRight: 16, fromBottom: 16 });
    expect(place.inHeader).toBe(false);
    expect(parseFloat(place.round)).toBeGreaterThanOrEqual(28);

    // It makes a card and opens it, like "New" in the row.
    await add.click();
    await expect(page.getByTestId('card-dialog')).toBeVisible();
  });

  test('a pane that gets wider unfolds the header, and folds it again when it narrows', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'needs a window that can be this wide');
    await open(page);
    await resize(page, 900, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
    // The sidebar cannot be folded away yet; a wider window gives the pane the same room.
    await resize(page, 1300, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'row');
    await expect(page.getByTestId('new-card')).toHaveText(/New/);
    await resize(page, 900, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
  });
});

test.describe('in Korean', () => {
  test.use({ locale: 'ko-KR' });

  test('the whole row has its names in Korean, and does not spill at 704 pixels', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'needs a window that can be this wide');
    await open(page);
    await showView(page, 'Status board');
    await setPaneWidth(page, 704);
    await expect(header(page)).toHaveAttribute('data-layout', 'row');
    // Icons, each named for a screen reader and by its tooltip.
    // With the number of properties the cards show (excluding relation properties).
    const known = new Set(
      Object.values(await properties(page))
        .filter((property) => property.type !== 'relation')
        .map((property) => property.id),
    );
    const shownCount = (await storedView(page)).fields.shownProperties.filter((id) =>
      known.has(id),
    ).length;
    expect(shownCount).toBeGreaterThan(0);
    await expect(tool(page, 'properties')).toHaveAccessibleName(`속성 (${String(shownCount)})`);
    await expect(tool(page, 'properties')).toHaveAttribute('title', `속성 (${String(shownCount)})`);
    await expect(tool(page, 'group')).toHaveAccessibleName(/그룹화 기준: Status/);
    await expect(tool(page, 'filter')).toHaveAccessibleName('필터');
    await expect(tool(page, 'sort')).toHaveAccessibleName('정렬');
    await tool(page, 'search').click();
    await expect(header(page).getByRole('searchbox', { name: '카드 검색' })).toBeFocused();
    await expect(page.getByTestId('new-card')).toHaveText(/새로 만들기/);
    const spill = await header(page).evaluate((element) => {
      const edge = element.getBoundingClientRect().right;
      const beyond = [...element.querySelectorAll('a, button, input')].filter(
        (each) => each.getBoundingClientRect().right > edge + 0.5,
      );
      return { scroll: element.scrollWidth - element.clientWidth, beyond: beyond.length };
    });
    expect(spill).toEqual({ scroll: 0, beyond: 0 });

    // The words of the editors are Korean too.
    await tool(page, 'filter').click();
    await expect(filterEditor(page)).toContainText('조건이 없습니다');
    await filterEditor(page).locator('[data-tool="add-filter"]').click();
    await expect(filterRows(page).first().locator('[data-slot="filter-condition"]')).toContainText(
      '포함함',
    );
  });
});

test.describe('views', () => {
  test('choosing a view shows it and changes the address', async ({ page }) => {
    await open(page);
    // An address without a view shows the first, and leaves the focus where it was.
    await expect(page.locator('body')).toBeFocused();
    await expect(tool(page, 'views')).not.toBeFocused();
    const first = page.url();
    await showView(page, 'Task table');
    await expect(page).not.toHaveURL(first);
    const overview = page.url();
    expect((await storedView(page)).title).toBe('Task table');

    // The address is the view: loaded on its own it shows the same one.
    await open(page, overview);
    expect((await storedView(page)).title).toBe('Task table');
    await expect(tool(page, 'views')).toHaveText('Task table');
    await tool(page, 'views').click();
    await expect(listed(page).filter({ hasText: 'Task table' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    // A line of the list is a link to its view.
    const href = await listed(page).filter({ hasText: 'Gantt' }).getAttribute('href');
    expect(new URL(href ?? '', page.url()).href).toBe(first);
    await closeTop(page);

    await page.goBack();
    await expect(page).toHaveURL(first);
  });

  test('a view is added, renamed, copied and deleted', async ({ page }) => {
    await open(page);
    const start = page.url();
    const everyProperty = Object.values(await properties(page)).map((property) => property.id);

    // Added: a table that shows every property.
    await tool(page, 'views').click();
    await page.getByRole('menuitem', { name: 'Add table view' }).click();
    await expect(page).not.toHaveURL(start);
    await expect.poll(async () => (await storedView(page)).title).toBe('Table view');
    const added = await storedView(page);
    expect(added.fields.viewType).toBe('table');
    expect(added.fields.shownProperties).toEqual(everyProperty);
    // No width is stored for the title column: the table gives it one by its room.
    expect(added.fields.columnWidths).toEqual({});

    // Renamed.
    await viewCommand(page, 'rename');
    const name = page.getByRole('textbox', { name: 'View name' });
    await expect(name).toHaveValue('Table view');
    await name.fill('Mine');
    await name.press('Enter');
    await expect(name).toBeHidden();
    await expect.poll(async () => (await storedView(page)).title).toBe('Mine');
    await expect(header(page)).toContainText('Mine');

    // Escape leaves the name as it was.
    await viewCommand(page, 'rename');
    await name.fill('Not this');
    await name.press('Escape');
    await expect(name).toBeHidden();
    expect((await storedView(page)).title).toBe('Mine');

    // Copied: the copy is shown.
    const original = page.url();
    await viewCommand(page, 'duplicate');
    await expect(page).not.toHaveURL(original);
    await expect.poll(async () => (await storedView(page)).title).toBe('Mine copy');
    expect((await storedView(page)).fields.shownProperties).toEqual(everyProperty);

    // Deleted, after asking: another view of the board is shown.
    const copy = page.url();
    await viewCommand(page, 'delete');
    const confirm = page.getByTestId('confirm-dialog');
    await expect(confirm).toContainText('Delete “Mine copy”?');
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirm).toBeHidden();
    expect(page.url()).toBe(copy);

    await viewCommand(page, 'delete');
    await confirm.getByRole('button', { name: 'Delete' }).click();
    await expect(page).not.toHaveURL(copy);
    await expect(header(page)).not.toContainText('Mine copy');
    await expect(header(page)).toBeVisible();

    // Undo brings the view back.
    await undo(page);
    await showView(page, 'Mine copy');
    expect(page.url()).toBe(copy);
  });

  test('a line of the list opens its view in a new tab, by a held key, the middle button or the keys', async ({
    page,
    context,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a mouse and its keys');
    await open(page);
    const first = page.url();
    await tool(page, 'views').click();
    const line = listed(page).filter({ hasText: 'Task table' });
    const href = new URL((await line.getAttribute('href')) ?? '', page.url()).href;

    const [held] = await Promise.all([
      context.waitForEvent('page'),
      line.click({ modifiers: ['ControlOrMeta'] }),
    ]);
    await expect(held).toHaveURL(href);
    await expect(held.getByTestId('view-header').locator('[data-tool="views"]')).toHaveText(
      'Task table',
    );
    // This tab shows what it showed.
    expect(page.url()).toBe(first);
    await expect(tool(page, 'views')).toHaveText('Gantt');
    await held.close();

    await tool(page, 'views').click();
    const [middle] = await Promise.all([
      context.waitForEvent('page'),
      line.click({ button: 'middle' }),
    ]);
    await expect(middle).toHaveURL(href);
    expect(page.url()).toBe(first);
    await middle.close();

    // From the keys: Enter with Ctrl (⌘ on a Mac) on the line, as on a link.
    await page.keyboard.press('Escape');
    await expect(page.getByRole('menu', { name: 'Views' })).toHaveCount(0);
    await tool(page, 'views').focus();
    await page.keyboard.press('Enter');
    await expect(listed(page).first()).toBeFocused();
    for (const _ of [1, 2, 3, 4]) await page.keyboard.press('ArrowDown');
    await expect(line).toBeFocused();
    const [keyed] = await Promise.all([
      context.waitForEvent('page'),
      page.keyboard.press('ControlOrMeta+Enter'),
    ]);
    await expect(keyed).toHaveURL(href);
    expect(page.url()).toBe(first);
    await expect(tool(page, 'views')).toHaveText('Gantt');
  });

  test('a view is renamed, copied and deleted by the keys alone', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard');
    await open(page);
    const views = tool(page, 'views');
    await views.focus();
    /** Opens the list from the button and goes to the line whose name starts with `letters`. */
    const choose = async (keys: string[], name: string): Promise<void> => {
      await expect(views).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(listed(page).first()).toBeFocused();
      for (const key of keys) await page.keyboard.press(key);
      await expect(page.getByRole('menuitem', { name })).toBeFocused();
      await page.keyboard.press('Enter');
    };

    // By its letters. (The list keeps letters typed for a second, also into its next
    // opening: the lines after this are reached by End and the arrows.)
    await choose(['r', 'e', 'n'], 'Rename view');
    const name = page.getByRole('textbox', { name: 'View name' });
    await expect(name).toBeFocused();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('Typed');
    await page.keyboard.press('Enter');
    await expect(name).toBeHidden();
    await expect.poll(async () => (await storedView(page)).title).toBe('Typed');
    await expect(views).toHaveText('Typed');

    const renamed = page.url();
    await choose(['End', 'ArrowUp'], 'Duplicate view');
    await expect(page).not.toHaveURL(renamed);
    await expect(views).toHaveText('Typed copy');
    await expect(page.getByTestId('view-shown')).toHaveText('Now showing: Typed copy');

    const copy = page.url();
    // Asked, and let be: the focus is back on the button.
    await choose(['End'], 'Delete view');
    await expect(page.getByRole('alertdialog')).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('alertdialog')).toHaveCount(0);
    await expect(views).toBeFocused();
    expect(page.url()).toBe(copy);

    await choose(['End'], 'Delete view');
    // The question has the keys, then Cancel, then Delete.
    const confirm = page.getByTestId('confirm-dialog');
    await expect(page.getByRole('alertdialog')).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(confirm.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(confirm.getByRole('button', { name: 'Delete' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page).not.toHaveURL(copy);
    await expect(views).not.toHaveText('Typed copy');
    await expect(views).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(listed(page).filter({ hasText: 'Typed copy' })).toHaveCount(0);
  });

  test('a view is moved down and up the list by the keys alone, the list staying open', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard');
    await open(page);
    const views = tool(page, 'views');
    await views.focus();
    await page.keyboard.press('Enter');
    await expect(listed(page)).toHaveText(SEED_VIEWS);
    const line = (name: string): Locator => listed(page).filter({ hasText: name });
    await expect(line('Gantt')).toBeFocused();
    // Alt and an arrow on the line, which keeps the focus where it goes.
    await page.keyboard.press('Alt+ArrowDown');
    const second = ['Milestones', 'Gantt', 'Month calendar', 'Status board', 'Task table'];
    await expect(listed(page)).toHaveText(second);
    await expect(line('Gantt')).toBeFocused();
    await expect(page.getByText('Gantt is now 2 of 5')).toBeAttached();
    await page.keyboard.press('Alt+ArrowDown');
    await expect(listed(page)).toHaveText([
      'Milestones',
      'Month calendar',
      'Gantt',
      'Status board',
      'Task table',
    ]);
    await expect(page.getByText('Gantt is now 3 of 5')).toBeAttached();
    await page.keyboard.press('Alt+ArrowUp');
    await expect(listed(page)).toHaveText(second);
    await expect(line('Gantt')).toBeFocused();
    await expect.poll(() => storedOrder(page)).toEqual(second);
    // The arrows alone go from line to line, past the buttons at their ends.
    await page.keyboard.press('ArrowDown');
    await expect(line('Month calendar')).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await page.keyboard.press('ArrowUp');
    await expect(line('Milestones')).toBeFocused();
    // At the top, Alt and up goes nowhere.
    await page.keyboard.press('Alt+ArrowUp');
    await expect(listed(page)).toHaveText(second);

    // Closed, the list keeps the order and says nothing of the moves when it opens again.
    await closeTop(page);
    await expect(views).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(listed(page)).toHaveText(second);
    await expect(page.getByText('Gantt is now 2 of 5')).toHaveCount(0);
    // And the view shown is still the one it was.
    await closeTop(page);
    await expect(views).toHaveText('Gantt');
  });

  test('a view is moved by the buttons at the end of its line, the list staying open', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a mouse');
    await open(page);
    await tool(page, 'views').click();
    const menu = page.getByRole('menu', { name: 'Views' });
    const down = menu.locator('[data-line-tool][data-action="move-down"]').first();
    await expect(down).toHaveAttribute('title', 'Move Gantt down (Alt+↓)');
    await down.click();
    await expect(listed(page)).toHaveText([
      'Milestones',
      'Gantt',
      'Month calendar',
      'Status board',
      'Task table',
    ]);
    await expect(listed(page).filter({ hasText: 'Gantt' })).toBeFocused();
    // A name of the list is whole beside its buttons.
    expect(
      await menu.evaluate(
        (element) =>
          [...element.querySelectorAll('a[role="menuitem"] span')].filter(
            (each) => each.scrollWidth > each.clientWidth,
          ).length,
      ),
    ).toBe(0);
  });

  test('on a compact page the sheet of views moves one by the keys alone', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard');
    await open(page);
    await resize(page, 390, 844);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
    const views = tool(page, 'views');
    const shown = (await views.textContent())?.trim() ?? '';
    await views.focus();
    await page.keyboard.press('Enter');
    await expect(sheet(page)).toBeVisible();
    // It names the view the lines after the views change, and the board.
    await expect(sheet(page).getByText(`Project schedule · showing ${shown}`)).toBeVisible();
    const lines = sheet(page).locator('a[role="menuitem"]');
    await expect(lines).toHaveText(SEED_VIEWS);
    // Tab reaches the buttons of the sheet, which keep the focus.
    const down = sheet(page).getByRole('menuitem', { name: 'Move Month calendar down' });
    // Its tooltip, for a mouse in a narrow window, says the keys too: not its name again.
    await expect(down).toHaveAttribute('title', 'Move Month calendar down (Alt+↓)');
    await tabTo(page, down);
    await page.keyboard.press('Enter');
    await expect(lines).toHaveText([
      'Gantt',
      'Milestones',
      'Status board',
      'Month calendar',
      'Task table',
    ]);
    await expect(down).toBeFocused();
    await expect(sheet(page).getByText('Month calendar is now 4 of 5')).toBeAttached();
    await page.keyboard.press('Enter');
    await expect(lines.last()).toHaveText('Month calendar');
    // At the end, down is no way: the focus goes to up, which is.
    await expect(down).toBeDisabled();
    const up = sheet(page).getByRole('menuitem', { name: 'Move Month calendar up' });
    await expect(up).toBeFocused();
    // Pressed twice before the first is answered: the word of the first, coming
    // after the second is shown, does not put the list back, nor take the focus.
    await page.keyboard.press('Enter');
    await page.keyboard.press('Enter');
    await expect(lines).toHaveText([
      'Gantt',
      'Milestones',
      'Month calendar',
      'Status board',
      'Task table',
    ]);
    await expect(sheet(page).getByText('Month calendar is now 3 of 5')).toBeAttached();
    await expect(up).toBeFocused();
    await expect.poll(() => storedOrder(page)).toEqual(SEED_VIEWS);
    await expect(up).toBeFocused();
    // And Alt and an arrow on a line, as in the dropdown.
    const first = lines.filter({ hasText: 'Gantt' });
    await first.focus();
    await page.keyboard.press('Alt+ArrowDown');
    await expect(lines.nth(1)).toHaveText('Gantt');
    await expect(first).toBeFocused();
    await expect
      .poll(() => storedOrder(page))
      .toEqual(['Milestones', 'Gantt', 'Month calendar', 'Status board', 'Task table']);
  });

  test('the last view of a board cannot be deleted', async ({ page }) => {
    await open(page);
    await open(page, await boardAddress(page, 'Meeting notes'));
    const { boardId, viewId } = placeOf(page.url());
    await page.evaluate(
      async ({ board, keep }) => {
        const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
        for (const view of await backend.views(board)) {
          if (view.id !== keep) await backend.deleteBlock(board, view.id);
        }
      },
      { board: boardId, keep: viewId },
    );
    await tool(page, 'views').click();
    await expect(page.locator('[role="menuitem"][data-action="delete"]')).toBeDisabled();
    await expect(page.locator('[role="menuitem"][data-action="duplicate"]')).toBeEnabled();
  });
});

test.describe('what a view shows', () => {
  test('properties are switched on and off, each as it is pressed', async ({ page }) => {
    await open(page);
    await showView(page, 'Status board');
    const phase = (await properties(page)).Phase?.id ?? '';
    expect((await storedView(page)).fields.shownProperties).not.toContain(phase);

    await openTool(page, 'properties');
    const panel = page.getByTestId('properties-panel');
    const toggle = panel.getByRole('switch', { name: 'Phase' });
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    const owner = panel.getByRole('switch', { name: 'Owner' });
    await expect(owner).toHaveAttribute('aria-checked', 'true');
    // The list has the non-relation properties of the board and nothing else: a card always shows
    // its title and the marks of a description and of a checklist, so neither has a switch.
    await expect(panel.getByRole('switch', { name: 'Comments and description' })).toHaveCount(0);
    await expect(panel.getByRole('switch', { name: 'Title' })).toHaveCount(0);
    expect(await panel.getByRole('switch').count()).toBe(
      Object.values(await properties(page)).filter((property) => property.type !== 'relation')
        .length,
    );

    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
    await expect.poll(async () => (await storedView(page)).fields.shownProperties).toContain(phase);
    // The list stays open for the next one.
    await owner.click();
    await expect(owner).toHaveAttribute('aria-checked', 'false');

    // Each switch was one change: one undo brings Owner back, the next takes Phase out.
    await undo(page);
    await expect(owner).toHaveAttribute('aria-checked', 'true');
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
    await undo(page);
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect((await storedView(page)).fields.shownProperties).not.toContain(phase);
  });

  test('grouping by another property starts its columns over, and one undo takes it back', async ({
    page,
  }) => {
    await open(page);
    await showView(page, 'Status board');
    const all = await properties(page);
    const before = await storedView(page);
    expect(before.fields.groupBy).toBe(all.Status?.id);
    // Hide a column, so that there is something for the change to clear.
    const firstStatus = Object.values(all.Status?.options ?? {})[0] ?? '';
    await patchView(page, {
      visibleGroups: before.fields.visibleGroups.filter((id) => id !== firstStatus),
      hiddenGroups: [firstStatus],
    });
    const hidden = await storedView(page);

    await openTool(page, 'group');
    const choices = page.getByRole('menuitem');
    // A board is always grouped: there is no "none" to pick.
    await expect(choices.filter({ hasText: 'Ungroup' })).toHaveCount(0);
    await expect(choices.filter({ hasText: 'Estimated hours' })).toHaveCount(0);
    await choices.filter({ hasText: 'Phase' }).click();
    // Picking one closes the list.
    await expect(choices).toHaveCount(0);

    await expect.poll(async () => (await storedView(page)).fields.groupBy).toBe(all.Phase?.id);
    const after = await storedView(page);
    expect(after.fields.visibleGroups).toEqual(['', ...Object.values(all.Phase?.options ?? {})]);
    expect(after.fields.hiddenGroups).toEqual([]);
    if ((await layout(page)) === 'row') {
      await expect(tool(page, 'group')).toHaveAccessibleName(/Group by: Phase/);
    }

    await undo(page);
    await expect.poll(async () => (await storedView(page)).fields.groupBy).toBe(all.Status?.id);
    const undone = await storedView(page);
    expect(undone.fields.visibleGroups).toEqual(hidden.fields.visibleGroups);
    expect(undone.fields.hiddenGroups).toEqual([firstStatus]);
  });

  test('a table can be grouped by nothing', async ({ page }) => {
    await open(page);
    await showView(page, 'Task table');
    const all = await properties(page);
    expect((await storedView(page)).fields.groupBy ?? '').toBe('');

    await openTool(page, 'group');
    await page.getByRole('menuitem', { name: 'Status', exact: true }).click();
    await expect.poll(async () => (await storedView(page)).fields.groupBy).toBe(all.Status?.id);

    await openTool(page, 'group');
    await page.getByRole('menuitem', { name: 'Ungroup', exact: true }).click();
    await expect.poll(async () => (await storedView(page)).fields.groupBy).toBe('');
    expect((await storedView(page)).fields.visibleGroups).toEqual([]);
  });
});

test.describe('filter', () => {
  test.beforeEach(async ({ page }) => {
    await open(page);
    await showView(page, 'Task table');
    await expect(cards(page)).toHaveCount(6);
  });

  test('options, text, a date and emptiness each change how many cards there are', async ({
    page,
  }) => {
    await openTool(page, 'filter');
    await expect(filterEditor(page)).toContainText('No conditions');

    // Options: ticked in a list that stays open.
    const phase = await addCondition(page, 'Phase');
    await expect(phase.locator('[data-slot="filter-condition"]')).toContainText('includes');
    await expect(cards(page)).toHaveCount(6);
    await phase.locator('[data-slot="property-select"]').click();
    await page.getByRole('option', { name: 'Planning' }).click();
    await expect(cards(page)).toHaveCount(2);
    await page.getByRole('option', { name: 'Launch' }).click();
    await expect(cards(page)).toHaveCount(3);
    await closeTop(page);
    await expect(filterEditor(page)).toBeVisible();
    await expect(tool(page, 'filter').locator('[data-count]')).toHaveText('1');

    // The same options, the other way round.
    await phase.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: "doesn't include", exact: true }).click();
    await expect(cards(page)).toHaveCount(3);

    // Emptiness has nothing to compare with.
    await phase.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: 'is empty', exact: true }).click();
    await expect(cards(page)).toHaveCount(0);
    await expect(phase.locator('[data-slot="property-select"]')).toHaveCount(0);
    await phase.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: 'is not empty', exact: true }).click();
    await expect(cards(page)).toHaveCount(6);

    // Another property: the condition becomes one its kind of value has.
    await phase.locator('[data-slot="filter-property"]').click();
    await page.getByRole('menuitem', { name: 'Title', exact: true }).click();
    const title = filterRows(page).first();
    await expect(title.locator('[data-slot="filter-condition"]')).toContainText('is');
    await title.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: 'contains', exact: true }).click();
    const text = title.getByRole('textbox', { name: 'Value for Title' });
    await text.fill('THE SC');
    await text.press('Enter');
    await expect(cards(page)).toHaveCount(2);
    // In whatever order: a line starts with the icon of its card.
    expect(await titles(page)).toEqual(
      expect.arrayContaining([
        expect.stringContaining('Agree on the scope'),
        expect.stringContaining('Design the screens'),
      ]),
    );

    // A date: a day picked from the calendar, on a board with both an updated-time and a date property.
    await closeTop(page);
    await open(page, await boardAddress(page, 'Customer contacts'));
    await showView(page, 'All customers');
    await expect(cards(page)).toHaveCount(5);
    await openTool(page, 'filter');
    const changed = await addCondition(page, 'Last changed at', 'is after');
    await changed.locator('[data-slot="filter-date"]').click();
    await page
      .locator('[data-slot="filter-date-picker"]')
      .getByRole('button', { name: 'Today', exact: true })
      .click();
    await expect(changed.locator('[data-slot="filter-date"]')).not.toContainText('Empty');
    await expect(cards(page)).toHaveCount(0);
    await changed.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: 'is before', exact: true }).click();
    await expect(cards(page)).toHaveCount(5);
    const stored = (await storedView(page)).fields.filter.filters[0] as { values: string[] };
    expect(stored.values[0]).toMatch(/^\d{4}-\d{2}-\d{2}$/);

    // Three of the five cards have a next contact date.
    await changed.locator('[data-slot="filter-property"]').click();
    await page.getByRole('menuitem', { name: 'Next contact', exact: true }).click();
    await changed.locator('[data-slot="filter-condition"]').click();
    await page.getByRole('menuitem', { name: 'is set', exact: true }).click();
    await expect(cards(page)).toHaveCount(3);
    await expect(changed.locator('[data-slot="filter-date"]')).toHaveCount(0);
  });

  test('several conditions: all of them, or any one; and undo, a step at a time', async ({
    page,
  }) => {
    await openTool(page, 'filter');
    const phase = await addCondition(page, 'Phase');
    await phase.locator('[data-slot="property-select"]').click();
    await page.getByRole('option', { name: 'Building' }).click();
    await closeTop(page);
    await expect(cards(page)).toHaveCount(3);
    // With one condition there is nothing to choose between.
    await expect(filterEditor(page).getByRole('radiogroup')).toHaveCount(0);

    const title = await addCondition(page, 'Title', 'contains');
    const text = title.getByRole('textbox', { name: 'Value for Title' });
    await text.fill('the');
    await text.press('Enter');
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page)).toContainText('Write the help pages');
    await expect(tool(page, 'filter').locator('[data-count]')).toHaveText('2');

    const match = filterEditor(page).getByRole('radiogroup');
    await expect(match.getByRole('radio', { name: 'All conditions' })).toBeChecked();
    await match.getByRole('radio', { name: 'Any condition' }).click();
    await expect(cards(page)).toHaveCount(5);
    expect((await storedView(page)).fields.filter.operation).toBe('or');

    // Undo: back to "all", then the text goes, then the condition on the title.
    await undo(page);
    await expect(cards(page)).toHaveCount(1);
    await expect(match.getByRole('radio', { name: 'All conditions' })).toBeChecked();
    await undo(page);
    await expect(cards(page)).toHaveCount(3);
    await expect(text).toHaveValue('');

    // A person: the members of the board, ticked like options.
    await title.locator('[data-slot="filter-property"]').click();
    await page.getByRole('menuitem', { name: 'Owner', exact: true }).click();
    await title.locator('[data-slot="property-person"]').click();
    const people = page.getByRole('option');
    await expect(people.first()).toBeVisible();
    expect(await people.count()).toBeGreaterThanOrEqual(3);
    await people.first().click();
    await closeTop(page);
    const owner = (await storedView(page)).fields.filter.filters[1] as { values: string[] };
    expect(owner.values).toHaveLength(1);
  });

  test('a group of conditions inside the filter is said to be there, and survives every edit', async ({
    page,
  }) => {
    const all = await properties(page);
    const nested = {
      operation: 'or',
      filters: [
        { propertyId: 'title', condition: 'contains', values: ['scope'] },
        { propertyId: 'title', condition: 'contains', values: ['help'] },
      ],
    };
    await patchView(page, { filter: { operation: 'and', filters: [nested] } });
    await expect(cards(page)).toHaveCount(2);
    await expect(tool(page, 'filter').locator('[data-count]')).toHaveText('2');

    await openTool(page, 'filter');
    await expect(page.getByTestId('filter-nested')).toContainText('Nested groups of conditions: 1');
    await expect(filterRows(page)).toHaveCount(0);

    // Add a condition, change it, change how they combine, remove it: the group is as it was.
    const phase = await addCondition(page, 'Phase');
    await phase.locator('[data-slot="property-select"]').click();
    await page.getByRole('option', { name: 'Building' }).click();
    await closeTop(page);
    await expect(cards(page)).toHaveCount(1);
    expect((await storedView(page)).fields.filter).toEqual({
      operation: 'and',
      filters: [
        nested,
        {
          propertyId: all.Phase?.id,
          condition: 'includes',
          values: [all.Phase?.options.Building],
        },
      ],
    });
    await filterEditor(page).getByRole('radio', { name: 'Any condition' }).click();
    await expect(cards(page)).toHaveCount(4);
    await phase.locator('[data-remove]').click();
    await expect(filterRows(page)).toHaveCount(0);
    expect((await storedView(page)).fields.filter).toEqual({ operation: 'or', filters: [nested] });
    await expect(cards(page)).toHaveCount(2);
  });
});

test.describe('sort', () => {
  test('several sorts apply in their order, each in its direction', async ({ page }) => {
    await open(page);
    await showView(page, 'Task table');
    await patchView(page, { sort: [] });
    await expect.poll(async () => (await storedView(page)).fields.sort).toEqual([]);
    await expect(cards(page)).toHaveCount(6);
    const all = await properties(page);

    await openTool(page, 'sort');
    await expect(sortEditor(page)).toContainText('No sort');
    await expect(page.getByTestId('sort-notice')).toHaveCount(0);

    await addSort(page, 'Title');
    await expect.poll(() => titles(page)).toEqual([...ALL].sort().map(containing));
    // A table is one of the views whose cards are dragged: the editor says what the sort turns off.
    await expect(page.getByTestId('sort-notice')).toContainText('cannot be dragged');
    await expect(tool(page, 'sort').locator('[data-count]')).toHaveText('1');

    // Reversed.
    const byTitle = sortEditor(page).locator('[data-sort="__title"]');
    await byTitle.getByRole('button', { name: /Title: Ascending/ }).click();
    await expect(byTitle.getByRole('button', { name: /Title: Descending/ })).toBeVisible();
    await expect.poll(() => titles(page)).toEqual([...ALL].sort().reverse().map(containing));

    // A second sort only orders what the first leaves equal: here nothing.
    await addSort(page, 'Phase');
    await expect.poll(() => titles(page)).toEqual([...ALL].sort().reverse().map(containing));
    expect((await storedView(page)).fields.sort).toEqual([
      { propertyId: '__title', reversed: true },
      { propertyId: all.Phase?.id, reversed: false },
    ]);

    // Moved up, the phase decides first and the title, still reversed, within it.
    const byPhase = sortEditor(page).locator(`[data-sort="${all.Phase?.id ?? ''}"]`);
    await expect(byPhase.getByRole('button', { name: 'Move Phase down' })).toBeDisabled();
    await byPhase.getByRole('button', { name: 'Move Phase up' }).click();
    await expect(sortEditor(page).getByRole('status')).toHaveText('Phase is now number 1 of 2.');
    await expect(byPhase.getByRole('button', { name: 'Move Phase up' })).toBeDisabled();
    // The focus went with the line, to the button that still works.
    await expect(byPhase.getByRole('button', { name: 'Move Phase down' })).toBeFocused();
    await expect
      .poll(() => titles(page))
      .toEqual(
        [
          'Write the help pages',
          'Test',
          'Build',
          'Launch',
          'Design the screens',
          'Agree on the scope',
        ].map(containing),
      );

    // A property is sorted by once: it is no longer offered.
    await sortEditor(page).locator('[data-tool="add-sort"]').click();
    await expect(page.getByRole('menuitem', { name: 'Status', exact: true })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Phase', exact: true })).toHaveCount(0);
    await closeTop(page);

    // Removed: the title alone again. The focus is not left on a button that is gone.
    await byPhase.getByRole('button', { name: 'Remove the sort by Phase' }).click();
    await expect(byPhase).toHaveCount(0);
    await expect(sortEditor(page).locator('[data-tool="add-sort"]')).toBeFocused();
    await expect.poll(() => titles(page)).toEqual([...ALL].sort().reverse().map(containing));

    await undo(page);
    await expect(sortEditor(page).locator('[data-sort]')).toHaveCount(2);
  });
});

test.describe('search', () => {
  test('narrows the cards as it is typed; closing it brings them back', async ({ page }) => {
    await open(page);
    await showView(page, 'Task table');
    await expect(cards(page)).toHaveCount(6);
    const folded = (await layout(page)) === 'folded';
    // By its element: while the folded row has it away, it has no role to be found by.
    const field = header(page).locator('input[type="search"]');

    // Away until its button is pressed, in either row.
    await expect(field).toBeHidden();
    await tool(page, 'search').click();
    await expect(field).toBeFocused();
    if (folded) {
      // The row is the search field now.
      await expect(header(page).getByRole('searchbox', { name: 'Search cards' })).toBeVisible();
      const covered = await header(page).evaluate((element) => {
        const search = element.querySelector('[data-testid="view-search"]');
        const onTop = (selector: string): boolean => {
          const box = element.querySelector(selector)?.getBoundingClientRect();
          if (!box) return false;
          const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
          return search?.contains(hit) ?? false;
        };
        return {
          wide: search?.getBoundingClientRect().width === element.getBoundingClientRect().width,
          views: onTop('[data-tool="views"]'),
          filter: onTop('[data-tool="filter"]') || !element.querySelector('[data-tool="filter"]'),
        };
      });
      expect(covered).toEqual({ wide: true, views: true, filter: true });
    }

    await field.pressSequentially('the sc');
    await expect(cards(page)).toHaveCount(2);
    await field.pressSequentially('o');
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page)).toContainText('Agree on the scope');
    // Search is the reader's own: nothing of it is stored in the view.
    expect(JSON.stringify(await storedView(page))).not.toContain('the sco');

    if (folded) {
      await header(page).getByRole('button', { name: 'Close search' }).click();
      await expect(field).toBeHidden();
      await expect(tool(page, 'search')).toBeFocused();
      await expect(tool(page, 'views')).toBeVisible();
    } else {
      // In the whole row the field is where its button was, and the button of the views
      // stays as it was; Escape gives the search its button back.
      await expect(tool(page, 'search')).toHaveCount(0);
      await expect(tool(page, 'views')).toHaveText('Task table');
      await field.press('Escape');
      await expect(field).toBeHidden();
      await expect(tool(page, 'search')).toBeFocused();
    }
    await expect(field).toHaveValue('');
    await expect(cards(page)).toHaveCount(6);

    // Another view starts without the search.
    await tool(page, 'search').click();
    await expect(field).toBeFocused();
    await field.fill('scope');
    await expect(cards(page)).toHaveCount(1);
    if (folded) {
      // While the field covers the row, what is under it is out of reach.
      expect(
        await tool(page, 'more').evaluate((element) => element.closest('[inert]') !== null),
      ).toBe(true);
      await page.goBack();
    } else {
      await showView(page, 'Status board');
    }
    await expect(field).toHaveValue('');
  });
});

test.describe('a compact page', () => {
  test('menus are sheets, and the back button closes them', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'mobile', 'the compact stage');
    await open(page);
    await showView(page, 'Task table');
    const address = page.url();

    // The list of views.
    await tool(page, 'views').click();
    await expect(sheet(page)).toBeVisible();
    await expect(
      sheet(page).getByRole('menuitem', { name: 'Task table', exact: true }),
    ).toBeVisible();
    // Its buttons that move a view are what a finger presses: 44 pixels at least.
    const up = sheet(page).getByRole('menuitem', { name: 'Move Task table up' });
    expect((await up.boundingBox())?.height).toBeGreaterThanOrEqual(44);
    await page.goBack();
    await expect(sheet(page)).toHaveCount(0);
    expect(page.url()).toBe(address);

    // The filter. Nothing in it has the focus: the keyboard waits to be asked for.
    await tool(page, 'filter').click();
    await expect(sheet(page)).toBeVisible();
    await expect(sheet(page)).toContainText('Filter');
    await expect(sheet(page)).toBeFocused();
    const title = await addCondition(page, 'Title', 'contains');
    // A sheet over the sheet, for the list of properties, came and went.
    await expect(sheet(page)).toHaveCount(1);
    // The line is folded in two: the value is under the property, not beside it.
    const boxes = await title.evaluate((row) => ({
      property: row.querySelector('[data-slot="filter-property"]')?.getBoundingClientRect().bottom,
      value: row.querySelector('[data-slot="filter-value"]')?.getBoundingClientRect().top,
    }));
    expect(boxes.value).toBeGreaterThanOrEqual(boxes.property ?? Infinity);
    // What a finger presses is 44 pixels high.
    for (const slot of ['filter-property', 'filter-condition']) {
      const box = await title.locator(`[data-slot="${slot}"]`).boundingBox();
      expect(box?.height).toBeGreaterThanOrEqual(44);
    }
    const text = title.getByRole('textbox', { name: 'Value for Title' });
    expect(await text.evaluate((element) => parseFloat(getComputedStyle(element).fontSize))).toBe(
      16,
    );
    await text.fill('scope');
    await text.press('Enter');
    await expect(cards(page)).toHaveCount(1);
    await page.goBack();
    await expect(sheet(page)).toHaveCount(0);
    expect(page.url()).toBe(address);
    await expect(cards(page)).toHaveCount(1);

    // "…" and, from it, the sort: one sheet takes the place of the other, and back closes it.
    await tool(page, 'more').click();
    await expect(sheet(page)).toContainText('View menu');
    await sheet(page).locator('[data-action="sort"]').click();
    await expect(sortEditor(page)).toBeVisible();
    await expect(sheet(page)).toHaveCount(1);
    await addSort(page, 'Title');
    await page.goBack();
    await expect(sheet(page)).toHaveCount(0);
    expect(page.url()).toBe(address);
    // One more step back leaves the view, as it would have without the sheets.
    await page.goBack();
    await expect(page).not.toHaveURL(address);
    await expect(header(page)).toBeVisible();

    // "Done" closes a sheet too.
    await tool(page, 'filter').click();
    await sheet(page).getByRole('button', { name: 'Done' }).click();
    await expect(sheet(page)).toHaveCount(0);
  });
});

test.describe('a board the reader may only look at', () => {
  test('has the views and search, and nothing that changes the view', async ({ page }) => {
    await open(page);
    await open(page, await boardAddress(page, 'Study notes'));
    for (const name of ['properties', 'group', 'filter', 'sort', 'rename']) {
      await expect(tool(page, name)).toHaveCount(0);
    }
    await expect(page.getByTestId('new-card')).toHaveCount(0);
    // "…" saves the view as a file, which changes nothing of it, and holds nothing else.
    await tool(page, 'more').click();
    await expect(page.getByRole('menuitem')).toHaveText(['Export to CSV']);
    await page.keyboard.press('Escape');

    await showView(page, 'By subject');
    expect((await storedView(page)).title).toBe('By subject');
    // The list switches views and offers nothing else: its lines are the five views.
    await tool(page, 'views').click();
    await expect(page.getByRole('menuitem')).toHaveCount(5);
    await expect(page.locator('a[role="menuitem"]')).toHaveCount(5);
    // Nor are they moved.
    await expect(page.locator('[data-line-tool]')).toHaveCount(0);
    await closeTop(page);
    await tool(page, 'search').click();
    const before = await cards(page).count();
    expect(before).toBeGreaterThan(1);
    await header(page).getByRole('searchbox', { name: 'Search cards' }).fill('index');
    await expect.poll(() => cards(page).count()).toBeLessThan(before);
    await expect(cards(page).first()).toContainText('Why an index is fast');
  });
});

test.describe('while the window changes size', () => {
  test('what is open stays open, and what was typed stays typed', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'starts wide and goes through every stage');
    await open(page);
    await showView(page, 'Task table');
    await tool(page, 'search').click();
    const field = header(page).getByRole('searchbox', { name: 'Search cards' });
    await field.fill('e');
    await expect(cards(page)).toHaveCount(6);
    await field.fill('the sc');
    await expect(cards(page)).toHaveCount(2);

    // The filter open in the whole row, with a text typed and not yet saved.
    await openTool(page, 'filter');
    const title = await addCondition(page, 'Title', 'contains');
    const text = title.getByRole('textbox', { name: 'Value for Title' });
    await text.pressSequentially('scope');
    expect((await storedView(page)).fields.filter.filters).toEqual([
      { propertyId: 'title', condition: 'contains', values: [] },
    ]);

    // Narrower: the row folds. The popover is the same one, and so is the field in it.
    await text.evaluate((element) => {
      (window as unknown as { kept: Element }).kept = element;
    });
    await resize(page, 900, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
    await expect(filterEditor(page)).toBeVisible();
    await expect(page.locator('[data-slot="popover-content"]')).toHaveCount(1);
    await expect(text).toHaveValue('scope');
    await expect(text).toBeFocused();
    expect(
      await text.evaluate((element) => (window as unknown as { kept: Element }).kept === element),
    ).toBe(true);
    // The search text is there too, and the folded row shows it.
    await expect(field).toHaveValue('the sc');
    await expect(field).toBeVisible();

    // Compact: the popover becomes a sheet. It is built anew; the text was saved on the way.
    await resize(page, 500, 720);
    await expect(sheet(page)).toBeVisible();
    await expect(page.locator('[data-slot="popover-content"]')).toHaveCount(0);
    await expect(filterEditor(page)).toBeVisible();
    await expect(text).toHaveValue('scope');
    // The field is another element than before, and the focus did not come along:
    // to keep those, the shared surfaces (`PickerSurface`, `ActionMenu`) would
    // have to draw their content once for the sheet and the popover, not once in
    // each.
    expect(
      await text.evaluate((element) => (window as unknown as { kept: Element }).kept === element),
    ).toBe(false);
    await expect(text).not.toBeFocused();
    await expect(field).toHaveValue('the sc');
    await expect(cards(page)).toHaveCount(1);
    expect((await storedView(page)).fields.filter.filters).toEqual([
      { propertyId: 'title', condition: 'contains', values: ['scope'] },
    ]);

    // And back to a popover, and to the whole row.
    await resize(page, 700, 720);
    await expect(sheet(page)).toHaveCount(0);
    await expect(page.locator('[data-slot="popover-content"]')).toHaveCount(1);
    await expect(text).toHaveValue('scope');
    await resize(page, 1280, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'row');
    await expect(filterEditor(page)).toBeVisible();
    await expect(field).toHaveValue('the sc');
    // The popover is next to the button it belongs to.
    const gap = await page.evaluate(() => {
      const button = document.querySelector('[data-tool="filter"]')?.getBoundingClientRect();
      const popover = document
        .querySelector('[data-slot="popover-content"]')
        ?.getBoundingClientRect();
      return button && popover ? popover.top - button.bottom : undefined;
    });
    expect(gap).toBeGreaterThanOrEqual(0);
    expect(gap).toBeLessThan(16);
  });

  test('a tool opened from "…" stays open, next to "…" or to its own button', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'starts wide and goes through every stage');
    await open(page);
    await showView(page, 'Status board');
    await resize(page, 900, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'folded');
    await openTool(page, 'sort');
    await expect(sortEditor(page)).toBeVisible();
    const anchored = (name: string): Promise<{ below: number; across: number } | undefined> =>
      page.evaluate((selector) => {
        const button = document.querySelector(selector)?.getBoundingClientRect();
        const popover = document
          .querySelector('[data-slot="popover-content"]')
          ?.getBoundingClientRect();
        if (!button || !popover) return undefined;
        return {
          below: popover.top - button.bottom,
          across: Math.max(button.left - popover.right, popover.left - button.right),
        };
      }, `[data-tool="${name}"]`);
    // Under "…", which is where it was opened from.
    await expect.poll(async () => (await anchored('more'))?.below).toBeLessThan(16);
    expect((await anchored('more'))?.across).toBeLessThanOrEqual(0);

    // The pane widens: the sort has its own button now, and the popover is under that.
    await resize(page, 1300, 720);
    await expect(header(page)).toHaveAttribute('data-layout', 'row');
    await expect(sortEditor(page)).toBeVisible();
    await expect(tool(page, 'sort')).toBeVisible();
    await expect.poll(async () => (await anchored('sort'))?.across).toBeLessThanOrEqual(0);
    expect((await anchored('sort'))?.below).toBeLessThan(16);

    // The name of a view being typed outlives the surface it is typed on.
    await closeTop(page);
    await resize(page, 700, 720);
    await viewCommand(page, 'rename');
    const name = page.getByRole('textbox', { name: 'View name' });
    await name.fill('Half typed');
    await resize(page, 500, 720);
    await expect(sheet(page)).toBeVisible();
    await expect(name).toHaveValue('Half typed');
    await resize(page, 700, 720);
    await expect(sheet(page)).toHaveCount(0);
    await expect(name).toHaveValue('Half typed');
    expect((await storedView(page)).title).toBe('Status board');
    await name.press('Enter');
    await expect.poll(async () => (await storedView(page)).title).toBe('Half typed');
  });
});

/**
 * The pictures of the view header, into `target/screens/view-header/`
 * (`e2e/screens.ts`): every scene at the stage of the
 * project that runs, light and dark. Only with `FVOCI_SCREENS=1`:
 *
 *   FVOCI_SCREENS=1 pnpm exec playwright test e2e/view-header.spec.ts -g pictures
 */
test.describe('pictures', () => {
  test.skip(process.env.FVOCI_SCREENS !== '1', 'only when asked for');

  const STAGES: Record<string, string> = {
    mobile: 'compact',
    middle: 'medium',
    desktop: 'expanded',
  };
  const directory = screensDir('view-header');

  type Scene = (page: Page) => Promise<void>;
  const SCENES: Record<string, Scene> = {
    header: async () => undefined,
    views: async (page) => {
      await tool(page, 'views').click();
      await expect(page.getByRole('menuitem').first()).toBeVisible();
    },
    more: async (page) => {
      test.skip((await layout(page)) === 'row', 'the whole row of a table has nothing behind "…"');
      await tool(page, 'more').click();
      await expect(page.getByRole('menuitem').first()).toBeVisible();
    },
    properties: async (page) => {
      await openTool(page, 'properties');
      await expect(page.getByTestId('properties-panel')).toBeVisible();
    },
    group: async (page) => {
      await openTool(page, 'group');
      await expect(page.getByRole('menuitem').first()).toBeVisible();
    },
    filter: async (page) => {
      const all = await properties(page);
      await patchView(page, {
        filter: {
          operation: 'and',
          filters: [
            {
              propertyId: all.Phase?.id,
              condition: 'includes',
              values: [all.Phase?.options.Planning, all.Phase?.options.Building],
            },
            { propertyId: 'title', condition: 'contains', values: ['the'] },
            { propertyId: all.Period?.id, condition: 'isBefore', values: ['2026-12-24'] },
            { propertyId: all.Period?.id, condition: 'isSet', values: [] },
          ],
        },
      });
      await openTool(page, 'filter');
      await expect(filterRows(page)).toHaveCount(4);
    },
    sort: async (page) => {
      const all = await properties(page);
      await patchView(page, {
        sort: [
          { propertyId: all.Phase?.id, reversed: false },
          { propertyId: '__title', reversed: true },
        ],
      });
      await openTool(page, 'sort');
      await expect(sortEditor(page).locator('[data-sort]')).toHaveCount(2);
    },
    search: async (page) => {
      await tool(page, 'search').click();
      await header(page).getByRole('searchbox').fill('the sc');
      await expect(cards(page)).toHaveCount(2);
    },
  };

  for (const [scene, arrange] of Object.entries(SCENES)) {
    for (const scheme of ['light', 'dark'] as const) {
      test(`${scene}, ${scheme}`, async ({ page }, testInfo) => {
        const stage = STAGES[testInfo.project.name];
        if (!stage) throw new Error(`no stage for ${testInfo.project.name}`);
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        await open(page);
        await showView(page, 'Task table');
        await expect(cards(page)).toHaveCount(6);
        await arrange(page);
        await page.waitForLoadState('networkidle');
        await page.evaluate(() => document.fonts.ready);
        // Let a sheet or a popover finish arriving.
        await page.waitForTimeout(400);
        mkdirSync(directory, { recursive: true });
        await page.screenshot({
          path: path.join(directory, `${scene}-${stage}-${scheme}.png`),
          // One pixel of the picture per pixel of the layout: the files stay small.
          scale: 'css',
        });
      });
    }
  }
});

test.describe('the file of a view', () => {
  /** Saves the view shown through "…", and gives the name and the text of the file. */
  async function saveCsv(page: Page): Promise<{ name: string; text: string }> {
    await tool(page, 'more').click();
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('menuitem', { name: 'Export to CSV' }).click(),
    ]);
    // Saved where the test keeps its files: a browser run elsewhere (WebKit) has no path of it here.
    const saved = test.info().outputPath(download.suggestedFilename());
    await download.saveAs(saved);
    return { name: download.suggestedFilename(), text: readFileSync(saved, 'utf8') };
  }

  test('"…" saves the cards the view shows, named and guarded, as a spreadsheet reads them', async ({
    page,
  }) => {
    await open(page);
    await showView(page, 'Task table');
    // The reader owns "Build"; one card's title begins as a formula would.
    await page.evaluate(async () => {
      interface Store {
        board: { cardProperties: { id: string; name: string }[] };
        cards: { id: string; title: string }[];
        setCardProperty(cardId: string, propertyId: string, value: unknown): Promise<void>;
        setCardTitle?(cardId: string, title: string): Promise<void>;
      }
      const dev = (
        window as unknown as {
          fvociDev: { workspace: { board: Store; me: { id: string } } };
        }
      ).fvociDev;
      const store = dev.workspace.board;
      const owner = store.board.cardProperties.find((property) => property.name === 'Owner');
      const build = store.cards.find((card) => card.title === 'Build');
      if (!owner || !build) throw new Error('no Owner, or no card "Build"');
      await store.setCardProperty(build.id, owner.id, dev.workspace.me.id);
    });
    // The first row that names "Build" is its own; those after it come after it.
    await expect(cards(page).filter({ hasText: 'Build' }).first()).toBeVisible();

    const { name, text } = await saveCsv(page);
    // WebKit writes the spaces of a name it is given as underscores.
    expect(name.replaceAll('_', ' ')).toBe('Project schedule - Task table.csv');
    // UTF-8 said once at the start, then lines ended by CRLF.
    expect(text.startsWith('\uFEFF')).toBe(true);
    const lines = text.slice(1).split('\r\n');
    expect(lines[0]).toBe(
      '"Title","Status","Phase","Owner","Period","Comes after","Estimated hours","Milestone"',
    );
    expect(lines).toHaveLength(8);
    const build = lines.find((line) => line.startsWith('"Build"')) ?? '';
    // The owner by name, never by identifier; the card before it by its title; hours a number,
    // the only cell not quoted.
    expect(build).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-/);
    expect(build).toContain(',"Design the screens",80,"FALSE"');
    expect(build.split(',')[3]).toMatch(/^"[^"]+"$/);
    expect(lines.find((line) => line.startsWith('"Launch"'))).toMatch(/,4,"TRUE"$/);
  });

  test('the file has the cards the search leaves, in the order of the view', async ({ page }) => {
    await open(page);
    await showView(page, 'Task table');
    // In the folded row the search takes the row, "…" with it, while it is open.
    test.skip((await layout(page)) !== 'row', 'the folded row has the search or "…", not both');
    await tool(page, 'search').click();
    await header(page).getByRole('searchbox', { name: 'Search cards' }).fill('the');
    await expect.poll(() => cards(page).count()).toBeLessThan(6);
    const shown = await cards(page).count();
    const { text } = await saveCsv(page);
    const titles = text
      .slice(1)
      .split('\r\n')
      .slice(1, -1)
      .map((line) => line.split('","')[0]?.replace(/^"/, ''));
    expect(titles).toHaveLength(shown);
    for (const title of titles) expect(title?.toLowerCase()).toContain('the');
  });

  test('a public link offers no file of its view', async ({ page }) => {
    const boardId = '01930000-0000-7000-8000-00000000bb01';
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
            cardProperties: [{ id: 'status', name: 'Status', type: 'select', options: [] }],
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
    await expect(tool(page, 'views')).toHaveText('Table');
    // A reader of a public link gets no "…" at all: nothing in it is theirs to do.
    await expect(tool(page, 'more')).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Export to CSV' })).toHaveCount(0);
  });
});
