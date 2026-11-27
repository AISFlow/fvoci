/**
 * What is done to a Gantt view on the in-memory backend: grouping it, its
 * zoom, and the cards without a date. A board of 2,000 cards is in
 * gantt-many.spec.ts.
 */
import { expect, test, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import {
  arrows,
  bar,
  bars,
  dayScale,
  gantt,
  goTo,
  NOW,
  openGantt,
  openRelated,
  scroller,
  setDates,
  storedFields,
  type Backend,
  type Dev,
} from './gantt.support.ts';
import { dialog, fitPane, paneOf } from './kanban.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

interface Option {
  id: string;
  label: string;
}
interface Select {
  id: string;
  name: string;
  options: Option[];
}

/** The first select property of the open board, and the option each dated card has. */
async function grouping(page: Page): Promise<{ select: Select; optionOf: Record<string, string> }> {
  return page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const select = store.board.cardProperties.find((property) => property.type === 'select');
    if (!select) throw new Error('the board has no select property');
    const cards = store.cards as unknown as {
      title: string;
      fields: { properties: Record<string, unknown> };
    }[];
    return {
      select: select as unknown as Select,
      optionOf: Object.fromEntries(
        cards.map((card) => {
          const value = card.fields.properties[select.id];
          return [card.title, typeof value === 'string' ? value : ''];
        }),
      ),
    };
  });
}

/** Chooses a line of "group by" in the header, whichever way the header is laid out. */
async function chooseGroup(page: Page, name: string): Promise<void> {
  const header = page.getByTestId('view-header');
  if ((await header.getAttribute('data-layout')) === 'row') {
    await header.locator('[data-tool="group"]').click();
  } else {
    await header.locator('[data-tool="more"]').click();
    await page.locator('[role="menuitem"][data-action="group"]').click();
  }
  await page.getByRole('menuitem', { name, exact: true }).click();
}

test('grouped as a table is: a heading before the cards of each group, the arrows across them', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await expect(gantt(page).locator('[data-group]')).toHaveCount(0);
  const { select, optionOf } = await grouping(page);
  await chooseGroup(page, select.name);
  expect((await storedFields(page)).groupBy).toBe(select.id);

  // One heading for the cards without a value and one for each option, as in a table.
  const headings = gantt(page).locator('[data-group]');
  await expect(headings).toHaveCount(select.options.length + 1);
  const dated = [
    'Open a card',
    'Filter, sort and group',
    'Write in the body',
    'Keep your documents in a wiki',
  ];
  const groups = [{ id: '', label: `No ${select.name}` }, ...select.options];
  // In the order of a table: the cards without a value first, then the options as the board has them.
  expect(
    await headings.evaluateAll((all) => all.map((each) => each.getAttribute('data-group'))),
  ).toEqual(groups.map((option) => option.id));
  for (const option of groups) {
    const heading = gantt(page).locator(`[data-group="${option.id}"]`);
    const count = dated.filter((title) => (optionOf[title] ?? '') === option.id).length;
    await expect(heading.locator('[data-slot="group-name"]')).toHaveText(option.label);
    await expect(heading.locator('[data-slot="group-count"] [aria-hidden]')).toHaveText(
      String(count),
    );
  }
  // Each card is under the heading of its option: the rows follow the headings down the page.
  const order = await gantt(page)
    .locator('[data-group], [data-row]')
    .evaluateAll((rows) =>
      rows.map((row) =>
        row.hasAttribute('data-group')
          ? `group:${row.getAttribute('data-group') ?? ''}`
          : (row.querySelector('[data-row-title] [data-title]')?.textContent ?? '').trim(),
      ),
    );
  let under = '';
  for (const line of order) {
    if (line.startsWith('group:')) under = line.slice('group:'.length);
    else expect(optionOf[line] ?? '', `"${line}" under its group`).toBe(under);
  }
  expect(order.filter((line) => !line.startsWith('group:')).sort()).toEqual([...dated].sort());
  // The arrows are still one for each edge, whatever group their cards are in.
  await expect(arrows(page)).toHaveCount(3);
  await expect(gantt(page)).toHaveAttribute('data-edges', '3');
  // A heading is not a stop of the keys that walk the bars.
  await bars(page).first().focus();
  for (let step = 0; step < 3; step += 1) await page.keyboard.press('ArrowDown');
  await expect(bars(page).last()).toBeFocused();

  await chooseGroup(page, 'Ungroup');
  await expect(headings).toHaveCount(0);
  await expect(bars(page)).toHaveCount(4);
});

