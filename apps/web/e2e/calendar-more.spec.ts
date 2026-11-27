/**
 * The cards of a day that do not fit, and the cards without a date, on the
 * way to writing: "+N" opens every card of its day,
 * each with its "…", and a card without a date is given one from the list
 * "No date" opens, with the keys alone; one undo takes the date away again.
 *
 * The same welcome board, moment and projects as e2e/calendar.spec.ts.
 */
import { primaryKey, menuTo, tabKey } from './keyboard.support.ts';
import { expect, test } from '@playwright/test';

import {
  CALENDAR_PAGE,
  CROWD,
  crowdable,
  dayButton,
  dueOf,
  focused,
  grid,
  openCalendar,
  setDates,
  tabTo,
} from './calendar.support.ts';
import { touch } from './kanban.support.ts';

test.use(CALENDAR_PAGE);

test('a card without a date is given one from "No date" with the keys alone, and undo takes it back', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  const button = page.getByTestId('calendar-undated');
  await expect(button).toHaveAccessibleName('No date: 8');
  await button.focus();
  await page.keyboard.press('Enter');
  const list = page.getByTestId('undated-list');
  await tabTo(
    page,
    list.getByRole('button', { name: 'Card menu: Describe cards with properties' }),
  );
  await page.keyboard.press('Enter');
  const set = page.locator('[data-action="change-date"]');
  await expect(set).toHaveText('Set date');
  await menuTo(page, set);
  await page.keyboard.press('Enter');

  const picker = page.getByTestId('day-picker');
  await expect(picker).toContainText('The day picked becomes its Day.');
  // The month of the picked day, the 10th, which has the focus in a popover; a sheet takes it
  // itself, and Tab goes on to the month's arrows and then the day. A week down is the 17th.
  const tenth = picker.locator('[data-bits-day][data-value="2026-11-10"]');
  if (info.project.name === 'mobile') await tabTo(page, tenth);
  await expect(tenth).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(picker).toBeHidden();
  await expect
    .poll(() => dueOf(page, 'Describe cards with properties'))
    .toEqual({ from: '2026-11-17' });
  await expect(button).toHaveAccessibleName('No date: 7');
  await expect(focused(page)).toHaveAccessibleName(/^Tuesday, November 17, 2 cards/);

  await page.keyboard.press(await primaryKey(page, 'z'));
  await expect.poll(() => dueOf(page, 'Describe cards with properties')).toBeNull();
  await expect(button).toHaveAccessibleName('No date: 8');
  // "Filter, sort and group" stays: the 12th to the 24th.
  await expect(dayButton(page, '2026-11-17')).toHaveAccessibleName(/November 17, 1 card$/);
});

test('"+N" opens every card of its day, each with its "…", reached by the keys', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await crowdable(page, info);
  const lanes = Number(await grid(page).getAttribute('data-lanes'));
  const crowd = CROWD.slice(0, lanes);
  // A cell of more rows than the crowd has cards would not overflow.
  expect(crowd).toHaveLength(lanes);
  await setDates(page, Object.fromEntries(crowd.map((title) => [title, { from: '2026-11-12' }])));
  const more = grid(page).locator('[data-more="2026-11-12"]');
  await expect(more).toBeVisible();
  const twelfth = dayButton(page, '2026-11-12');
  await twelfth.click();
  await page.keyboard.press('Escape');
  await expect(twelfth).toBeFocused();
  // Under a finger the day is the button, "+N" a picture in it. With a mouse "+N" is a
  // button of its own and a stop of the Tab key, right after the day's.
  if (!touch(info)) {
    await page.keyboard.press(await tabKey(page));
    await expect(more).toBeFocused();
  }
  await page.keyboard.press('Enter');
  const list = page.getByTestId('day-popover');
  await expect(list.locator('[data-card]')).toHaveCount(lanes + 2);
  await expect(list.getByRole('button', { name: /^Card menu: / })).toHaveCount(lanes + 2);
  await expect(list.getByTestId('day-new-card')).toHaveText('New card on November 12');
});
