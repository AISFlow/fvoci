/**
 * Dragging the bars of a Gantt view on the in-memory backend: a bar to other
 * days and its ends to other days, with a mouse and with a finger; one undo
 * takes a drag back; a press without a drag opens the card; Escape does not
 * cancel; a change the backend refuses puts the bar back; nothing is
 * dragged where the dates cannot be changed.
 *
 * The chart is at the zoom of weeks: a day is 0.75rem, 12 pixels.
 */
import { menuTo } from './keyboard.support.ts';
import { expect, test, type Locator, type Page } from '@playwright/test';

import { viewIdOf } from './address.ts';
import { bar, dayScale, gantt, openGantt, scroller, type Dev } from './gantt.support.ts';
import { tabTo } from './calendar.support.ts';
import { dialog, Finger, pointOn, touch, undo } from './kanban.support.ts';
import { screensDir } from './screens.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

/** A picture for a person to look at, with `FVOCI_SCREENS=1`: target/screens/drag/. */
async function shoot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS) {
    await page.screenshot({ path: `${screensDir('drag')}/gantt-${name}.png` });
  }
}

const row = (page: Page, title: string): Locator =>
  gantt(page)
    .locator('[data-row]')
    .filter({ has: page.locator('[data-row-title]', { hasText: title }) });
const grip = (page: Page, title: string, end: 'start' | 'end'): Locator =>
  row(page, title).locator(`[data-grip="${end}"]`);
const dragged = (page: Page): Locator => gantt(page).locator('[data-bar][data-dragged]');

/** The card's value of "Due Date" as the board keeps it. */
async function due(page: Page, title: string): Promise<unknown> {
  return page.evaluate((wanted) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const property = store.board.cardProperties.find((each) => each.name === 'Day');
    const card = store.cards.find((each) => each.title === wanted) as
      { fields: { properties: Record<string, unknown> } } | undefined;
    return property ? card?.fields.properties[property.id] : undefined;
  }, title);
}

/**
 * Something to wait on for the end of the next scroll of the chart: the
 * browser says so (`scrollend`), once a swipe that goes on by itself after
 * the finger is up has stopped too. Readings of where it is scrolled to may
 * stand still while it is still going, on a busy page. Only an end that
 * leaves the chart elsewhere than it is now counts.
 */
async function scrollEnd(page: Page): Promise<() => Promise<void>> {
  await scroller(page).evaluate((element) => {
    const from = element.scrollLeft;
    Reflect.set(window, 'scrollEnded', false);
    element.addEventListener('scrollend', () => {
      if (element.scrollLeft !== from) Reflect.set(window, 'scrollEnded', true);
    });
  });
  return () =>
    expect.poll(() => page.evaluate(() => Reflect.get(window, 'scrollEnded') === true)).toBe(true);
}

/** Waits for the chart to stop scrolling: a swipe goes on for a while after the finger is up. */
async function settled(page: Page): Promise<void> {
  let last = -1;
  await expect
    .poll(async () => {
      const now = await scroller(page).evaluate((element) => element.scrollLeft);
      const still = now === last;
      last = now;
      return still;
    })
    .toBe(true);
}

/** Scrolls the days so that `target` is in the middle of what is in sight of them. */
async function centre(page: Page, target: Locator): Promise<void> {
  await target.scrollIntoViewIfNeeded();
  const box = await target.boundingBox();
  const titles = await gantt(page).locator('[data-gantt-corner]').boundingBox();
  const sight = await scroller(page).boundingBox();
  if (!box || !titles || !sight) throw new Error('nothing to centre');
  const middle = (titles.x + titles.width + sight.x + sight.width) / 2;
  await scroller(page).evaluate(
    (element, by) => {
      element.scrollLeft += by;
    },
    box.x + box.width / 2 - middle,
  );
  await settled(page);
}

/** Presses on `target`, moves `days` days across in steps, and lets go. */
async function dragBy(page: Page, target: Locator, days: number, beforeUp?: () => Promise<void>) {
  // Hover waits for a stable, hittable bar before the coordinate-driven gesture.
  await target.hover();
  const { width } = await dayScale(page);
  const from = await pointOn(target);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + days * width, from.y, { steps: 12 });
  await beforeUp?.();
  await page.mouse.up();
}

