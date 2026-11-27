/**
 * The Gantt view on the in-memory backend: making one, cards as bars on
 * their days, the line of today, the keys that walk the bars, and a narrow
 * page. The arrows are in gantt-relations.spec.ts; grouping, the zoom and
 * the cards without a date in gantt-tools.spec.ts.
 */
import { expect, test } from '@playwright/test';

import { viewIdOf } from './address.ts';
import {
  addGantt,
  bar,
  bars,
  clearDates,
  dayScale,
  gantt,
  NOW,
  openGantt,
  openRelated,
  scroller,
  setAfter,
  setDates,
  storedFields,
  WELCOME,
  type Dev,
} from './gantt.support.ts';
import { dialog, openView } from './kanban.support.ts';
import { tabKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

test('a Gantt view is made from the list of kinds, placed by the first date, zoomed to weeks', async ({
  page,
}, info) => {
  await page.clock.setFixedTime(NOW);
  await openView(page, info, ...WELCOME);
  await clearDates(page);
  await addGantt(page);
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'week');
  const header = page.getByTestId('view-header');
  if ((await header.getAttribute('data-layout')) === 'row') {
    await expect(header.locator('[data-tool="display-by"]')).toHaveAccessibleName(
      'Display by: Day',
    );
  } else {
    await header.locator('[data-tool="more"]').click();
    await expect(page.locator('[role="menuitem"][data-action="display-by"]')).toContainText('Day');
    await page.keyboard.press('Escape');
  }
  const fields = await storedFields(page);
  expect(fields.viewType).toBe('gantt');
  expect(fields.ganttZoom).toBe('week');
  const due = await page.evaluate(
    () =>
      (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board.board.cardProperties.find(
        (property) => property.name === 'Day',
      )?.id,
  );
  expect(fields.dateProperty).toBe(due);
  // No card of the board has a date yet.
  await expect(page.getByTestId('gantt-empty')).toBeVisible();
  // The axis has today on it and begins on a Sunday.
  await expect(gantt(page)).toHaveAttribute('data-first', '2026-10-25');
});

test('a dated card is a bar from its first day to its last, a card without a date has none', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const scale = await dayScale(page);
  /** A bar lies on these days: `days` of them from `first`. */
  async function lies(title: string, first: string, days: number): Promise<void> {
    await bar(page, title).scrollIntoViewIfNeeded();
    const now = await dayScale(page);
    const box = await bar(page, title).boundingBox();
    if (!box) throw new Error(`"${title}" has no bar`);
    expect(Math.abs(box.x - now.x(first))).toBeLessThan(1);
    expect(Math.abs(box.width - days * now.width)).toBeLessThan(1);
  }
  expect(scale.width).toBeGreaterThan(4);
  await lies('Open a card', '2026-11-12', 1);
  await lies('Filter, sort and group', '2026-11-12', 12);
  // A moment is on the reader's day: the 10th in Seoul, not the 9th of UTC.
  await lies('Write in the body', '2026-11-10', 1);
  // Into the next month.
  await lies('Keep your documents in a wiki', '2026-12-03', 2);
  await expect(bar(page, 'Filter, sort and group')).toHaveAccessibleName(
    'number 4, Filter, sort and group, November 12 – November 23',
  );
  await expect(bar(page, 'Write in the body')).toHaveAccessibleName(
    '8:00 AM number 5, Write in the body, November 10',
  );
  // Every row is a card with a bar.
  await expect(gantt(page).locator('[data-row]')).toHaveCount(4);

  // The day of a moment is the day in the time zone of the reader's settings,
  // not of the device: 23:00 UTC on the 9th is 15:00 on the 9th in Los Angeles.
  await page.evaluate(() =>
    (window as unknown as { fvociDev: Dev }).fvociDev.workspace.changeSettings({
      timeZone: 'America/Los_Angeles',
    }),
  );
  await expect(bar(page, 'Write in the body')).toHaveAccessibleName(
    '3:00 PM number 5, Write in the body, November 9',
  );
  await lies('Write in the body', '2026-11-09', 1);
});

