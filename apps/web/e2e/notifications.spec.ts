/**
 * The bell of the top bar, end to end, on the in-memory backend: a comment
 * somebody else writes on a card the reader wrote shows on the bell at once,
 * without a reload; the notification opens the card at that comment and is
 * read by it; and the switch of comments, turned off in the settings, is
 * kept, and nothing is told while it is off. Runs at the three stages: the
 * bell stays in the bar at every width, and in a narrow pane opens a sheet.
 *
 * Somebody else is the other person of the same world (`fvociDev.someoneElse`).
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { openApp, showSidebar, sidebar, strip } from './shell.support.ts';

interface DevBackend {
  createCard(boardId: string, draft: { title: string }): Promise<{ id: string }>;
  createComment(boardId: string, cardId: string, body: unknown): Promise<{ id: string }>;
  settings(): Promise<Record<string, unknown>>;
}

interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

interface Place {
  boardId: string;
  cardId: string;
}

const bell = (page: Page): Locator => page.getByTestId('open-notifications');
const badge = (page: Page): Locator => page.getByTestId('unread-notifications');
const inbox = (page: Page): Locator => page.getByTestId('notifications');

/** Opens the list by the bell: a popover, or a sheet of the whole screen in a narrow pane. */
async function openInbox(page: Page): Promise<void> {
  await bell(page).click();
}

async function expectBellNamed(page: Page, name: string): Promise<void> {
  await expect(bell(page)).toHaveAccessibleName(name);
}

/** A card the reader writes on the board shown, which the other person is a member of. */
async function cardOfMine(page: Page, title: string): Promise<Place> {
  const boardId = /\/boards\/([^/]+)\//.exec(new URL(page.url()).pathname)?.[1] ?? '';
  const cardId = await page.evaluate(
    async ({ boardId, title }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      return (await dev.backend.createCard(boardId, { title })).id;
    },
    { boardId, title },
  );
  return { boardId, cardId };
}

/** The other person comments on the card: the comment's identifier. */
function theyComment(page: Page, place: Place, words: string): Promise<string> {
  return page.evaluate(
    async ({ boardId, cardId, words }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const body = {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'text', text: words }] }],
      };
      return (await dev.someoneElse().createComment(boardId, cardId, body)).id;
    },
    { ...place, words },
  );
}

/** Opens the settings from the reader's account: on the strip where there is one, else in the sidebar. */
async function openSettings(page: Page): Promise<Locator> {
  if (await strip(page).isVisible()) {
    await strip(page).getByTestId('strip-open-settings').click();
  } else {
    await showSidebar(page);
    await sidebar(page).getByTestId('open-settings').click();
  }
  const dialog = page.getByTestId('settings-dialog');
  await expect(dialog).toBeVisible();
  return dialog;
}

test('a comment on the reader’s card rings the bell at once, and opens the card at the comment', async ({
  page,
}) => {
  await openApp(page);
  const place = await cardOfMine(page, 'Watched plan');
  await expect(bell(page)).toBeVisible();
  await expect(badge(page)).toBeHidden();

  const commentId = await theyComment(page, place, 'Looks good to me');
  await expect(badge(page)).toHaveText('1');
  await expectBellNamed(page, 'Notifications, 1 unread');

  await openInbox(page);
  const line = inbox(page).getByRole('link', { name: /commented on Watched plan/ });
  await expect(line).toBeVisible();
  await expect(inbox(page).locator('[data-unread]')).toHaveCount(1);
  await line.click();

  await expect(inbox(page)).toBeHidden();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-title')).toHaveValue('Watched plan');
  await expect(page.locator(`[data-comment="${commentId}"]`)).toBeInViewport();
  await expect(badge(page)).toBeHidden();
  // The card closed, the bell lists it read.
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  await openInbox(page);
  await expect(inbox(page).locator('[data-notification]')).toHaveCount(1);
  await expect(inbox(page).locator('[data-unread]')).toHaveCount(0);
});