test.describe('a mouse', () => {
  test.skip(({ hasTouch }) => hasTouch, 'the mouse is tested in the desktop project');

  test('moves a bar by whole days, keeping its length; one undo takes it back', async ({
    page,
  }, info) => {
    await openGantt(page, info);
    const share = bar(page, 'Filter, sort and group');
    const { x } = await dayScale(page);
    await dragBy(page, share, 3, async () => {
      // While it is dragged it is drawn on the days it would go to, which are said over it.
      await expect(dragged(page)).toHaveCount(1);
      await expect(gantt(page).getByTestId('gantt-drag-days')).toHaveText(
        'November 15 – November 26',
      );
      await shoot(page, 'dragging');
    });
    await shoot(page, 'dropped');
    await expect(dragged(page)).toHaveCount(0);
    await expect(dialog(page)).toHaveCount(0);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-15',
      to: '2026-11-26',
    });
    expect((await share.boundingBox())?.x).toBeCloseTo(x('2026-11-15'), 0);

    await undo(page);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-12',
      to: '2026-11-23',
    });
    expect((await share.boundingBox())?.x).toBeCloseTo(x('2026-11-12'), 0);
  });

  test('a moment keeps its time of day on the reader’s clock', async ({ page }, info) => {
    await openGantt(page, info);
    await dragBy(page, bar(page, 'Write in the body'), -2);
    // 08:00 on the 10th in Seoul, now 08:00 on the 8th.
    expect(await due(page, 'Write in the body')).toEqual({
      from: Date.parse('2026-11-07T23:00:00Z'),
    });
  });

  test('a press without a drag opens the card, and Escape does not cancel a drag', async ({
    page,
  }, info) => {
    await openGantt(page, info);
    await bar(page, 'Open a card').click();
    await expect(dialog(page)).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog(page)).toHaveCount(0);
    expect(await due(page, 'Open a card')).toEqual({ from: '2026-11-12' });

    await dragBy(page, bar(page, 'Open a card'), 2, async () => {
      await page.keyboard.press('Escape');
      await expect(dragged(page)).toHaveCount(1);
    });
    expect(await due(page, 'Open a card')).toEqual({ from: '2026-11-14' });
    await expect(dialog(page)).toHaveCount(0);
  });

  test('a bar put back where it was changes nothing', async ({ page }, info) => {
    await openGantt(page, info);
    const { width } = await dayScale(page);
    const from = await pointOn(bar(page, 'Filter, sort and group'));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 5 * width, from.y, { steps: 8 });
    await page.mouse.move(from.x, from.y, { steps: 8 });
    await page.mouse.up();
    await expect(dragged(page)).toHaveCount(0);
    await expect(dialog(page)).toHaveCount(0);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-12',
      to: '2026-11-23',
    });
  });

  test('the ends are dragged: a day becomes a range, and a start stops at the end', async ({
    page,
  }, info) => {
    await openGantt(page, info);
    await row(page, 'Open a card').hover();
    await shoot(page, 'grips');
    await dragBy(page, grip(page, 'Open a card', 'end'), 2, async () => {
      await shoot(page, 'resizing');
    });
    expect(await due(page, 'Open a card')).toEqual({ from: '2026-11-12', to: '2026-11-14' });

    await dragBy(page, grip(page, 'Filter, sort and group', 'start'), 20);
    expect(await due(page, 'Filter, sort and group')).toEqual({ from: '2026-11-23' });

    await dragBy(page, grip(page, 'Keep your documents in a wiki', 'start'), -3);
    expect(await due(page, 'Keep your documents in a wiki')).toEqual({
      from: '2026-11-30',
      to: '2026-12-04',
    });
    await undo(page);
    expect(await due(page, 'Keep your documents in a wiki')).toEqual({
      from: '2026-12-03',
      to: '2026-12-04',
    });
  });

  test('at 390 pixels wide, a bar and its end are dragged the same way', async ({ page }, info) => {
    await openGantt(page, info);
    // After opening: the page is opened at the width of the project's pane.
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(scroller(page)).toBeVisible();
    await centre(page, bar(page, 'Filter, sort and group'));
    await dragBy(page, bar(page, 'Filter, sort and group'), 2, async () => {
      await expect(dragged(page)).toHaveCount(1);
      await shoot(page, '390-dragging');
    });
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-14',
      to: '2026-11-25',
    });
    // Away from the right edge, where a held end scrolls the days.
    await centre(page, grip(page, 'Filter, sort and group', 'end'));
    await row(page, 'Filter, sort and group').hover();
    await dragBy(page, grip(page, 'Filter, sort and group', 'end'), 2, async () => {
      await shoot(page, '390-resizing');
    });
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-14',
      to: '2026-11-27',
    });
    await shoot(page, '390-dropped');
  });

  test('a change the backend refuses puts the bar back', async ({ page }, info) => {
    await openGantt(page, info);
    const share = bar(page, 'Filter, sort and group');
    const before = (await share.boundingBox())?.x ?? 0;
    // The next change of a card is refused; the calls of the backend are counted.
    await page.evaluate(() => {
      const backend = (
        window as unknown as {
          fvociDev: {
            backend: {
              failNext: (method: string, error: Error) => void;
              patchCard: (...args: unknown[]) => Promise<unknown>;
            };
          };
        }
      ).fvociDev.backend;
      const patch = backend.patchCard.bind(backend);
      Reflect.set(window, 'patches', 0);
      backend.patchCard = (...args: unknown[]) => {
        Reflect.set(window, 'patches', Number(Reflect.get(window, 'patches')) + 1);
        return patch(...args);
      };
      backend.failNext('patchCard', new Error('refused by the test'));
    });
    await dragBy(page, share, 4);
    // The drop was written, and refused.
    await expect.poll(() => page.evaluate(() => Number(Reflect.get(window, 'patches')))).toBe(1);
    await expect.poll(async () => (await share.boundingBox())?.x).toBeCloseTo(before, 0);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-12',
      to: '2026-11-23',
    });
  });

  test('near the right edge the days scroll and the axis grows', async ({ page }, info) => {
    await openGantt(page, info);
    const lastBefore = (await gantt(page).getAttribute('data-last')) ?? '';
    const box = await scroller(page).boundingBox();
    if (!box) throw new Error('no chart');
    // Hover waits for a stable, hittable bar before the coordinate-driven gesture.
    await bar(page, 'Open a card').hover();
    const from = await pointOn(bar(page, 'Open a card'));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width - 4, from.y, { steps: 10 });
    // Held at the edge: the chart scrolls on by itself until it comes to the end of the axis,
    // which grows, and the bar goes on past where the axis ended: let go once it is there.
    await expect
      .poll(async () => (await gantt(page).getAttribute('data-last')) ?? '', { timeout: 10_000 })
      .not.toBe(lastBefore);
    await expect
      .poll(
        async () => ((await dragged(page).getAttribute('data-drag-first')) ?? '') > lastBefore,
        {
          timeout: 10_000,
        },
      )
      .toBe(true);
    await page.mouse.up();
    const moved = (await due(page, 'Open a card')) as { from: string };
    expect(moved.from > lastBefore).toBe(true);
    expect(await scroller(page).evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);
  });
});

