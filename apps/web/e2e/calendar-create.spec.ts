/**
 * Making a card on a day of the calendar: "New card
 * on …" at the end of the cards of a day, wherever they are listed, and with
 * a mouse the "+" of a day's cell. The card has that day as its date and
 * opens, as on a board; a view's filter gets the values it asks for,
 * as a board and a table give them. One undo takes the card away.
 *
 * The same welcome board, moment and projects as e2e/calendar.spec.ts.
 */
import { expect, test, type Page } from '@playwright/test';

import { primaryKey, tabKey } from './keyboard.support.ts';
import {
  calendar,
  CALENDAR_PAGE,
  day,
  dayButton,
  dueOf,
  focused,
  listOf,
  openCalendar,
  tabTo,
  wide,
} from './calendar.support.ts';
import { dialog, touch } from './kanban.support.ts';

test.use(CALENDAR_PAGE);

/** How many cards the open board has, as the board store has them. */
async function cardCount(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      (window as unknown as { fvociDev: { workspace: { board: { cards: unknown[] } } } }).fvociDev
        .workspace.board.cards.length,
  );
}

test('a card is made on an empty day with the keys alone, and one undo takes it away', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  const before = await cardCount(page);
  await expect(dayButton(page, '2026-11-25')).toHaveAccessibleName(/^Wednesday, November 25$/);
  await page.getByTestId('calendar-undated').focus();
  await page.keyboard.press(await tabKey(page));
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(focused(page)).toHaveAccessibleName(/^Wednesday, November 25/);
  if (wide(info)) await page.keyboard.press('Enter');
  const list = listOf(page, info);
  await expect(list).toContainText('No cards on this day.');
  await tabTo(page, list.getByRole('button', { name: 'New card on November 25' }));
  await page.keyboard.press('Enter');

  // The card opens, asking for its title; it has the day as its date.
  await expect(dialog(page)).toBeVisible();
  await expect.poll(() => cardCount(page)).toBe(before + 1);
  const made = await page.evaluate(() => {
    const store = (
      window as unknown as {
        fvociDev: {
          workspace: { board: { cards: { id: string; title: string; createdAt: number }[] } };
        };
      }
    ).fvociDev.workspace.board;
    return [...store.cards].sort((a, b) => b.createdAt - a.createdAt)[0]?.title ?? 'none';
  });
  expect(made).toBe('');
  await expect.poll(() => dueOf(page, '')).toEqual({ from: '2026-11-25' });
  // The first Escape leaves the title, the second closes the card.
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  await expect(dayButton(page, '2026-11-25')).toHaveAccessibleName(/November 25, 1 card$/);

  // Back on the new bar with a mouse; elsewhere, on the day, where the card has no bar to go to.
  await expect(focused(page)).toHaveAccessibleName(/November 25/);
  await page.keyboard.press(await primaryKey(page, 'z'));
  await expect.poll(() => cardCount(page)).toBe(before);
  await expect(dayButton(page, '2026-11-25')).toHaveAccessibleName(/^Wednesday, November 25$/);
});

test('with a mouse the "+" of a day makes a card there; under a finger the day is one button', async ({
  page,
}, info) => {
  await openCalendar(page, info);
  const before = await cardCount(page);
  const cell = day(page, '2026-11-26');
  const plus = calendar(page).locator('[data-new-card="2026-11-26"]');
  if (touch(info) || !wide(info)) {
    // The cell is the button of the day; cards are made from its list.
    await expect(plus).toHaveCount(0);
    return;
  }
  await expect(plus).toHaveCSS('opacity', '0');
  await cell.hover();
  await expect(plus).toHaveCSS('opacity', '1');
  // A double click makes one card: the second press comes while the first is being made.
  await plus.dblclick();
  await expect(dialog(page)).toBeVisible();
  await expect.poll(() => cardCount(page)).toBe(before + 1);
  await expect.poll(() => dueOf(page, '')).toEqual({ from: '2026-11-26' });
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  // Back on the card's new bar.
  await expect(focused(page)).toHaveAttribute('data-bar', /.+/);
});

test('the header’s “New” makes the card on the day picked', async ({ page }, info) => {
  await openCalendar(page, info);
  const before = await cardCount(page);
  // A day is picked: its cards open, and are closed again.
  await dayButton(page, '2026-11-25').click();
  if (wide(info)) {
    await expect(listOf(page, info)).toBeVisible();
    await page.keyboard.press('Escape');
  }
  await expect(calendar(page)).toHaveAttribute('data-selected', '2026-11-25');
  await page.locator('[data-testid="new-card"]:visible').first().click();
  await expect(dialog(page)).toBeVisible();
  await expect.poll(() => cardCount(page)).toBe(before + 1);
  const newest = await page.evaluate(() => {
    const store = (
      window as unknown as {
        fvociDev: {
          workspace: {
            board: {
              board: { cardProperties: { id: string; name: string }[] };
              cards: { createdAt: number; fields: { properties: Record<string, unknown> } }[];
            };
          };
        };
      }
    ).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((each) => each.name === 'Day')?.id ?? '';
    const last = [...store.cards].sort((a, b) => b.createdAt - a.createdAt)[0];
    return last?.fields.properties[due];
  });
  expect(newest).toEqual({ from: '2026-11-25' });
});