test('the zoom steps through days, weeks and months, and the view keeps it', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const line = page.getByTestId('gantt-today-line');
  const step = page.getByTestId('gantt-zoom');
  const zoomIn = page.getByTestId('gantt-zoom-in');
  const zoomOut = page.getByTestId('gantt-zoom-out');
  /** How many days a bar is wide by the marks of the axis, and whether it lies on its first day. */
  async function onDays(title: string, first: string, days: number): Promise<void> {
    await bar(page, title).scrollIntoViewIfNeeded();
    const scale = await dayScale(page);
    const box = await bar(page, title).boundingBox();
    if (!box) throw new Error(`"${title}" has no bar`);
    expect(Math.abs(box.x - scale.x(first))).toBeLessThan(1);
    expect(Math.abs(box.width - days * scale.width)).toBeLessThan(1);
  }

  /** Where the line of today is across the page; it has to be among the days, clear of the titles. */
  async function todayAt(): Promise<number> {
    const box = await line.boundingBox();
    const titles = await gantt(page).locator('[data-gantt-corner]').boundingBox();
    const days = await scroller(page).boundingBox();
    if (!box || !titles || !days) throw new Error('nothing to measure');
    expect(box.x).toBeGreaterThan(titles.x + titles.width);
    expect(box.x).toBeLessThan(days.x + days.width);
    return box.x;
  }
  const stored = async (): Promise<unknown> => (await storedFields(page)).ganttZoom;

  await expect(step).toContainText('Zoom: Weeks');
  const start = await todayAt();
  await zoomIn.click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'day');
  await expect(step).toContainText('Zoom: Days');
  await expect.poll(stored).toBe('day');
  // Today, which was in sight, stays where it was.
  await expect.poll(async () => Math.abs((await todayAt()) - start)).toBeLessThan(2);
  // A mark for each day, under the name of the month.
  const first = (await gantt(page).getAttribute('data-first')) ?? '';
  const last = (await gantt(page).getAttribute('data-last')) ?? '';
  const days = Math.round((Date.parse(last) - Date.parse(first)) / 86_400_000) + 1;
  await expect(gantt(page).locator('[data-tick]')).toHaveCount(days);
  await expect(gantt(page).locator('[data-upper="2026-11-01"]')).toHaveText('November 2026');
  await onDays('Filter, sort and group', '2026-11-12', 12);
  // The closest step: the button stays and does nothing.
  await expect(zoomIn).toHaveAttribute('aria-disabled', 'true');
  // Pressed all the same (the test's own click would wait for the button to be enabled).
  await zoomIn.click({ force: true });
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'day');

  await zoomOut.click();
  await zoomOut.click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'month');
  await expect(step).toContainText('Zoom: Months');
  await expect.poll(stored).toBe('month');
  await expect(zoomOut).toHaveAttribute('aria-disabled', 'true');
  await todayAt();
  // Months under years, on whole months.
  expect(((await gantt(page).getAttribute('data-first')) ?? '').slice(8)).toBe('01');
  await expect(gantt(page).locator('[data-upper]').first()).toHaveText('2026');
  await expect(gantt(page).locator('[data-tick="2026-11-01"]')).toHaveText('Nov');
  // A day is a fifth of a rem wide: the bar of one day is widened to be seen.
  const narrow = await bar(page, 'Open a card').boundingBox();
  expect(Math.round(narrow?.width ?? 0)).toBe(8);
  // One undo takes one step back, once the step is written.
  await page.evaluate(() => (window as unknown as { fvociDev: Dev }).fvociDev.workspace.undoLast());
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'week');
  await expect.poll(stored).toBe('week');
});

test('zooming out and back in comes back to the days the reader was at, until "Today" is asked for', async ({
  page,
}, info) => {
  await openGantt(page, info);
  // A card the summer after next: there is a long way to scroll.
  await setDates(page, { 'Keep your documents in a wiki': { from: '2028-06-01' } });
  const line = page.getByTestId('gantt-today-line');
  const left = (): Promise<number> => scroller(page).evaluate((element) => element.scrollLeft);
  const zoomed = async (press: 'gantt-zoom-in' | 'gantt-zoom-out', zoom: string): Promise<void> => {
    await page.getByTestId(press).click();
    await expect(gantt(page)).toHaveAttribute('data-zoom', zoom);
    await expect.poll(async () => (await storedFields(page)).ganttZoom).toBe(zoom);
  };
  // Away from today: a year on, among the weeks.
  await scroller(page).evaluate((element) => {
    element.scrollLeft = 4000;
  });
  await expect(line).not.toBeInViewport();
  const before = await left();
  // At months the whole chart is nearly in sight and the box is hardly scrolled.
  await zoomed('gantt-zoom-out', 'month');
  expect(await left()).toBeLessThan(before / 2);
  await zoomed('gantt-zoom-in', 'week');
  await expect.poll(async () => Math.abs((await left()) - before)).toBeLessThan(3);

  // "Today" lets go of that place, also when it does not have to move the box.
  await zoomed('gantt-zoom-out', 'month');
  await scroller(page).evaluate((element) => {
    element.scrollLeft = 0;
  });
  await page.getByTestId('gantt-today').click();
  await zoomed('gantt-zoom-in', 'week');
  await expect(line).toBeInViewport();
  const titles = await gantt(page).locator('[data-gantt-corner]').boundingBox();
  const today = await line.boundingBox();
  expect(today?.x ?? 0).toBeGreaterThan((titles?.x ?? 0) + (titles?.width ?? 0));
});

