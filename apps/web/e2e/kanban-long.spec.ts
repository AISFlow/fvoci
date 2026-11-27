/**
 * A kanban column of 400 cards (kanban-long.support.ts), which draws a window
 * of them: those near what is in sight, the focus, the gap of a drag and the
 * card that is open. Everything a card does still works far down the column.
 * A mouse and a finger drag a card to the bottom edge, where the column
 * scrolls by itself past cards not drawn, and drop it where the gap is;
 * several selected cards go together; the keyboard goes from card to card
 * past the window and carries a card to either end; a selection stays on
 * cards out of sight; a card opened by its address has the focus, in sight,
 * once it is closed; and a card made at the end of the column is in sight.
 * Where a card is, is read from what a screen reader is told (`aria-posinset`).
 */
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';

import { primaryName } from './keyboard.support.ts';
import {
  announcer,
  card,
  cardsBox,
  column,
  dialog,
  expectNoDrag,
  Finger,
  fitPane,
  kanban,
  overlay,
  paneOf,
  pointOn,
  titlesIn,
} from './kanban.support.ts';
import {
  cardIdOf,
  LONG_COLUMN,
  longTitle,
  makeLongColumn,
  showView,
} from './kanban-long.support.ts';

// Enough for the column to be many times higher than the view, and quick to make in
// every project.
const MANY = 400;
/** The one card of the seed in the long column, and the cards made for it. */
const ALL = MANY + 1;

/** Opens the long column; `wordy`: every third title of two lines, higher than guessed. */
async function openLong(page: Page, info: TestInfo, wordy = false): Promise<void> {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await fitPane(page, paneOf(info));
  await showView(page, await makeLongColumn(page, MANY, wordy), ALL);
  await expect(column(page, LONG_COLUMN).locator('[data-column-count]')).toContainText(String(ALL));
  expect(errors).toEqual([]);
}

const list = (page: Page): Locator => cardsBox(page, LONG_COLUMN);

const scrollTop = (page: Page): Promise<number> =>
  list(page).evaluate((element) => element.scrollTop);

/** Where a card drawn is in its column, as a screen reader is told, from 1. */
async function positionOf(page: Page, title: string): Promise<number> {
  const matched = page.locator('[data-card]').filter({
    has: page.locator('[data-slot="card-title-text"]', { hasText: new RegExp(`^${title}$`) }),
  });
  return Number(await matched.locator('xpath=..').getAttribute('aria-posinset'));
}

/** Scrolls the long column, and nothing around it, until the card is in its middle. */
async function centre(page: Page, target: Locator): Promise<void> {
  const box = await target.boundingBox();
  if (!box) throw new Error('the card is not on the page');
  await list(page).evaluate((element, { y, height }) => {
    const within = element.getBoundingClientRect();
    element.scrollTop += y - within.top - (element.clientHeight - height) / 2;
  }, box);
}

/**
 * Picks a card up with the mouse near its top: its middle may lie near the bottom edge of the
 * column, where the column scrolls by itself.
 */
async function pickUp(page: Page, target: Locator): Promise<void> {
  const from = await pointOn(target, 0.2);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 6, from.y - 6, { steps: 3 });
  await expect(overlay(page)).toBeVisible();
}

async function scrollTo(page: Page, where: 'start' | 'end'): Promise<void> {
  await list(page).evaluate((element, end) => {
    element.scrollTop = end ? element.scrollHeight : 0;
  }, where === 'end');
}

/** The title of the card that has the focus, if a card has it. */
const focusedTitle = (page: Page): Promise<string> =>
  page.evaluate(
    () =>
      document.activeElement
        ?.closest('[data-card]')
        ?.querySelector('[data-slot="card-title-text"]')
        ?.textContent.trim() ?? '',
  );

