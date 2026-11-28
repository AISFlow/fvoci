/**
 * The inbox sorted, end to end, on the in-memory backend: with the focus in
 * the list, J goes to the next line and K back, U marks one unread again and
 * E puts it away, in the archive's tab; a line's menu brings it back; and in
 * a phone's sheet a finger swipes a line away, and the menu is 44 pixels a
 * side.
 *
 * Somebody else is the other person of the same world (`fvociDev.someoneElse`).
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { openApp } from './shell.support.ts';

interface DevBackend {
  createCard(boardId: string, draft: { title: string }): Promise<{ id: string }>;
  createComment(boardId: string, cardId: string, body: unknown): Promise<{ id: string }>;
}

interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

const bell = (page: Page): Locator => page.getByTestId('open-notifications');
const inbox = (page: Page): Locator => page.getByTestId('notifications');
const lines = (page: Page): Locator => inbox(page).locator('[data-notification]');
const tab = (page: Page, name: string): Locator =>
  inbox(page).getByTestId('notification-tabs').getByRole('tab', { name });

/** Two cards of the reader's, each commented on by the other person: two lines, the second card's first. */
async function toldTwice(page: Page): Promise<void> {
  const boardId = /\/boards\/([^/]+)\//.exec(new URL(page.url()).pathname)?.[1] ?? '';
  await page.evaluate(async (boardId) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Seen it' }] }],
    };
    for (const title of ['First plan', 'Second plan']) {
      const card = await dev.backend.createCard(boardId, { title });
      await dev.someoneElse().createComment(boardId, card.id, body);
    }
  }, boardId);
  await expect(page.getByTestId('unread-notifications')).toHaveText('2');
}

test('the keys of the list go from line to line, mark one unread and put one away', async ({
  page,
}) => {
  await openApp(page);
  await toldTwice(page);
  await bell(page).click();
  await expect(lines(page)).toHaveCount(2);
  await expect(tab(page, 'All')).toHaveAttribute('aria-selected', 'true');

  const first = lines(page).nth(0).locator('[data-row-focus]');
  const second = lines(page).nth(1).locator('[data-row-focus]');
  await first.focus();
  await page.keyboard.press('j');
  await expect(second).toBeFocused();
  await page.keyboard.press('k');
  await expect(first).toBeFocused();

  // U reads it, and again marks it unread.
  await page.keyboard.press('u');
  await expect(inbox(page).locator('[data-unread]')).toHaveCount(1);
  await page.keyboard.press('u');
  await expect(inbox(page).locator('[data-unread]')).toHaveCount(2);

  // E puts it away: the focus goes on to the line after it.
  await page.keyboard.press('e');
  await expect(lines(page)).toHaveCount(1);
  await expect(lines(page).first().locator('[data-row-focus]')).toBeFocused();
  await expect(lines(page).first()).toContainText('First plan');
  await expect(page.getByTestId('unread-notifications')).toHaveText('1');

  await tab(page, 'Unread').click();
  await expect(lines(page)).toHaveCount(1);
  await tab(page, 'Archive').click();
  await expect(lines(page)).toHaveCount(1);
  await expect(lines(page).first()).toContainText('Second plan');
  await expect(lines(page).first()).not.toHaveAttribute('data-unread');

  // Its menu brings it back into the inbox, read.
  await lines(page).first().getByTestId('notification-menu').click();
  await page.getByRole('menuitem', { name: 'Move to inbox' }).click();
  await expect(lines(page)).toHaveCount(0);
  await tab(page, 'All').click();
  await expect(lines(page)).toHaveCount(2);
  await expect(inbox(page).locator('[data-unread]')).toHaveCount(1);
});

test.describe('the notifications sheet of a phone', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('a finger swipes a line away, and its menu is in reach', async ({ page }) => {
    await openApp(page);
    await toldTwice(page);
    await bell(page).click();
    await expect(lines(page)).toHaveCount(2);
    const menu = lines(page).first().getByTestId('notification-menu');
    // Its size once the sheet has opened: it zooms in from 95 percent (`zoom-in-95`).
    await expect
      .poll(async () => {
        const box = await menu.boundingBox();
        return box && [Math.round(box.width), Math.round(box.height)];
      })
      .toEqual([44, 44]);

    const line = lines(page).first();
    const at = await line.boundingBox();
    if (!at) throw new Error('the line is not shown');
    const y = at.y + at.height / 2;
    const touch = { pointerType: 'touch', isPrimary: true, pointerId: 7, clientY: y };
    await line.dispatchEvent('pointerdown', { ...touch, clientX: at.x + at.width - 60 });
    for (const step of [20, 60, 120, 180]) {
      await line.dispatchEvent('pointermove', {
        ...touch,
        clientX: at.x + at.width - 60 - step,
      });
    }
    await line.dispatchEvent('pointerup', { ...touch, clientX: at.x + at.width - 240 });
    await expect(lines(page)).toHaveCount(1);
    await expect(inbox(page)).toBeVisible();
  });
});