test.describe('a finger', () => {
  test.skip(({ hasTouch }) => !hasTouch, 'a finger is tested in the touch projects');

  test('rests on a bar to drag it; moved at once, it scrolls', async ({ page }, info) => {
    await openGantt(page, info);
    const finger = await Finger.on(page);
    const { width } = await dayScale(page);

    // Start in the visible days: the initial bar can extend past a phone's right edge.
    await centre(page, bar(page, 'Filter, sort and group'));
    // A swipe across a bar scrolls the chart and moves nothing.
    const left = await scroller(page).evaluate((element) => element.scrollLeft);
    const ended = await scrollEnd(page);
    const start = await pointOn(bar(page, 'Filter, sort and group'));
    await finger.down(start);
    await finger.move({ x: start.x - 120, y: start.y });
    await finger.up();
    // The chart goes on by itself after the finger is up: until the browser says it has stopped.
    await ended();
    await expect
      .poll(() => scroller(page).evaluate((element) => element.scrollLeft))
      .toBeGreaterThan(left + 40);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-12',
      to: '2026-11-23',
    });
    // Away from the edges of the days, where a held bar scrolls them.
    await centre(page, bar(page, 'Filter, sort and group'));

    // Resting on it takes it.
    const on = await pointOn(bar(page, 'Filter, sort and group'));
    await finger.down(on);
    await expect(dragged(page)).toHaveCount(1);
    await finger.move({ x: on.x + 2 * width, y: on.y });
    await finger.up();
    await expect(dragged(page)).toHaveCount(0);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-14',
      to: '2026-11-25',
    });

    // So does resting on its grip, which makes it longer.
    await centre(page, grip(page, 'Filter, sort and group', 'end'));
    const end = await pointOn(grip(page, 'Filter, sort and group', 'end'));
    await finger.down(end);
    await expect(dragged(page)).toHaveCount(1);
    await finger.move({ x: end.x + 3 * width, y: end.y });
    await finger.up();
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-14',
      to: '2026-11-28',
    });
    await undo(page);
    expect(await due(page, 'Filter, sort and group')).toEqual({
      from: '2026-11-14',
      to: '2026-11-25',
    });
  });

  test('a tap opens the card', async ({ page }, info) => {
    await openGantt(page, info);
    await bar(page, 'Open a card').tap();
    await expect(dialog(page)).toBeVisible();
  });
});