test.describe('a column of 400 cards', () => {
  test('draws a window of its cards and tells a screen reader of every one', async ({
    page,
  }, info) => {
    await openLong(page, info);
    const drawn = await list(page).locator('[data-card]').count();
    expect(drawn).toBeGreaterThan(0);
    expect(drawn).toBeLessThan(ALL / 4);
    await expect(list(page).getByRole('listitem').first()).toHaveAttribute(
      'aria-setsize',
      String(ALL),
    );
    await expect(card(page, longTitle(MANY))).toHaveCount(0);

    await scrollTo(page, 'end');
    await expect(card(page, longTitle(MANY))).toBeInViewport();
    expect(await positionOf(page, longTitle(MANY))).toBe(ALL);
    expect(await list(page).locator('[data-card]').count()).toBeLessThan(ALL / 4);
  });

  test('the keyboard goes from card to card past the window, and carries a card to either end', async ({
    page,
  }, info) => {
    await openLong(page, info);
    const goal = 24;
    await card(page, longTitle(1)).focus();
    // A card's menu stands between it and the next card.
    const reached = [await focusedTitle(page)];
    for (let press = 0; press < goal * 2 && reached.at(-1) !== longTitle(goal); press += 1) {
      await page.keyboard.press('Tab');
      const title = await focusedTitle(page);
      if (title !== '' && title !== reached.at(-1)) reached.push(title);
    }
    expect(reached).toEqual(Array.from({ length: goal }, (_, at) => longTitle(at + 1)));
    const carried = card(page, longTitle(goal));
    await expect(carried).toBeFocused();
    await expect(carried).toBeInViewport();
    // Scrolled away from the focus, the keyboard still goes on from it to the next card.
    await scrollTo(page, 'end');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Tab');
    await expect(card(page, longTitle(goal + 1))).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Shift+Tab');
    await expect(carried).toBeFocused();

    await page.keyboard.press('Space');
    await expect(kanban(page)).toHaveAttribute('data-dragging', 'keyboard');
    await expect(announcer(page)).toHaveText(
      `Picked up ${longTitle(goal)}. ${LONG_COLUMN}, position ${String(goal + 1)} of ${String(ALL)}.`,
    );
    await page.keyboard.press('End');
    await expect(page.locator('[data-drag-carried]')).toBeFocused();
    await expect(page.locator('[data-drag-carried]')).toBeInViewport();
    await page.keyboard.press('Space');
    await expect(announcer(page)).toHaveText(
      `Dropped ${longTitle(goal)} in ${LONG_COLUMN}, position ${String(ALL)} of ${String(ALL)}.`,
    );
    await expect(carried).toBeFocused();
    await expect(carried).toBeInViewport();
    await expect.poll(() => positionOf(page, longTitle(goal))).toBe(ALL);

    await page.keyboard.press('Space');
    await page.keyboard.press('Home');
    await expect(page.locator('[data-drag-carried]')).toBeInViewport();
    await page.keyboard.press('Space');
    await expect(announcer(page)).toHaveText(
      `Dropped ${longTitle(goal)} in ${LONG_COLUMN}, position 1 of ${String(ALL)}.`,
    );
    await expect(carried).toBeFocused();
    await expect(carried).toBeInViewport();
    await expect.poll(() => positionOf(page, longTitle(goal))).toBe(1);
  });

  test('a selection stays on a card that went out of sight and back', async ({
    page,
    hasTouch,
  }, info) => {
    test.skip(hasTouch, 'a mouse');
    await openLong(page, info);
    const modifier = await primaryName(page);
    const first = card(page, longTitle(1));
    await centre(page, first);
    await first.click({ modifiers: [modifier] });
    await scrollTo(page, 'end');
    await card(page, longTitle(MANY)).click({ modifiers: [modifier] });
    await expect(page.getByTestId('selection-bar')).toContainText('2 selected');
    await scrollTo(page, 'start');
    await expect(first).toHaveAttribute('data-selected');
    // The two go together: dropped before the card above, the one far down comes along. That
    // card in the middle of the column, away from the edges where the column scrolls by itself.
    const before = await titlesIn(page, LONG_COLUMN);
    const above = before[before.indexOf(longTitle(1)) - 1] ?? '';
    const aboveCard = page.locator('[data-card]').filter({
      has: page.locator('[data-slot="card-title-text"]', {
        hasText: new RegExp(`^${above}$`),
      }),
    });
    await centre(page, aboveCard);
    await expect(first).toBeInViewport();

    await pickUp(page, first);
    await expect(overlay(page).locator('[data-slot="card-count"]')).toHaveText('2');
    const top = await pointOn(aboveCard, 0.25);
    await page.mouse.move(top.x, top.y, { steps: 6 });
    await expect(list(page).locator('[data-drag-gap]')).toBeVisible();
    await page.mouse.up();
    await expectNoDrag(page);
    const at = await positionOf(page, longTitle(1));
    await expect.poll(() => positionOf(page, above)).toBe(at + 2);
    expect(await positionOf(page, longTitle(MANY))).toBe(at + 1);
  });

  test('a card opened by its address has the focus, in sight, once it is closed', async ({
    page,
  }, info) => {
    await openLong(page, info, true);
    const far = card(page, longTitle(300));
    await expect(far).toHaveCount(0);
    const cardId = await cardIdOf(page, longTitle(300));
    await page.evaluate((id) => {
      history.pushState(null, '', `${location.pathname}/${encodeURIComponent(id)}`);
      window.dispatchEvent(new PopStateEvent('popstate'));
    }, cardId);
    await expect(dialog(page)).toContainText(longTitle(300));
    await page.keyboard.press('Escape');
    await expect(dialog(page)).toBeHidden();
    await expect(far).toBeFocused();
    await expect(far).toBeInViewport();
  });

  test('a card made at the end of the column is in sight once it is closed', async ({
    page,
  }, info) => {
    await openLong(page, info, true);
    await column(page, LONG_COLUMN).locator('[data-column-new]').click();
    await expect(dialog(page)).toBeVisible();
    await page.goBack();
    await expect(dialog(page)).toBeHidden();
    await expect(column(page, LONG_COLUMN).locator('[data-column-count]')).toContainText(
      String(ALL + 1),
    );
    const made = list(page).locator('[data-card]').last();
    await expect(made).toBeFocused();
    await expect(made).toBeInViewport();
    await expect(made.locator('xpath=..')).toHaveAttribute('aria-posinset', String(ALL + 1));
  });
});

