/**
 * Changing a card's date on the calendar without a drag: "Change date" in
 * the "…" of the card's line opens a month of days, the card moves whole to
 * the day picked, and one undo brings it back. A calendar placed by when
 * cards were changed offers no way and says why.
 *
 * The same welcome board, moment and projects as e2e/calendar.spec.ts.
 */
import { primaryKey, menuTo, tabKey } from './keyboard.support.ts';
import { expect, test, type Page, type TestInfo } from '@playwright/test';

import {
  addUpdatedTimeProperty,
  calendar,
  CALENDAR_PAGE,
  dayButton,
  dueOf,
  focused,
  listOf,
  openCalendar,
  setDates,
  tabTo,
  wide,
} from './calendar.support.ts';

test.use(CALENDAR_PAGE);

/** With the keys alone: the 12th's list, the card's "…", "Change date". */
async function changeDateByKeys(page: Page, info: TestInfo, title: string): Promise<void> {
  await page.getByTestId('calendar-undated').focus();
  await page.keyboard.press(await tabKey(page));
  // The picked day takes the focus: today, the 10th, two days before the 12th, at first.
  if ((await calendar(page).getAttribute('data-selected')) === '2026-11-10') {
    await expect(focused(page)).toHaveAccessibleName(/Tuesday, November 10/);
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
  }
  await expect(focused(page)).toHaveAccessibleName(/^Thursday, November 12/);
  if (wide(info)) await page.keyboard.press('Enter');
  const list = listOf(page, info);
  await expect(list.locator('[data-card]').filter({ hasText: title })).toBeVisible();
  await tabTo(page, list.getByRole('button', { name: `Card menu: ${title}` }));
  await page.keyboard.press('Enter');
  const change = page.locator('[data-action="change-date"]');
  await expect(change).toHaveText('Change date');
  // A dropdown puts the focus on its first line; a sheet on itself.
  await menuTo(page, change);
  await page.keyboard.press('Enter');
}

test('a card moves to another day with the keys alone, and one undo brings it back', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await expect(dayButton(page, '2026-11-12')).toHaveAccessibleName(/November 12, 2 cards/);
  await changeDateByKeys(page, info, 'Open a card');

  const picker = page.getByTestId('day-picker');
  await expect(picker).toBeVisible();
  await expect(picker).toContainText('Now November 12.');
  // The focus is on the card's day in a popover; a sheet takes it itself, and Tab goes on to
  // the month's arrows and then the day. A week down is the 19th.
  const twelfth = picker.locator('[data-bits-day][data-value="2026-11-12"]');
  if (info.project.name === 'mobile') await tabTo(page, twelfth);
  await expect(twelfth).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');

  await expect(picker).toBeHidden();
  await expect.poll(() => dueOf(page, 'Open a card')).toEqual({ from: '2026-11-19' });
  await expect(page.getByTestId('calendar-status')).toHaveText(
    'Open a card is now on Thursday, November 19.',
  );
  // The focus is on the day the card is on now, which says so.
  await expect(focused(page)).toHaveAccessibleName(/^Thursday, November 19, 2 cards/);
  await expect(dayButton(page, '2026-11-12')).toHaveAccessibleName(/November 12, 1 card$/);

  await page.keyboard.press(await primaryKey(page, 'z'));
  await expect.poll(() => dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });
  await expect(dayButton(page, '2026-11-12')).toHaveAccessibleName(/November 12, 2 cards/);
  await expect(dayButton(page, '2026-11-19')).toHaveAccessibleName(/November 19, 1 card$/);
});

test('Escape leaves the date as it was and gives the focus back to the day', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await changeDateByKeys(page, info, 'Open a card');
  await expect(page.getByTestId('day-picker')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('day-picker')).toBeHidden();
  expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });
  // Back to the day; under the small grid, where the card's line is still there, to its "…".
  await expect(focused(page)).toHaveAccessibleName(
    wide(info) ? /^Thursday, November 12/ : 'Card menu: Open a card',
  );

  // Enter on the day the card is on already: the picker closes, and nothing changes.
  await changeDateByKeys(page, info, 'Open a card');
  const twelfth = page
    .getByTestId('day-picker')
    .locator('[data-bits-day][data-value="2026-11-12"]');
  if (info.project.name === 'mobile') await tabTo(page, twelfth);
  await expect(twelfth).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('day-picker')).toBeHidden();
  expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });
  await expect(page.getByTestId('calendar-status')).toHaveText('');
});