test('nothing is dragged by a reader, nor where the bars are placed by when cards were made', async ({
  page,
}, info) => {
  await openGantt(page, info);
  await expect(grip(page, 'Open a card', 'end')).toHaveCount(1);
  // Placed by when the cards were made: no grips. Then by the date again.
  const placeBy = (propertyId: string | undefined): Promise<void> =>
    page.evaluate(
      async ([wanted, viewId]) => {
        const board = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
        if (!board.board.cardProperties.some((each) => each.id === 'made')) {
          await board.changeBoard((current) => ({
            ...current,
            cardProperties: [
              ...current.cardProperties,
              { id: 'made', name: 'Made', type: 'createdTime', options: [] },
            ],
          }));
        }
        const id =
          wanted ?? board.board.cardProperties.find((each) => each.name === 'Day')?.id ?? '';
        await board.setViewFields(viewId, { dateProperty: id });
      },
      [propertyId, viewIdOf(page.url())] as const,
    );
  await placeBy('made');
  const made = gantt(page).locator('[data-bar]').first();
  await expect(made).toBeVisible();
  await expect(gantt(page).locator('[data-grip]')).toHaveCount(0);
  if (!touch(info)) {
    const from = await pointOn(made);
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 40, from.y, { steps: 6 });
    await expect(dragged(page)).toHaveCount(0);
    await page.mouse.up();
  }
  await placeBy(undefined);
  await expect(grip(page, 'Open a card', 'end')).toHaveCount(1);

  // A reader: the board as a viewer sees it.
  await page.evaluate(() => {
    const board = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board as unknown as {
      members: Record<string, unknown>[];
    };
    board.members = board.members.map((member) => ({
      ...member,
      role: 'viewer',
    }));
  });
  await expect(gantt(page).locator('[data-grip]')).toHaveCount(0);
  const share = bar(page, 'Filter, sort and group');
  const at = await pointOn(share);
  const before = await share.boundingBox();
  if (touch(info)) {
    const finger = await Finger.on(page);
    await finger.down(at);
    await page.waitForTimeout(500);
    await finger.move({ x: at.x + 40, y: at.y });
    await finger.up();
  } else {
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await page.mouse.move(at.x + 40, at.y, { steps: 6 });
    await expect(dragged(page)).toHaveCount(0);
    await page.mouse.up();
  }
  await expect(dragged(page)).toHaveCount(0);
  expect(await due(page, 'Filter, sort and group')).toEqual({
    from: '2026-11-12',
    to: '2026-11-23',
  });
  expect(before).not.toBeNull();
});

test('with the keys: Shift+F10 on a bar opens the card’s “…”, whose “Change date” moves it; one undo takes it back', async ({
  page,
}, info) => {
  await openGantt(page, info);
  const share = bar(page, 'Filter, sort and group');
  await share.focus();
  // The "…" is shown with the focus in its row, and is no stop of Tab: the bars are one stop.
  const menu = row(page, 'Filter, sort and group').locator('[data-card-menu]');
  await expect(menu).toHaveAccessibleName('Card menu: Filter, sort and group');
  await expect(menu).toHaveAttribute('tabindex', '-1');
  await expect(share).toHaveAttribute('aria-keyshortcuts', 'Shift+F10');
  await page.keyboard.press('Shift+F10');
  const change = page.locator('[data-action="change-date"]');
  await expect(change).toHaveText('Change date');
  await menuTo(page, change);
  await page.keyboard.press('Enter');

  const picker = page.getByTestId('day-picker');
  await expect(picker).toBeVisible();
  const twelfth = picker.locator('[data-bits-day][data-value="2026-11-12"]');
  if (info.project.name === 'mobile') await tabTo(page, twelfth);
  await expect(twelfth).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(picker).toBeHidden();
  await expect
    .poll(() => due(page, 'Filter, sort and group'))
    .toEqual({
      from: '2026-11-19',
      to: '2026-11-30',
    });
  // The focus is back on the bar.
  await expect(share).toBeFocused();
  await undo(page);
  expect(await due(page, 'Filter, sort and group')).toEqual({
    from: '2026-11-12',
    to: '2026-11-23',
  });

  // The menu key opens it too.
  await page.keyboard.press('ContextMenu');
  await expect(change).toBeVisible();
});
