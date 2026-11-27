/**
 * The calendar view on the in-memory backend: making one, cards on their
 * days and bars across several, the small grid of a narrow box, the cards of
 * a day, moving through the months, and the date the view is placed by.
 *
 * The clock of the page is fixed at 2026-11-10 01:00 in Seoul (still the
 * 9th in UTC, so that a day taken from UTC would show), and so is its
 * time zone; the language is English, whose weeks start on Sunday. November
 * 2026 then has five weeks, from 1 November to 5 December.
 *
 * The projects are three widths: the
 * desktop (a pane of 1024 pixels, a mouse) and the unfolded device (570, a
 * finger) have room for bars; the phone (412) gets the small grid.
 */
import { expect, test, type Locator } from '@playwright/test';

import { ON_A_CARD } from './address.ts';
import {
  addUpdatedTimeProperty,
  bars,
  calendar,
  CALENDAR_PAGE,
  CROWD,
  crowdable,
  day,
  dayList,
  grid,
  mini,
  NOW,
  openCalendar,
  setDates,
  wide,
} from './calendar.support.ts';
import { boardPage, dialog, fitPane, openView, touch } from './kanban.support.ts';
import { tabKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';

test.use(CALENDAR_PAGE);

test('a calendar view is made from the list of kinds, placed by the first date', async ({
  page,
}, info) => {
  await page.clock.setFixedTime(NOW);
  await openView(page, info, 'Getting started with fvoci', 'Board');
  const header = page.getByTestId('view-header');
  await header.locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add calendar view' }).click();
  await expect(calendar(page)).toBeVisible();
  await expect(page.getByTestId('calendar-period')).toHaveText('November 2026');
  if ((await header.getAttribute('data-layout')) === 'row') {
    await expect(header.locator('[data-tool="display-by"]')).toHaveAccessibleName(
      'Display by: Day',
    );
  } else {
    await header.locator('[data-tool="more"]').click();
    await expect(page.locator('[role="menuitem"][data-action="display-by"]')).toContainText('Day');
  }
});

test('the weeks of the month start on Sunday, and today is marked', async ({ page }, info) => {
  await openCalendar(page, info);
  const weeks = calendar(page).locator('[data-week]');
  await expect(weeks).toHaveCount(5);
  await expect(weeks.first()).toHaveAttribute('data-week', '2026-11-01');
  await expect(calendar(page).locator('[data-weekday]').first()).toHaveText('Sun');
  const today = calendar(page).locator('[aria-current="date"]');
  await expect(today).toHaveCount(1);
  await expect(today).toHaveAccessibleName(/Tuesday, November 10/);
});

test('a dated card is on its day, a range is a bar in each week it crosses', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  // The phone is given room for bars: the grid is the same under a finger.
  if (!wide(info)) await fitPane(page, 700);
  const week = (date: string): Locator => grid(page).locator(`[data-week="${date}"]`);
  /** Whether a bar starts in the column of a day, as the cells lie on the page. */
  async function startsOn(bar: Locator, date: string): Promise<void> {
    const cell = await day(page, date).boundingBox();
    const box = await bar.boundingBox();
    if (!cell || !box) throw new Error(`no cell ${date} or no bar`);
    expect(box.x).toBeGreaterThanOrEqual(cell.x - 1);
    expect(box.x).toBeLessThan(cell.x + cell.width / 2);
  }
  // "Open a card" is on Thursday the 12th: the fifth column of the second week.
  await startsOn(
    week('2026-11-08').locator('[data-bar]').filter({ hasText: 'Open a card' }),
    '2026-11-12',
  );
  // The range is three bars, cut square where it goes on.
  await expect(bars(page, 'Filter, sort and group')).toHaveCount(3);
  const [first, second, third] = await bars(page, 'Filter, sort and group').all();
  const grid8 = await week('2026-11-08').boundingBox();
  const box1 = await first?.boundingBox();
  const box2 = await second?.boundingBox();
  const box3 = await third?.boundingBox();
  if (!grid8 || !box1 || !box2 || !box3) throw new Error('the bars are not on the page');
  const column = grid8.width / 7;
  // Thursday to Saturday, the whole week, Sunday to Tuesday.
  expect(Math.round(box1.width / column)).toBe(3);
  expect(Math.round(box2.width / column)).toBe(7);
  expect(Math.round(box3.width / column)).toBe(3);
  expect(box1.x + box1.width).toBeCloseTo(grid8.x + grid8.width, -1);
  // A moment is on the reader's day (the 10th in Seoul; the 9th in UTC), with its time.
  const moment = week('2026-11-08').locator('[data-bar]').filter({ hasText: 'Write in the body' });
  await expect(moment).toContainText('8:00');
  await startsOn(moment, '2026-11-10');
  // A bar says its day and time for a screen reader.
  if (!touch(info))
    await expect(moment).toHaveAccessibleName('8:00 AM number 5, Write in the body, November 10');
  // A day says how many cards it has.
  await expect(
    calendar(page).getByRole('button', { name: 'Thursday, November 12, 2 cards' }),
  ).toHaveCount(1);
});