test('the cards without a date are counted above the chart and listed on asking', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const total = await page.evaluate(
    () =>
      (
        (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board.cards as unknown as {
          fields: { isTemplate: boolean };
        }[]
      ).filter((card) => !card.fields.isTemplate).length,
  );
  const button = page.getByTestId('gantt-undated');
  await expect(button).toHaveAccessibleName(`No date: ${total - 4}`);
  await expect(button).toHaveAttribute('aria-expanded', 'false');
  await button.click();
  const list = page.getByTestId('undated-list');
  await expect(list.locator('[data-card]')).toHaveCount(total - 4);
  await expect(list).toContainText('Attach files and open them right here');
  await expect(list).not.toContainText('Filter, sort and group');
  await expect(list).toContainText('These cards have no Day');

  // A card opened from the list gives the focus back to "No date" when it closes.
  await list.getByRole('link', { name: 'Attach files and open them right here' }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).locator('[data-card-title]')).toHaveValue(
    'Attach files and open them right here',
  );
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);
  await expect(button).toBeFocused();

  // Where the list is a column beside the chart, Escape in it closes it.
  if ((await page.getByTestId('undated-panel').count()) > 0) {
    await page.getByTestId('undated-close').focus();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('undated-panel')).toHaveCount(0);
    await expect(button).toBeFocused();
  }
  await expect(list).toHaveCount(0);
});

test('a reader who may not change the view zooms for the visit, and the view stays as it was', async ({
  page,
}, info) => {
  await page.clock.setFixedTime(NOW);
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await fitPane(page, paneOf(info));
  // A Gantt view on the board the reader can only read, made by someone who may.
  const made = await page.evaluate(async () => {
    const dev = (
      window as unknown as { fvociDev: { backend: Backend; someoneElse?: () => Backend } }
    ).fvociDev;
    const workspaces = await dev.backend.workspaces();
    const boards = await dev.backend.boards(workspaces[0]?.id ?? '');
    const boardId = boards.find((board) => board.title === 'Study notes')?.id ?? '';
    const fields = {
      viewType: 'gantt',
      ganttZoom: 'week',
      sort: [],
      shownProperties: ['__title'],
      visibleGroups: [],
      hiddenGroups: [],
      collapsedGroups: [],
      filter: { operation: 'and', filters: [] },
      cardOrder: [],
      columnWidths: {},
      columnCalculations: {},
      groupCalculations: {},
      defaultTemplateId: '',
    };
    const author = dev.someoneElse?.() ?? dev.backend;
    // Two dated cards, the second after the first and starting before it ends.
    const board = await author.container(boardId);
    const due = board.cardProperties.find((property) => property.name === 'Review on');
    if (!due) throw new Error('the board has no Review on');
    await author.patchContainer(boardId, {
      updatedCardProperties: [{ id: 'after', name: 'After', type: 'relation', options: [] }],
    });
    const one = await author.createCard(boardId, {
      title: 'First',
      properties: { [due.id]: { from: '2026-11-09', to: '2026-11-13' } },
    });
    await author.createCard(boardId, {
      title: 'Second',
      properties: {
        [due.id]: { from: '2026-11-11' },
        after: [{ card: one.id, kind: 'fs', lagDays: 0 }],
      },
    });
    const view = await author.createView(boardId, {
      title: 'Read only',
      fields: { ...fields, dateProperty: due.id, dependencyPropertyId: 'after' },
    });
    return { boardId, viewId: view.id };
  });
  await goTo(page, boardPath(made.boardId, made.viewId));
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'week');
  // The reader sees the chart whole: 5 seeded dated cards + 2 added, the arrow, what is out of order; and opens a card.
  await expect(bars(page)).toHaveCount(7);
  await expect(arrows(page)).toHaveCount(1);
  await expect(page.getByTestId('gantt-late')).toHaveAccessibleName(
    'Out of order: 1. Show the next.',
  );
  await bar(page, 'Second').click();
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(bar(page, 'Second')).toBeFocused();
  // Nothing that changes the view for everyone is offered: "…" only saves it as a file.
  await page.getByTestId('view-header').locator('[data-tool="more"]').click();
  await expect(page.getByRole('menuitem')).toHaveText(['Export to CSV']);
  await page.keyboard.press('Escape');
  await page.getByTestId('gantt-zoom-in').click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'day');
  await page.getByTestId('gantt-zoom-out').click();
  await page.getByTestId('gantt-zoom-out').click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'month');
  expect((await storedFields(page)).ganttZoom).toBe('week');
});
