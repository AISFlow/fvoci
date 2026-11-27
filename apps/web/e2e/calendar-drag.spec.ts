/**
 * Dragging cards on the calendar, on the in-memory backend: a bar of the
 * grid to another day and its ends to other days, with a mouse and with a
 * finger; a card without a date onto a day; one undo takes a drag back; a
 * press without a drag opens the card; Escape does not cancel; nothing is
 * dragged by a reader.
 *
 * The grid of bars is drawn on the desktop and on the page 626 pixels wide
 * (a finger); a phone has the small grid, where nothing is dragged.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { bars, CALENDAR_PAGE, calendar, day, dueOf, openCalendar } from './calendar.support.ts';
import { dialog, Finger, pointOn, undo } from './kanban.support.ts';
import { screensDir } from './screens.ts';

test.use(CALENDAR_PAGE);

/** A picture for a person to look at, with `FVOCI_SCREENS=1`: target/screens/drag/. */
async function shoot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS) {
    await page.screenshot({ path: `${screensDir('drag')}/calendar-${name}.png` });
  }
}

const chip = (page: Page): Locator => page.getByTestId('calendar-drag-chip');
const marked = (page: Page): Locator => calendar(page).locator('[data-drop]');

/** Presses on `from`, moves onto the middle of the cell of `to` in steps, and lets go. */
async function dragTo(page: Page, from: Locator, to: string, beforeUp?: () => Promise<void>) {
  const start = await pointOn(from);
  const end = await pointOn(day(page, to), 0.75);
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(end.x, end.y, { steps: 12 });
  await beforeUp?.();
  await page.mouse.up();
}

/** Moves the grip at one end of a bar onto the cell of `to`. */
async function dragGrip(page: Page, bar: Locator, end: 'start' | 'end', to: string) {
  const grip = bar.locator(`[data-grip="${end}"]`);
  await bar.hover();
  await dragTo(page, grip, to);
}

