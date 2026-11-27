/**
 * The head of a board, on the in-memory backend: the faces of who is here now and the
 * number of the board's members, which opens the members of the sharing dialog. Others
 * come onto the board as other users of the same world (`fvociDev.backend.as`).
 */
import { expect, test, type Page } from '@playwright/test';

import { othersArrive, type Dev } from './presence.support.ts';
import {
  boardLink,
  openApp,
  pressInBar,
  READ_ONLY_BOARD,
  showSidebar,
  topBar,
} from './shell.support.ts';

const people = (page: Page) => topBar(page).getByTestId('board-people');
const presence = (page: Page) => topBar(page).getByTestId('board-presence');
const faces = (page: Page) => presence(page).locator('[data-avatar]:visible');
/** The "+k" that shows, if any. */
const plus = (page: Page) => presence(page).locator('span:visible', { hasText: '+' });

test('alone, the head of the board shows nobody here and the number of members', async ({
  page,
}) => {
  await openApp(page);
  const members = await page.evaluate(
    () => (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board.members.length,
  );
  // In a narrow pane the button shows a figure and the number; its name is the same.
  await expect(topBar(page).getByTestId('board-members-count')).toHaveAccessibleName(
    `Members ${members}`,
  );
  await expect(people(page)).toHaveAttribute(
    'aria-label',
    `People viewing this board: 0, members: ${members}`,
  );
  await expect(faces(page)).toHaveCount(0);
  await expect(plus(page)).toHaveCount(0);
});

test('five faces in a wide pane and two in a narrow one, then the rest as a number', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openApp(page);
  const members = await othersArrive(page, 7);
  await expect(presence(page)).toHaveAccessibleName('People viewing this board: 7');
  await expect(people(page)).toHaveAttribute(
    'aria-label',
    `People viewing this board: 7, members: ${members}`,
  );
  await expect(faces(page)).toHaveCount(5);
  await expect(plus(page)).toHaveText('+2');
  const count = topBar(page).getByTestId('board-members-count');
  await expect(count).toHaveText(`Members ${members}`, { useInnerText: true });
  // Wide, the members have their own button: the list of who is here does not end in them
  // again (it opens outside the pane, where the pane's container queries do not reach).
  await presence(page).click();
  await expect(page.getByTestId('presence-list')).toBeVisible();
  await expect(page.getByTestId('presence-members')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('presence-list')).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(faces(page)).toHaveCount(2);
  await expect(plus(page)).toHaveText('+5');
  // Narrow, with others here, the faces alone keep the title its room; their list ends in
  // the members, which open the sharing dialog at its members.
  await expect(count).toBeHidden();
  await presence(page).click();
  await page.getByTestId('presence-members').click();
  await expect(page.getByTestId('share-dialog')).toBeVisible();
  // Once the list's sheet has gone too: it gives the focus back to the faces as it goes, which
  // must not pull it out of the dialog.
  await expect(page.getByTestId('presence-list')).toHaveCount(0);
  await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
  await expect(page.getByTestId('share-members-title')).toBeFocused();
});

test('the number of members opens the sharing dialog at its members', async ({ page }) => {
  await openApp(page);
  await topBar(page).getByTestId('board-members-count').click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('share-members-title')).toBeFocused();
  // An admin changes them here: the search to add people is there.
  await expect(dialog.getByTestId('share-search')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // The share button opens the same dialog at its top, with the focus in it.
  await pressInBar(page, 'share');
  await expect(dialog.getByTestId('share-open')).toBeVisible();
  await expect(dialog.locator(':focus')).toHaveCount(1);
  await expect(dialog.getByTestId('share-members-title')).not.toBeFocused();
});

test('a reader who is no admin sees the members and changes nothing', async ({ page }) => {
  await openApp(page);
  await showSidebar(page);
  await boardLink(page, READ_ONLY_BOARD).click();
  await expect(topBar(page).getByTestId('read-only')).toBeVisible();
  await expect(topBar(page).getByTestId('share-board')).toHaveCount(0);
  await topBar(page).getByTestId('board-members-count').click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-members-title')).toBeFocused();
  await expect(dialog.locator('[data-slot="dialog-title"]')).toHaveText('Members');
  await expect(dialog.getByTestId('share-member-role').first()).toBeVisible();
  for (const control of ['share-search', 'share-guest', 'share-open', 'share-public']) {
    await expect(dialog.getByTestId(control)).toHaveCount(0);
  }
  await expect(dialog.getByRole('combobox')).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: /^Remove / })).toHaveCount(0);
});