test('a long column moved among the others keeps its place and its cards in sight', async ({
  page,
}, info) => {
  await openLong(page, info);
  await centre(page, card(page, longTitle(1)));
  await list(page).evaluate((element) => {
    element.scrollTop += 20_000;
  });
  const cardInSight = (): Promise<boolean> =>
    list(page).evaluate((element) => {
      const box = element.getBoundingClientRect();
      return [...element.querySelectorAll('[data-card]')].some((each) => {
        const cardBox = each.getBoundingClientRect();
        return cardBox.bottom > box.top && cardBox.top < box.bottom;
      });
    });
  // Wait for the window at the new scroll depth to mount and settle ResizeObserver anchoring.
  await expect(card(page, longTitle(1))).toHaveCount(0);
  await expect.poll(cardInSight).toBe(true);
  await list(page).evaluate(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            resolve();
          });
        });
      }),
  );
  const before = await scrollTop(page);
  const heading = column(page, LONG_COLUMN).locator('[data-column-heading]');
  await heading.focus();

  // Carrying the column with the keyboard and cancelling keeps its scroll and cards in sight.
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await expect.poll(async () => Math.abs((await scrollTop(page)) - before)).toBeLessThanOrEqual(1);
  expect(await cardInSight()).toBe(true);
  await page.keyboard.press('Escape');
  await expect(heading).toBeFocused();
  await expect.poll(async () => Math.abs((await scrollTop(page)) - before)).toBeLessThanOrEqual(1);
  expect(await cardInSight()).toBe(true);

  // Moving and dropping it keeps its scroll and cards in sight too.
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(heading).toBeFocused();
  // Within a pixel: at three pixels to a point, WebKit rounds where it scrolls to.
  await expect.poll(async () => Math.abs((await scrollTop(page)) - before)).toBeLessThanOrEqual(1);
  expect(await cardInSight()).toBe(true);
});

test.describe('dragging down a column of 400 cards', () => {
  /**
   * Holds the card at the bottom edge of the column until the column has
   * scrolled by itself, scrolls it on far past the cards drawn when the drag
   * began, as a wheel would, and drops the card where the gap is: next to the
   * card the gap stands before.
   */
  async function dragToEdge(
    page: Page,
    move: (to: { x: number; y: number }) => Promise<void>,
    drop: () => Promise<void>,
  ): Promise<void> {
    const drawnBefore = await titlesIn(page, LONG_COLUMN);
    const box = await list(page).boundingBox();
    if (!box) throw new Error('no column');
    const edge = { x: box.x + box.width / 2, y: box.y + box.height - 6 };
    await move(edge);
    await expect
      .poll(async () => {
        await move(edge);
        return scrollTop(page);
      })
      .toBeGreaterThan(box.height * 1.5);
    await list(page).evaluate((element) => {
      element.scrollTop += 4000;
    });
    // Away from the edge, the gap stands before a card in sight.
    await move({ x: edge.x, y: box.y + box.height / 2 - 4 });
    await move({ x: edge.x, y: box.y + box.height / 2 });
    const gap = list(page).locator('[data-drag-gap]');
    await expect(gap).toBeInViewport();
    const after = await gap.evaluate(
      (element) =>
        element.nextElementSibling?.querySelector('[data-slot="card-title-text"]')?.textContent ??
        '',
    );
    expect(after).toMatch(/^Long \d+$/);
    // A card the column did not draw when the drag began.
    expect(drawnBefore).not.toContain(after);
    await drop();
    await expectNoDrag(page);
    await expect
      .poll(() => positionOf(page, after))
      .toBe((await positionOf(page, longTitle(1))) + 1);
    await expect(list(page).getByRole('listitem').first()).toHaveAttribute(
      'aria-setsize',
      String(ALL),
    );
  }

  test('a mouse', async ({ page, hasTouch }, info) => {
    test.skip(hasTouch, 'a mouse');
    await openLong(page, info);
    await centre(page, card(page, longTitle(1)));
    await pickUp(page, card(page, longTitle(1)));
    await dragToEdge(
      page,
      (to) => page.mouse.move(to.x, to.y, { steps: 2 }),
      () => page.mouse.up(),
    );
  });

  test('a finger', async ({ page, hasTouch }, info) => {
    test.skip(!hasTouch, 'a finger');
    await openLong(page, info);
    const finger = await Finger.on(page);
    await centre(page, card(page, longTitle(1)));
    await finger.rest(card(page, longTitle(1)), 0.2);
    await dragToEdge(
      page,
      (to) => finger.move(to, 2),
      () => finger.up(),
    );
  });
});