test.describe('a mouse', () => {
  test.skip(({ hasTouch }) => hasTouch, 'the mouse is tested in the desktop project');

  test('moves a card to another day; one undo takes it back', async ({ page }, info) => {
    await openCalendar(page, info);
    await dragTo(page, bars(page, 'Open a card'), '2026-11-19', async () => {
      // While it is dragged the day it would go to is marked, and a chip names it.
      await expect(marked(page)).toHaveCount(1);
      await expect(day(page, '2026-11-19')).toHaveAttribute('data-drop', '');
      await expect(chip(page)).toContainText('Open a card');
      await shoot(page, 'dragging');
    });
    await shoot(page, 'dropped');
    await expect(chip(page)).toHaveCount(0);
    await expect(marked(page)).toHaveCount(0);
    await expect(dialog(page)).toHaveCount(0);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-19' });
    await expect(day(page, '2026-11-19').getByText('Open a card')).toHaveCount(0);
    await expect(bars(page, 'Open a card')).toHaveCount(1);

    await undo(page);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });
  });

  test('a moment keeps its time of day, and a range its days', async ({ page }, info) => {
    await openCalendar(page, info);
    await dragTo(page, bars(page, 'Write in the body'), '2026-11-13');
    expect(await dueOf(page, 'Write in the body')).toEqual({
      from: Date.parse('2026-11-12T23:00:00Z'),
    });
    // Taken by the middle of its first week (12 to 14), the 13th, and let go on the 19th:
    // the range of 13 days moves by six.
    await dragTo(page, bars(page, 'Filter, sort and group').first(), '2026-11-19');
    expect(await dueOf(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-18',
      to: '2026-11-30',
    });
  });

  test('the ends of a bar are dragged by their grips', async ({ page }, info) => {
    await openCalendar(page, info);
    // A day becomes a range.
    await dragGrip(page, bars(page, 'Open a card'), 'end', '2026-11-14');
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12', to: '2026-11-14' });
    // The last day, from the last week the range is in.
    await dragGrip(page, bars(page, 'Filter, sort and group').last(), 'end', '2026-11-26');
    expect(await dueOf(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-12',
      to: '2026-11-26',
    });
    // The first day stops at the last.
    await dragGrip(page, bars(page, 'Open a card'), 'start', '2026-11-20');
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-14' });
    await undo(page);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12', to: '2026-11-14' });
  });

  test('a press without a drag opens the card, and Escape does not cancel a drag', async ({
    page,
  }, info) => {
    await openCalendar(page, info);
    await bars(page, 'Open a card').click();
    await expect(dialog(page)).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog(page)).toHaveCount(0);

    await dragTo(page, bars(page, 'Open a card'), '2026-11-18', async () => {
      await page.keyboard.press('Escape');
      await expect(chip(page)).toBeVisible();
    });
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-18' });

    // Let go where it came from, or off the days: nothing changes.
    await dragTo(page, bars(page, 'Open a card'), '2026-11-18');
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-18' });
    await expect(dialog(page)).toHaveCount(0);
  });

  test('a card without a date is dragged from its column onto a day', async ({ page }, info) => {
    await openCalendar(page, info);
    await page.getByTestId('calendar-undated').click();
    const line = page.getByTestId('undated-panel').locator('[data-card]').first();
    const title = ((await line.locator('a [data-title]').first().textContent()) ?? '').trim();
    await dragTo(page, line.locator('a').first(), '2026-11-04', async () => {
      await shoot(page, 'undated-dragging');
    });
    expect(await dueOf(page, title)).toEqual({ from: '2026-11-04' });
    // Its line has left the list: the focus is on "No date", not lost.
    await expect(page.getByTestId('calendar-undated')).toBeFocused();
    await expect(page.getByTestId('undated-panel').getByText(title, { exact: true })).toHaveCount(
      0,
    );
    await undo(page);
    // No value: the page answers nothing (null).
    expect((await dueOf(page, title)) ?? undefined).toBeUndefined();
  });

  test('in a week, a bar is dragged to another day, and by its end', async ({ page }, info) => {
    await openCalendar(page, info);
    await page.getByTestId('calendar-scale').locator('[data-scale="week"]').click();
    await expect(calendar(page)).toHaveAttribute('data-scale', 'week');
    // The week of the 10th: Sunday the 8th to Saturday the 14th.
    await dragTo(page, bars(page, 'Open a card'), '2026-11-13');
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-13' });
    await dragGrip(page, bars(page, 'Open a card'), 'end', '2026-11-14');
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-13', to: '2026-11-14' });
    await undo(page);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-13' });
  });

  test('a reader drags nothing', async ({ page }, info) => {
    await openCalendar(page, info);
    await expect(bars(page, 'Open a card').locator('[data-grip]')).toHaveCount(2);
    await page.evaluate(() => {
      const board = (
        window as unknown as {
          fvociDev: { workspace: { board: { members: Record<string, unknown>[] } } };
        }
      ).fvociDev.workspace.board;
      board.members = board.members.map((member) => ({
        ...member,
        role: 'viewer',
      }));
    });
    await expect(bars(page, 'Open a card').locator('[data-grip]')).toHaveCount(0);
    await dragTo(page, bars(page, 'Open a card'), '2026-11-19', async () => {
      await expect(chip(page)).toHaveCount(0);
    });
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });
  });
});

test.describe('a finger', () => {
  test.skip(
    ({ hasTouch, viewport }) => !hasTouch || (viewport?.width ?? 0) < 600,
    'the grid of bars under a finger: the page 626 pixels wide',
  );

  test('rests on a bar to drag it; a tap is still the day’s', async ({ page }, info) => {
    await openCalendar(page, info);
    const finger = await Finger.on(page);
    const start = await pointOn(bars(page, 'Open a card'));
    await finger.down(start);
    await expect(chip(page)).toBeVisible();
    await finger.move(await pointOn(day(page, '2026-11-19'), 0.75));
    await shoot(page, 'finger-dragging');
    await finger.up();
    await expect(chip(page)).toHaveCount(0);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-19' });
    await undo(page);
    expect(await dueOf(page, 'Open a card')).toEqual({ from: '2026-11-12' });

    // A tap on a bar opens the cards of its day, as before.
    const on = await pointOn(bars(page, 'Open a card'));
    await page.touchscreen.tap(on.x, on.y);
    await expect(page.getByTestId('day-popover')).toBeVisible();
  });
});