test('a day with more cards than rows says "+N", and the day lists them all', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await crowdable(page, info);
  // As many more cards as the cells have rows: two cards on the day are already there.
  const lanes = Number(await grid(page).getAttribute('data-lanes'));
  const crowd = CROWD.slice(0, lanes);
  expect(crowd).toHaveLength(lanes);
  await setDates(page, Object.fromEntries(crowd.map((title) => [title, { from: '2026-11-12' }])));
  const more = grid(page).locator('[data-more="2026-11-12"]');
  await expect(more).toBeVisible();
  const hidden = Number((await more.textContent())?.replace('+', ''));
  // Every row but the last has a bar, the last says how many more.
  expect(hidden).toBe(lanes + 2 - (lanes - 1));

  if (touch(info)) await day(page, '2026-11-12').click();
  else await more.click();
  const list = page.getByTestId('day-popover');
  await expect(list).toBeVisible();
  await expect(list.locator('[data-card]')).toHaveCount(lanes + 2);
  await expect(list).toContainText('Thursday, November 12');
  // A card of several days says which of them this is.
  await expect(
    list.locator('[data-card]').filter({ hasText: 'Filter, sort and group' }),
  ).toContainText('Day 1 of 13');
  await page.keyboard.press('Escape');
  await expect(list).toBeHidden();
});

test('a card opens from its bar with a mouse, from the list of its day with a finger', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  if (wide(info) && !touch(info)) {
    await bars(page, 'Open a card').click();
  } else {
    await day(page, '2026-11-12').click();
    const list = wide(info) ? page.getByTestId('day-popover') : dayList(page);
    await list.locator('[data-card]').filter({ hasText: 'Open a card' }).locator('a').click();
  }
  await expect(dialog(page)).toContainText('Open a card');
  await expect(page).toHaveURL(ON_A_CARD);
});

test('a narrow box has the small grid over the list of the picked day', async ({ page }, info) => {
  await openCalendar(page, info);
  // The width of the phone, in every project: a mouse gets the same small grid.
  if (wide(info)) await fitPane(page, 412);
  await expect(mini(page)).toBeVisible();
  // Today is picked first; its list has the card of 08:00.
  await expect(dayList(page)).toHaveAttribute('data-date', '2026-11-10');
  await expect(dayList(page).locator('[data-card]')).toHaveCount(1);
  await expect(dayList(page)).toContainText('8:00');
  // The range is a line across its days, the card of one day a dot.
  await expect(mini(page).locator('[data-line]')).toHaveCount(3);
  await day(page, '2026-11-12').click();
  await expect(dayList(page)).toHaveAttribute('data-date', '2026-11-12');
  await expect(dayList(page).locator('[data-card]')).toHaveCount(2);
  // A day is a button wide enough for a thumb.
  const box = await day(page, '2026-11-12').boundingBox();
  expect(box?.width).toBeGreaterThanOrEqual(44);
  expect(box?.height).toBeGreaterThanOrEqual(44);
});

test('the grid turns into the small one at 34rem of box, and keeps the month', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await page.getByTestId('calendar-next').click();
  await expect(page.getByTestId('calendar-period')).toHaveText('December 2026');
  await fitPane(page, 543);
  await expect(calendar(page)).toHaveAttribute('data-layout', 'mini');
  await expect(page.getByTestId('calendar-period')).toHaveText('December 2026');
  await fitPane(page, 544);
  await expect(calendar(page)).toHaveAttribute('data-layout', 'bars');
  await expect(calendar(page)).toHaveAttribute('data-month', '2026-12');
  // Nothing is wider than the page.
  const overflow = await boardPage(page).evaluate(
    (element) => element.scrollWidth - element.clientWidth,
  );
  expect(overflow).toBe(0);
});

