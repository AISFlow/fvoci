/**
 * The week of the calendar: a week as high as the box with
 * every card in it and bars of two lines, or under a narrow box the strip of
 * the week over the lists of its seven days; the arrows go a week; month or
 * week is remembered on the device for the view.
 *
 * The same welcome board, moment and projects as e2e/calendar.spec.ts.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import {
  addUpdatedTimeProperty,
  calendar,
  CALENDAR_PAGE,
  dayButton,
  focused,
  grid,
  openCalendar,
  setDates,
  tabTo,
  wide,
} from './calendar.support.ts';
import { dialog } from './kanban.support.ts';
import { tabKey } from './keyboard.support.ts';

test.use(CALENDAR_PAGE);

const title = (page: Page): Locator => page.getByTestId('calendar-period');

/** The text with the narrow spaces `Intl` puts around a dash read as plain ones. */
async function titleText(page: Page): Promise<string> {
  return ((await title(page).textContent()) ?? '').replace(/\s/gu, ' ').trim();
}

/** Month or week with the keys alone: the two buttons where there is room, else the one. */
async function chooseByKeys(page: Page, scale: 'month' | 'week'): Promise<void> {
  const both = page.getByTestId('calendar-scale');
  const target = (await both.isVisible())
    ? both.locator(`[data-scale="${scale}"]`)
    : page.getByTestId('calendar-scale-toggle');
  await page.getByTestId('calendar-today').focus();
  await tabTo(page, target);
  await page.keyboard.press('Enter');
  await expect(calendar(page)).toHaveAttribute('data-scale', scale);
}

test('a week with the keys alone: its days, the arrows a week on, and back to the month', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await expect(calendar(page)).toHaveAttribute('data-scale', 'month');
  await chooseByKeys(page, 'week');
  await expect.poll(() => titleText(page)).toBe('Nov 8 – 14');
  await expect(calendar(page).locator('[data-week]')).toHaveCount(wide(info) ? 1 : 0);
  if (!wide(info)) {
    await expect(page.getByTestId('week-agenda').locator('[data-agenda-day]')).toHaveCount(7);
    await expect(page.getByTestId('week-agenda')).toContainText('No cards on this day.');
  }
  await expect(page.getByRole('button', { name: 'Next week' })).toBeVisible();

  // The arrow keys walk the days into the next week, and the week follows.
  await page.getByTestId('calendar-undated').focus();
  await page.keyboard.press(await tabKey(page));
  await expect(focused(page)).toHaveAccessibleName(/^Tuesday, November 10/);
  await page.keyboard.press('ArrowDown');
  await expect(focused(page)).toHaveAccessibleName(/^Tuesday, November 17/);
  await expect.poll(() => titleText(page)).toBe('Nov 15 – 21');

  await page.getByRole('button', { name: 'Next week' }).click();
  await expect.poll(() => titleText(page)).toBe('Nov 22 – 28');
  await page.getByRole('button', { name: 'Next week' }).click();
  await expect.poll(() => titleText(page)).toBe('Nov 29 – Dec 5');
  await page.getByTestId('calendar-today').click();
  await expect.poll(() => titleText(page)).toBe('Nov 8 – 14');

  await chooseByKeys(page, 'month');
  await expect(title(page)).toHaveText('November 2026');
});

test('a wide week draws every card of it, with no "+N", in bars of two lines', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  // More cards on the 12th than a month's cell has rows for.
  await setDates(page, {
    'See the same cards five ways': { from: '2026-11-12' },
    'Describe cards with properties': { from: '2026-11-12' },
    'Attach files and open them right here': { from: '2026-11-12' },
    'Keep your documents in a wiki': { from: '2026-11-12' },
    'Start a board from a template': { from: '2026-11-12' },
    'Work on it together': { from: '2026-11-12' },
  });
  await chooseByKeys(page, 'week');
  if (!wide(info)) {
    // The phone's week is the lists of its days: the 12th lists all eight.
    const twelfth = page.getByTestId('week-agenda').locator('[data-agenda-day="2026-11-12"]');
    await expect(twelfth.locator('[data-card]')).toHaveCount(8);
    return;
  }
  await expect(grid(page).locator('[data-more]')).toHaveCount(0);
  const bars = grid(page).locator('[data-bar]');
  // Eight cards on the 12th, one at 08:00 on the 10th... all drawn.
  await expect(
    grid(page).locator('[data-bar]').filter({ hasText: 'Work on it together' }),
  ).toHaveCount(1);
  expect(await bars.count()).toBeGreaterThanOrEqual(9);
  const tall = await bars.first().boundingBox();
  expect(tall?.height ?? 0).toBeGreaterThanOrEqual(44);
  // The second line of a timed card is its time; of a range, its days.
  await expect(bars.filter({ hasText: 'Write in the body' })).toContainText('8:00 AM');
  await expect(bars.filter({ hasText: 'Filter, sort and group' })).toContainText(
    'November 12 – 24',
  );
  // A bar of a week is pressed to open its card, under a finger too (it is 44 pixels high).
  await bars.filter({ hasText: 'Open a card' }).click();
  await expect(dialog(page)).toContainText('Open a card');
});

test('the week is remembered for the view on this device, after a reload', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await chooseByKeys(page, 'week');
  await page.reload();
  await expect(calendar(page)).toHaveAttribute('data-scale', 'week');
  await expect.poll(() => titleText(page)).toBe('Nov 8 – 14');
  // The month in view is not kept: a new load is at today.
  await expect(dayButton(page, '2026-11-10')).toHaveAttribute('aria-current', 'date');
});

test('in a narrow week the picked day stays in view, and the list says why dates are fixed', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await addUpdatedTimeProperty(page);
  await chooseByKeys(page, 'week');
  if (wide(info)) {
    // A wide week has no list to scroll: every day is a column of the grid.
    await expect(page.getByTestId('week-agenda')).toHaveCount(0);
    return;
  }
  const agenda = page.getByTestId('week-agenda');
  await dayButton(page, '2026-11-10').focus();
  for (let step = 0; step < 4; step += 1) await page.keyboard.press('ArrowRight');
  await expect(focused(page)).toHaveAccessibleName(/^Saturday, November 14/);
  await expect(agenda.locator('[data-agenda-day="2026-11-14"] h3')).toBeInViewport();

  // Placed by when cards were changed: no "Change date", and the list says why.
  await page.getByTestId('view-header').locator('[data-tool="more"]').click();
  await page.locator('[role="menuitem"][data-action="display-by"]').click();
  await page.getByRole('menuitem', { name: 'Last updated time' }).click();
  await expect(agenda.getByTestId('dates-fixed')).toHaveText(
    'The cards are placed by Last updated time, which cannot be changed here.',
  );
});