test('a bar in another year says its year: the marks of the days are not read out', async ({
  page,
}, info) => {
  await openGantt(page, info);
  await setDates(page, {
    'Open a card': { from: '2025-03-01' },
    'Filter, sort and group': { from: '2026-12-30', to: '2027-01-02' },
  });
  await expect(bar(page, 'Open a card')).toHaveAccessibleName(
    'number 1, Open a card, March 1, 2025',
  );
  await expect(bar(page, 'Filter, sort and group')).toHaveAccessibleName(
    'number 4, Filter, sort and group, December 30, 2026 – January 2, 2027',
  );
});

test('today has a line through the chart, in sight when the view opens', async ({ page }, info) => {
  // A card a year and a half back: the axis begins long before today, and
  // the chart has to be scrolled for today to be in sight.
  await page.clock.setFixedTime(NOW);
  await openView(page, info, ...WELCOME);
  await clearDates(page);
  await setDates(page, {
    'Open a card': { from: '2025-05-01' },
    'Filter, sort and group': { from: '2026-11-12', to: '2026-11-23' },
  });
  await addGantt(page);
  await expect(bars(page)).toHaveCount(2);
  const line = page.getByTestId('gantt-today-line');
  await expect(line).toHaveAttribute('data-day', '2026-11-10');
  await expect(line).toBeInViewport();
  expect(await scroller(page).evaluate((element) => element.scrollLeft)).toBeGreaterThan(1000);
  const scale = await dayScale(page);
  const box = await line.boundingBox();
  if (!box) throw new Error('no line of today');
  // In the middle of the 10th.
  expect(Math.abs(box.x + box.width / 2 - (scale.x('2026-11-10') + scale.width / 2))).toBeLessThan(
    1.5,
  );
  // "Today" brings it back after the days were scrolled away.
  await scroller(page).evaluate((element) => {
    element.scrollLeft = 0;
  });
  await expect(line).not.toBeInViewport();
  await page.getByTestId('gantt-today').click();
  await expect(line).toBeInViewport();
});

test('the days in sight stay in sight when a card moves the start of the axis', async ({
  page,
}, info) => {
  await openGantt(page, info);
  // A card next summer: the days go on to the right of what is in sight, so
  // that the box can be scrolled to wherever today comes to lie.
  await setDates(page, { 'Keep your documents in a wiki': { from: '2027-06-01' } });
  const line = page.getByTestId('gantt-today-line');
  await expect(line).toBeInViewport();
  const before = await line.boundingBox();
  await expect(gantt(page)).toHaveAttribute('data-first', '2026-10-25');
  await setDates(page, { 'Open a card': { from: '2025-11-12' } });
  await expect(gantt(page)).toHaveAttribute('data-first', '2025-10-26');
  await expect(line).toBeInViewport();
  await expect
    .poll(async () => Math.abs(((await line.boundingBox())?.x ?? -999) - (before?.x ?? 0)))
    .toBeLessThan(2);
});

test('the titles stay at the left and the marks at the top while the chart scrolls', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const title = gantt(page).locator('[data-row-title]').first();
  const axis = page.getByTestId('gantt-axis');
  const before = { title: await title.boundingBox(), axis: await axis.boundingBox() };
  await scroller(page).evaluate((element) => {
    element.scrollLeft += 300;
  });
  await expect
    .poll(async () => (await scroller(page).evaluate((element) => element.scrollLeft)) > 0)
    .toBe(true);
  const after = { title: await title.boundingBox(), axis: await axis.boundingBox() };
  const box = await scroller(page).boundingBox();
  if (!before.title || !before.axis || !after.title || !after.axis || !box) {
    throw new Error('nothing to measure');
  }
  expect(after.title.x).toBe(before.title.x);
  expect(after.axis.y).toBe(before.axis.y);
  // The column of titles leaves most of a narrow page to the days.
  expect(after.title.width).toBeGreaterThan(80);
  expect(after.title.width / box.width).toBeLessThan(0.4);
});