test('a range keeps its length and a moment its time, into the next month', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  /** Picks a day in the picker the card's "…" opened, by pressing it. */
  async function moveOn(date: string, title: string, to: string): Promise<void> {
    await dayButton(page, date).click();
    const line = listOf(page, info).locator('[data-card]').filter({ hasText: title });
    await line.hover();
    await line.getByRole('button', { name: `Card menu: ${title}` }).click();
    await page.locator('[data-action="change-date"]').click();
    const picker = page.getByTestId('day-picker');
    await picker.getByRole('button', { name: /next/i }).click();
    await picker.locator(`[data-bits-day][data-value="${to}"]`).click();
    await expect(picker).toBeHidden();
  }

  await moveOn('2026-11-12', 'Filter, sort and group', '2026-12-01');
  await expect
    .poll(() => dueOf(page, 'Filter, sort and group'))
    .toEqual({
      from: '2026-12-01',
      to: '2026-12-13',
    });
  // The calendar went to the day the card is on now.
  await expect(page.getByTestId('calendar-period')).toHaveText('December 2026');
  await expect(calendar(page)).toHaveAttribute('data-selected', '2026-12-01');

  await page.getByTestId('calendar-previous').click();
  // 08:00 on the 10th in Seoul, to 08:00 on the 2nd of December.
  await moveOn('2026-11-10', 'Write in the body', '2026-12-02');
  await expect
    .poll(() => dueOf(page, 'Write in the body'))
    .toEqual({ from: Date.parse('2026-12-01T23:00:00Z') });
});

test('a calendar placed by when cards were changed offers no change, and says why', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await addUpdatedTimeProperty(page);
  const header = page.getByTestId('view-header');
  if ((await header.getAttribute('data-layout')) === 'row') {
    await header.locator('[data-tool="display-by"]').click();
  } else {
    await header.locator('[data-tool="more"]').click();
    await page.locator('[role="menuitem"][data-action="display-by"]').click();
  }
  await page.getByRole('menuitem', { name: 'Last updated time' }).click();
  await dayButton(page, '2026-11-10').click();
  const list = listOf(page, info);
  await expect(list.getByTestId('dates-fixed')).toHaveText(
    'The cards are placed by Last updated time, which cannot be changed here.',
  );
  const menu = list.getByRole('button', { name: /^Card menu: / }).first();
  // What a screen reader hears on the "…" says why, as the list does.
  await expect(menu).toHaveAccessibleDescription(
    'The cards are placed by Last updated time, which cannot be changed here.',
  );
  await list.locator('[data-card]').first().hover();
  await menu.click();
  await expect(page.locator('[data-action="duplicate"]')).toBeVisible();
  await expect(page.locator('[data-action="change-date"]')).toHaveCount(0);
});

test('the picker of days is wholly in the window, beside a day of the last week of a month', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'a window 720 pixels high, with a mouse');
  await openCalendar(page, info);
  await setDates(page, { 'Open a card': { from: '2026-11-26' } });
  await dayButton(page, '2026-11-26').click();
  const list = listOf(page, info);
  await list.getByRole('button', { name: 'Card menu: Open a card' }).click();
  await page.locator('[data-action="change-date"]').click();
  const picker = page.getByTestId('day-picker');
  await expect(picker).toBeVisible();
  const box = await picker.boundingBox();
  const window = page.viewportSize();
  if (!box || !window) throw new Error('nothing to measure');
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(window.height);
  // The last week of the month is there to pick from.
  await expect(picker.locator('[data-bits-day][data-value="2026-11-30"]')).toBeInViewport();
});