test('the arrows go a month on and back, and "Today" comes back', async ({ page }, info) => {
  await openCalendar(page, info);
  await page.getByTestId('calendar-next').click();
  await expect(page.getByTestId('calendar-period')).toHaveText('December 2026');
  await expect(calendar(page).locator('[data-week]').first()).toHaveAttribute(
    'data-week',
    '2026-11-29',
  );
  await page.getByTestId('calendar-previous').click();
  await page.getByTestId('calendar-previous').click();
  await expect(page.getByTestId('calendar-period')).toHaveText('October 2026');
  await page.getByTestId('calendar-today').click();
  await expect(page.getByTestId('calendar-period')).toHaveText('November 2026');
  await expect(calendar(page)).toHaveAttribute('data-selected', '2026-11-10');
});

test('the days are one stop of Tab, and the arrows walk them into the next month', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  await expect(calendar(page).locator('[data-day-button][tabindex="0"]')).toHaveCount(1);
  // From the last button of the row above, one Tab is on the picked day: the other days
  // are not stops (bars are stops of their own; the first week has none here).
  await page.getByTestId('calendar-undated').focus();
  await page.keyboard.press(await tabKey(page));
  await expect(page.locator(':focus')).toHaveAccessibleName(/Tuesday, November 10/);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(calendar(page)).toHaveAttribute('data-selected', '2026-11-18');
  await expect(page.locator(':focus')).toHaveAccessibleName(/November 18/);
  for (let step = 0; step < 3; step += 1) await page.keyboard.press('ArrowDown');
  await expect(calendar(page)).toHaveAttribute('data-selected', '2026-12-09');
  await expect(page.getByTestId('calendar-period')).toHaveText('December 2026');
  if (wide(info)) {
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('day-popover')).toBeVisible();
    await page.keyboard.press('Escape');
  }
  await expect(page.locator(':focus')).toHaveAccessibleName(/December 9/);
  await expect(calendar(page).locator('[data-day-button][tabindex="0"]')).toHaveCount(1);
});

test('a card opened from the cards of a day gives the focus back when it closes', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  const button = day(page, '2026-11-12')
    .locator('[data-day-button]')
    .or(calendar(page).locator('[data-day="2026-11-12"][data-day-button]'));
  await button.focus();
  await page.keyboard.press('Enter');
  const list = wide(info) ? page.getByTestId('day-popover') : dayList(page);
  await expect(list).toHaveAttribute(
    wide(info) ? 'data-testid' : 'data-date',
    /day-popover|2026-11-12/,
  );
  const link = list.locator('[data-card]').filter({ hasText: 'Open a card' }).locator('a');
  await link.focus();
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toContainText('Open a card');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  // Back on the card's bar where a mouse has bars to press; on the day whose popover listed
  // it under a finger; on the line itself in the list under the small grid, which stays.
  const focused = page.locator(':focus');
  if (!wide(info)) await expect(focused).toHaveAccessibleName('number 1 Open a card');
  else if (touch(info)) await expect(focused).toHaveAccessibleName(/^Thursday, November 12/);
  else await expect(focused).toHaveAccessibleName('number 1, Open a card, November 12');
});

test('the cards without a date are counted in the row and listed on asking', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  // Twelve cards on the board, four with a "Day". The name starts with the words shown.
  const button = page.getByTestId('calendar-undated');
  await expect(button).toHaveAccessibleName('No date: 8');
  await button.click();
  const list = page.getByTestId('undated-list');
  await expect(list).toHaveCount(1);
  await expect(list.locator('[data-card]')).toHaveCount(8);
  await expect(list).toContainText('These cards have no Day');
  await expect(list.locator('[data-card]').filter({ hasText: 'Open a card' })).toHaveCount(0);
  const panel = page.getByTestId('undated-panel');
  const sheet = page.getByTestId('bottom-sheet');
  if (info.project.name === 'mobile') {
    // A compact page has it on a sheet, and Escape closes the sheet.
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText('No date: 8');
    await expect(panel).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(list).toHaveCount(0);
  } else {
    // Elsewhere a column at the right: the focus goes in, and Escape brings it back.
    await expect(panel).toBeVisible();
    await expect(sheet).toHaveCount(0);
    await expect(page.locator(':focus')).toHaveAccessibleName('Close the cards without a date');
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
    await expect(page.locator(':focus')).toHaveAccessibleName('No date: 8');
    // Picking a day closes the column; opening the column closes the list of a day.
    await button.click();
    // Tuesday: left of the column, which covers Thursday on a medium pane.
    await day(page, '2026-11-10')
      .locator('[data-day-button]')
      .or(calendar(page).locator('[data-day="2026-11-10"][data-day-button]'))
      .click();
    await expect(panel).toHaveCount(0);
    await expect(page.getByTestId('day-popover')).toBeVisible();
    await button.click();
    await expect(page.getByTestId('day-popover')).toHaveCount(0);
    await expect(panel).toBeVisible();
    // Shift+Tab out of the column into the grid it covers closes it.
    await page.keyboard.press(await tabKey(page, true));
    await page.keyboard.press(await tabKey(page, true));
    await expect(panel).toHaveCount(0);
  }
  // A card given a date leaves the list; one opened from it gives the focus back to "No date".
  await setDates(page, { 'Describe cards with properties': { from: '2026-11-20' } });
  await expect(button).toHaveAccessibleName('No date: 7');
  await button.click();
  await expect(list.locator('[data-card]')).toHaveCount(7);
  await list.locator('[data-card]').filter({ hasText: 'Work on it together' }).locator('a').click();
  await expect(dialog(page)).toContainText('Work on it together');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await expect(page.locator(':focus')).toHaveAccessibleName('No date: 7');
});