test('the bars are one stop of Tab, the arrows walk them, and a bar opens its card', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const stops = gantt(page).locator('[data-bar][tabindex="0"]');
  await expect(stops).toHaveCount(1);
  // From the last button of the row above, Tab goes to the one bar and then out of the chart.
  await page.getByTestId('gantt-undated').focus();
  await page.keyboard.press(await tabKey(page));
  await expect(bars(page).first()).toBeFocused();
  await page.keyboard.press(await tabKey(page));
  await expect(scroller(page).locator(':focus')).toHaveCount(0);
  // The titles are for a pointer: none is in the order of Tab.
  await expect(gantt(page).locator('[data-row-title]:not([tabindex="-1"])')).toHaveCount(0);

  await bars(page).first().focus();
  await page.keyboard.press('ArrowDown');
  await expect(bars(page).nth(1)).toBeFocused();
  await expect(stops).toHaveCount(1);
  await expect(bars(page).nth(1)).toHaveAttribute('tabindex', '0');
  await page.keyboard.press('ArrowUp');
  await expect(bars(page).first()).toBeFocused();
  // At the top the key stays where it is.
  await page.keyboard.press('ArrowUp');
  await expect(bars(page).first()).toBeFocused();
  await page.keyboard.press('End');
  await expect(bars(page).last()).toBeFocused();
  await page.keyboard.press('Home');
  await expect(bars(page).first()).toBeFocused();
  const title = await gantt(page).locator('[data-row-title] [data-title]').first().textContent();
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).locator('[data-card-title]')).toHaveValue((title ?? '').trim());
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);
  // The focus comes back to the bar the card was opened from.
  await expect(bars(page).first()).toBeFocused();

  // The card of the stop loses its date: the stop is on another bar, still one.
  await page.keyboard.press('End');
  const last =
    (await gantt(page).locator('[data-row-title] [data-title]').last().textContent()) ?? '';
  await setDates(page, { [last.trim()]: undefined });
  await expect(bars(page)).toHaveCount(3);
  await expect(stops).toHaveCount(1);
});

test('the title of a row opens its card, and gives the focus back without scrolling the days', async ({
  page,
}, info) => {
  await openGantt(page, info);
  // A bar far from what is in sight: its title is at hand, the bar is not.
  await setDates(page, { 'Keep your documents in a wiki': { from: '2028-03-01' } });
  await page.getByTestId('gantt-today').click();
  const title = gantt(page).locator('[data-row-title]', {
    hasText: 'Keep your documents in a wiki',
  });
  const before = await scroller(page).evaluate((element) => element.scrollLeft);
  await title.click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).locator('[data-card-title]')).toHaveValue(
    'Keep your documents in a wiki',
  );
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);
  await expect(title).toBeFocused();
  expect(await scroller(page).evaluate((element) => element.scrollLeft)).toBe(before);
  // Tab goes on from the row: to its bar.
  await page.keyboard.press(await tabKey(page));
  await expect(bar(page, 'Keep your documents in a wiki')).toBeFocused();
});

test('the focus stays in the chart when the card that has it leaves', async ({ page }, info) => {
  await openGantt(page, info);
  // The bar of the second row has the focus when its card is deleted: the
  // bar that now stands in that place takes it.
  await bars(page).nth(1).focus();
  const gone = (await bars(page).nth(1).getAttribute('data-bar')) ?? '';
  await page.evaluate(
    (id) => (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board.deleteCards([id]),
    gone,
  );
  await expect(bars(page)).toHaveCount(3);
  await expect(bars(page).nth(1)).toBeFocused();

  // A card opened from its bar loses its date while it is open: it is among
  // the cards without a date now, and "No date" takes the focus when it closes.
  const title =
    (await gantt(page).locator('[data-row-title] [data-title]').nth(1).textContent()) ?? '';
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toBeVisible();
  await setDates(page, { [title.trim()]: undefined });
  await expect(bars(page)).toHaveCount(2);
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByTestId('gantt-undated')).toBeFocused();

  // The last bars go while one has the focus: with no bar left, "No date" has it.
  await bars(page).first().focus();
  const rest = await gantt(page).locator('[data-row-title] [data-title]').allTextContents();
  await setDates(page, Object.fromEntries(rest.map((each) => [each.trim(), undefined])));
  await expect(bars(page)).toHaveCount(0);
  await expect(page.getByTestId('gantt-undated')).toBeFocused();
  // An empty chart is not a stop of the Tab key by itself.
  await expect(scroller(page)).toHaveAttribute('tabindex', '-1');
});

