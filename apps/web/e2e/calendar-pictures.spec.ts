/**
 * Pictures of writing on the calendar for a person to look at, in
 * `target/screens/calendar-v2/` when `FVOCI_SCREENS` is set: the card's
 * "…" and the picker of a day, a day's "New card on …", the week, and the
 * same at 390 pixels. The test runs either way: what it checks on the way
 * (each surface is there, nothing is wider than the page at 390) holds with
 * or without pictures.
 */
import { expect, test, type Page, type TestInfo } from '@playwright/test';

import {
  calendar,
  CALENDAR_PAGE,
  dayButton,
  listOf,
  openCalendar,
  setDates,
  wide,
} from './calendar.support.ts';
import { screensDir } from './screens.ts';

test.use(CALENDAR_PAGE);

const STAGE: Record<string, string> = { mobile: 'compact', middle: 'medium', desktop: 'expanded' };

async function shoot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS) {
    await page.screenshot({ path: `${screensDir('calendar-v2')}/${name}.png` });
  }
}

/** The day's list, its first card's "…", and "Change date": the picker by the day. */
async function openPicker(page: Page, info: TestInfo, stage: string): Promise<void> {
  await dayButton(page, '2026-11-12').click();
  const list = listOf(page, info);
  await expect(list.getByTestId('day-new-card')).toBeVisible();
  await shoot(page, `day-${stage}`);
  const line = list.locator('[data-card]').filter({ hasText: 'Filter, sort and group' });
  await line.hover();
  await line.getByRole('button', { name: 'Card menu: Filter, sort and group' }).click();
  await expect(page.locator('[data-action="change-date"]')).toBeVisible();
  await shoot(page, `menu-${stage}`);
  await page.locator('[data-action="change-date"]').click();
  await expect(page.getByTestId('day-picker')).toBeVisible();
  await shoot(page, `picker-${stage}`);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('day-picker')).toBeHidden();
}

/** Month or week: the two buttons where the pane has room, else the one that turns. */
async function scaleTo(page: Page, scale: 'month' | 'week'): Promise<void> {
  const both = page.getByTestId('calendar-scale');
  if (await both.isVisible()) await both.locator(`[data-scale="${scale}"]`).click();
  else await page.getByTestId('calendar-scale-toggle').click();
  await expect(calendar(page)).toHaveAttribute('data-scale', scale);
}

test('pictures of writing on the calendar, wide and at 390 pixels', async ({ page }, info) => {
  const stage = STAGE[info.project.name] ?? 'expanded';
  await openCalendar(page, info);
  await setDates(page, {
    'See the same cards five ways': { from: '2026-11-12' },
    'Start a board from a template': { from: '2026-11-11', to: '2026-11-13' },
    'Work on it together': { from: '2026-11-03', to: '2026-11-05' },
  });
  if (wide(info)) {
    // The "+" of a day where the mouse is.
    await dayButton(page, '2026-11-25').hover();
  }
  await shoot(page, `month-${stage}`);
  await openPicker(page, info, stage);

  await scaleTo(page, 'week');
  await shoot(page, `week-${stage}`);
  await scaleTo(page, 'month');

  if (info.project.name !== 'mobile') return;
  // A phone that is 390 wide already keeps its height: WebKit's phone takes no press below the
  // height it began with once it is made taller.
  if (page.viewportSize()?.width !== 390) await page.setViewportSize({ width: 390, height: 844 });
  await expect(calendar(page)).toHaveAttribute('data-layout', 'mini');
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow).toBe(0);
  await shoot(page, 'month-390');
  await openPicker(page, info, '390');
  await scaleTo(page, 'week');
  await expect(page.getByTestId('week-agenda')).toBeVisible();
  await shoot(page, 'week-390');
  await page.getByTestId('calendar-undated').click();
  await expect(page.getByTestId('undated-list')).toBeVisible();
  await shoot(page, 'undated-390');
});