test('the switch of comments is kept off, and nothing is told while it is', async ({ page }) => {
  await openApp(page);
  const dialog = await openSettings(page);
  const comments = dialog.locator('[data-setting="notifyComments"]').getByRole('switch');
  await expect(comments).toBeChecked();
  await comments.click();
  await expect(comments).not.toBeChecked();
  await expect
    .poll(() =>
      page.evaluate(() => (window as unknown as { fvociDev: Dev }).fvociDev.backend.settings()),
    )
    .toMatchObject({ notifyComments: false, notifyMentions: true, notifyAssignments: true });

  // Kept: the page loads again with it off.
  await openApp(page);
  const again = await openSettings(page);
  const kept = again.locator('[data-setting="notifyComments"]').getByRole('switch');
  await expect(kept).not.toBeChecked();
  await page.keyboard.press('Escape');
  await expect(again).toBeHidden();

  const place = await cardOfMine(page, 'Quiet plan');
  await theyComment(page, place, 'Not told');
  // Turned on again (and saved, as the backend reads it when a comment comes): only
  // what comes after is told.
  const last = await openSettings(page);
  const on = last.locator('[data-setting="notifyComments"]').getByRole('switch');
  await on.click();
  await expect(on).toBeChecked();
  await expect
    .poll(() =>
      page.evaluate(() => (window as unknown as { fvociDev: Dev }).fvociDev.backend.settings()),
    )
    .toMatchObject({ notifyComments: true });
  await page.keyboard.press('Escape');
  await theyComment(page, place, 'Told');
  await expect(badge(page)).toHaveText('1');
  await openInbox(page);
  await expect(inbox(page).getByRole('link', { name: /commented on Quiet plan/ })).toBeVisible();
  await expect(inbox(page).getByRole('link', { name: /2 times/ })).toHaveCount(0);
});

test.describe('the notifications sheet of a narrow pane', () => {
  test.use({ viewport: { width: 393, height: 800 }, hasTouch: true });

  test('is opened by the bell, takes the focus, closes on back without leaving the page, and returns the focus to the bell', async ({
    page,
  }) => {
    await openApp(page);
    const firstUrl = page.url();
    // Navigate to a second board first so an unhandled back would leave the current page.
    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Issue tracker' }).click();
    await expect(page.getByTestId('board-title')).toHaveValue('Issue tracker');
    const secondUrl = page.url();
    expect(secondUrl).not.toBe(firstUrl);

    // The bell stands in the bar, outside "…", and opens the sheet: the focus moves into it.
    await expect(page.getByTestId('bar-menu')).toBeVisible();
    await openInbox(page);
    await expect(inbox(page)).toBeVisible();
    await expect(page.getByRole('menu')).toHaveCount(0);
    await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
    await expect
      .poll(() => inbox(page).evaluate((dialog) => dialog.contains(document.activeElement)))
      .toBe(true);

    // Wait for the sheet's overlay entry, then press Back: the sheet closes and the page underneath stays.
    await expect
      .poll(() =>
        page.evaluate(() =>
          Boolean((history.state as Record<string, unknown> | null)?.fvociOverlay),
        ),
      )
      .toBe(true);
    await page.goBack();
    await expect(inbox(page)).toBeHidden();
    expect(page.url()).toBe(secondUrl);
    await expect(page.getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(bell(page)).toBeFocused();

    // Escape closes the sheet and returns focus to the bell.
    await openInbox(page);
    await expect(inbox(page)).toBeVisible();
    await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
    await expect
      .poll(() =>
        page.evaluate(() =>
          Boolean((history.state as Record<string, unknown> | null)?.fvociOverlay),
        ),
      )
      .toBe(true);
    await page.keyboard.press('Escape');
    await expect(inbox(page)).toBeHidden();
    await expect(bell(page)).toBeFocused();

    // Closing with Escape gave the overlay entry back, so Back returns to the first board.
    await expect
      .poll(() =>
        page.evaluate(() =>
          Boolean((history.state as Record<string, unknown> | null)?.fvociOverlay),
        ),
      )
      .toBe(false);
    await page.goBack();
    await expect(page).toHaveURL(firstUrl);
  });
});