/**
 * Pictures for a person to look at, in `target/screens/gantt/` when
 * `FVOCI_SCREENS` is set. The test runs either way: what it checks on the
 * way (nothing of the chart beyond the page at 390 pixels) holds with or without
 * pictures.
 */
test('the chart in light and dark, and at 390 pixels', async ({ page }, info) => {
  const shoot = async (name: string): Promise<void> => {
    if (process.env.FVOCI_SCREENS) {
      await page.screenshot({ path: `${screensDir('gantt')}/gantt-${name}.png` });
    }
  };
  await openRelated(page, info);
  await setDates(page, {
    'See the same cards five ways': { from: '2026-11-05', to: '2026-11-11' },
    'Start a board from a template': { from: '2026-11-16', to: '2026-11-20' },
    'Work on it together': { from: '2026-11-25', to: '2026-12-01' },
  });
  // One relation out of order: "Start a board from a template" starts before "Filter, sort and group" ends.
  await setAfter(page, {
    'Start a board from a template': ['Filter, sort and group'],
    'Work on it together': ['Start a board from a template'],
  });
  await expect(bars(page)).toHaveCount(7);
  await expect(page.getByTestId('gantt-late')).toBeVisible();
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    if (scheme === 'dark') await expect(page.locator('html')).toHaveClass(/\bdark\b/);
    else await expect(page.locator('html')).not.toHaveClass(/\bdark\b/);
    await shoot(`${info.project.name}-${scheme}`);
  }
  await page.emulateMedia({ colorScheme: 'light' });

  // Closer and farther, and a loop.
  await page.getByTestId('gantt-zoom-in').click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'day');
  await shoot(`${info.project.name}-days`);
  await page.getByTestId('gantt-zoom-out').click();
  await page.getByTestId('gantt-zoom-out').click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'month');
  await shoot(`${info.project.name}-months`);
  await page.getByTestId('gantt-zoom-in').click();
  await setAfter(page, { 'Write in the body': ['Work on it together'] });
  await expect(page.getByTestId('gantt-loops')).toBeVisible();
  await shoot(`${info.project.name}-loop`);

  // Grouped, and the cards without a date: a column beside the chart, or a sheet on the phone.
  await page.evaluate(async (id) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const select = store.board.cardProperties.find((property) => property.type === 'select');
    if (!select) throw new Error('nothing to group by');
    await store.setViewFields(id, { groupBy: select.id });
  }, viewIdOf(page.url()));
  await expect(gantt(page).locator('[data-group]').first()).toBeVisible();
  await shoot(`${info.project.name}-grouped`);
  await page.getByTestId('gantt-undated').click();
  await expect(page.getByTestId('undated-list')).toBeVisible();
  await shoot(`${info.project.name}-undated`);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('undated-list')).toHaveCount(0);
  // Not grouped again, for the picture of a phone below.
  await page.evaluate(async (id) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    await store.setViewFields(id, { groupBy: '' });
  }, viewIdOf(page.url()));
  await expect(gantt(page).locator('[data-group]')).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByTestId('gantt-today').click();
  await expect(page.getByTestId('gantt-today-line')).toBeInViewport();
  // The chart and the row above it end where the page does; the days scroll inside.
  for (const part of [scroller(page), page.getByTestId('gantt-toolbar')]) {
    const box = await part.boundingBox();
    if (!box) throw new Error('a part of the chart is not on the page');
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(390);
  }
  // The other tools can scroll; "No date" stays in sight beside them.
  await expect(page.getByTestId('gantt-undated')).toBeInViewport({ ratio: 1 });
  const last = await page.getByTestId('gantt-undated').boundingBox();
  expect((last?.x ?? 0) + (last?.width ?? 999)).toBeLessThanOrEqual(390);
  await shoot(`390-${info.project.name}`);
});