test('"display by" places the cards by another date', async ({ page }, info) => {
  await openCalendar(page, info);
  await addUpdatedTimeProperty(page);
  await expect(day(page, '2026-11-12').first()).toBeVisible();
  const header = page.getByTestId('view-header');
  if ((await header.getAttribute('data-layout')) === 'row') {
    await header.locator('[data-tool="display-by"]').click();
  } else {
    await header.locator('[data-tool="more"]').click();
    await page.locator('[role="menuitem"][data-action="display-by"]').click();
  }
  await page.getByRole('menuitem', { name: 'Last updated time' }).click();
  // Every card was last changed today by this test's setup.
  await expect(
    calendar(page).getByRole('button', { name: /^Thursday, November 12, \d+ cards?$/ }),
  ).toHaveCount(0);
  await expect(
    calendar(page).getByRole('button', { name: /^Tuesday, November 10, 12 cards$/ }),
  ).toHaveCount(1);
  // Every card has a time it was changed: none is without a date.
  await expect(page.getByTestId('calendar-undated')).toHaveAccessibleName('No date: 0');
  await page.getByTestId('calendar-undated').click();
  await expect(page.getByTestId('undated-list')).toContainText('Every card has a date.');
});

/**
 * Pictures for a person to look at, in `target/screens/calendar/` when
 * `FVOCI_SCREENS` is set. The test runs either way: what it checks on the
 * way (the small grid at 390 pixels and nothing wider than the page, the
 * cards of a day) holds with or without pictures.
 */
test('the calendar in light and dark, at 390 pixels, and the cards of a day', async ({
  page,
}, info) => {
  const STAGE: Record<string, string> = {
    mobile: 'compact',
    middle: 'medium',
    desktop: 'expanded',
  };
  const stage = STAGE[info.project.name] ?? 'expanded';
  const shoot = async (name: string): Promise<void> => {
    if (process.env.FVOCI_SCREENS) {
      await page.screenshot({ path: `${screensDir('calendar')}/calendar-${name}.png` });
    }
  };
  await openCalendar(page, info);
  await setDates(page, {
    'See the same cards five ways': { from: '2026-11-12' },
    'Start a board from a template': { from: '2026-11-12' },
    'Work on it together': { from: '2026-11-03', to: '2026-11-05' },
    'Attach files and open them right here': { from: '2026-11-30', to: '2026-12-02' },
  });
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    if (scheme === 'dark') await expect(page.locator('html')).toHaveClass(/\bdark\b/);
    else await expect(page.locator('html')).not.toHaveClass(/\bdark\b/);
    await shoot(`${stage}-${scheme}`);
  }
  await page.emulateMedia({ colorScheme: 'light' });

  // The cards without a date: a sheet on the phone, a column elsewhere.
  await page.getByTestId('calendar-undated').click();
  await expect(page.getByTestId('undated-list').locator('[data-card]')).toHaveCount(4);
  await shoot(`undated-${stage}`);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('undated-list')).toHaveCount(0);

  if (!wide(info)) await fitPane(page, 700);
  if (touch(info)) await day(page, '2026-11-12').click();
  else await day(page, '2026-11-12').locator('[data-day-button]').click();
  await expect(page.getByTestId('day-popover').locator('[data-card]')).toHaveCount(4);
  await shoot(`day-${stage}`);
  await page.keyboard.press('Escape');

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(calendar(page)).toHaveAttribute('data-layout', 'mini');
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow).toBe(0);
  await shoot(`390-${info.project.name}`);
});
