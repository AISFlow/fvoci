/**
 * The reader's own space, end to end, on the in-memory backend: the switcher
 * puts it first, set apart and named "My space", and says which workspace is
 * shown; the app opens on the shared workspace all the same. Entered, it
 * starts empty and says what it is; what is made there stays there, and
 * nothing of the shared workspace shows in it.
 *
 * Closed to the session once the account has a password, it asks for the
 * password and opens where it is when the password is given.
 *
 * What is hidden in the own space (people, invitations, the organization)
 * comes from the server alone, so it is tested against the server
 * (`e2e-live/own-space.spec.ts`).
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { ON_A_VIEW, placeOf, WORKSPACE } from './address.ts';
import {
  boardLink,
  FIRST_BOARD,
  hideSidebar,
  openApp,
  pressInBar,
  showSidebar,
  sidebar,
  topBar,
} from './shell.support.ts';
import { ON_A_WIKI } from './wiki.support.ts';

const switcher = (page: Page): Locator => sidebar(page).getByTestId('workspace-switcher');
const workspacesMenu = (page: Page): Locator =>
  page.getByRole('menu', { name: 'Switch workspace', exact: true });

/** Opens the switcher and goes to the workspace of that name. */
async function switchTo(page: Page, name: string): Promise<void> {
  await showSidebar(page);
  await switcher(page).click();
  await workspacesMenu(page).getByRole('menuitem', { name, exact: true }).click();
}

test('the switcher puts the own space first and says which is shown', async ({ page }) => {
  await openApp(page);
  // The app opens on the workspace the reader shares with others.
  expect(placeOf(page.url()).workspaceId).toBe(WORKSPACE);
  await showSidebar(page);
  await expect(switcher(page)).toHaveAccessibleName('Switch workspace: fvoci');
  await expect(switcher(page)).not.toHaveAttribute('data-own-space');

  await switcher(page).click();
  const items = workspacesMenu(page).getByRole('menuitem');
  await expect(items).toHaveText(['My space', 'fvoci']);
  await expect(items.nth(0)).toHaveAccessibleName('My space');
  await expect(items.nth(1)).toHaveAccessibleName('fvoci, current');
  // Its mark is not that of the others.
  const markOf = (item: Locator): Promise<string | null> =>
    item.locator('svg').first().getAttribute('class');
  expect(await markOf(items.nth(0))).not.toBe(await markOf(items.nth(1)));

  await items.nth(0).click();
  await expect(page).toHaveURL(/\/w\/[0-9a-f-]{36}$/u);
  expect(placeOf(page.url()).workspaceId).not.toBe(WORKSPACE);
  await showSidebar(page);
  await expect(switcher(page)).toHaveAccessibleName('Switch workspace: My space');
  await expect(switcher(page)).toHaveAttribute('data-own-space', '');
});

test('the own space starts empty, keeps what is made in it, and shows nothing of the shared one', async ({
  page,
}) => {
  await openApp(page);
  await switchTo(page, 'My space');
  const start = page.getByTestId('workspace-start');
  await expect(start.getByRole('heading', { name: 'My space' })).toBeVisible();
  await expect(start).toContainText('yours alone');
  await showSidebar(page);
  await expect(boardLink(page, FIRST_BOARD)).toHaveCount(0);
  await expect(sidebar(page).getByRole('link')).toHaveCount(0);

  // The start leads to making a board, as "Add board or wiki" does.
  const own = placeOf(page.url()).workspaceId;
  await page.keyboard.press('Escape');
  await start.getByTestId('workspace-start-add').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await expect(page).toHaveURL(ON_A_VIEW);
  expect(placeOf(page.url()).workspaceId).toBe(own);
  await expect(page.getByTestId('board-page')).toBeVisible();
  // Nobody else is there to find a wiki: it is not asked.
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await expect(picker.getByRole('radio')).toHaveCount(0);
  await picker.getByTestId('new-title').fill('Notes of mine');
  // Enter is done with the title: on to the kinds, the first of them.
  await picker.getByTestId('new-title').press('Enter');
  await expect(picker.getByTestId('empty-board')).toBeFocused();
  await expect(picker).toBeVisible();
  await picker.getByTestId('new-wiki').click();
  await expect(page).toHaveURL(ON_A_WIKI);
  await showSidebar(page);
  await expect(sidebar(page).getByRole('link')).toHaveCount(2);

  await switchTo(page, 'fvoci');
  await expect(page.getByTestId('board-page')).toBeVisible();
  expect(placeOf(page.url()).workspaceId).toBe(WORKSPACE);
  await showSidebar(page);
  await expect(boardLink(page, FIRST_BOARD)).toBeVisible();
  await expect(sidebar(page).getByText('Notes of mine')).toHaveCount(0);
  await expect(boardLink(page, '(Untitled Board)')).toHaveCount(0);

  // Back in the own space, it is as it was left.
  await switchTo(page, 'My space');
  await showSidebar(page);
  await expect(sidebar(page).getByRole('link')).toHaveCount(2);
  await expect(sidebar(page).getByText('Notes of mine')).toBeVisible();

  // Its diaries' entries are a kind to find, as nowhere else.
  await hideSidebar(page);
  await topBar(page).getByTestId('open-search').click();
  await expect(
    page.getByTestId('search-kinds').getByRole('radio', { name: 'Diary', exact: true }),
  ).toBeVisible();
  await page.keyboard.press('Escape');
});

test('a board of the own space is shared by its public link alone, with nobody to count', async ({
  page,
}) => {
  await openApp(page);
  // In the shared workspace the board has its members and the whole dialog.
  await expect(page.getByTestId('board-people')).toBeVisible();
  await pressInBar(page, 'share');
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-open')).toBeVisible();
  await expect(dialog.getByTestId('share-members')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  await switchTo(page, 'My space');
  await page.keyboard.press('Escape');
  await page.getByTestId('workspace-start-add').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page.getByTestId('board-people')).toHaveCount(0);
  await pressInBar(page, 'share');
  await expect(dialog.getByTestId('share-public-section')).toBeVisible();
  await expect(dialog.getByTestId('share-open')).toHaveCount(0);
  await expect(dialog.getByTestId('share-members')).toHaveCount(0);
  await expect(dialog.getByTestId('share-search')).toHaveCount(0);
  await dialog.getByTestId('share-public').click();
  await expect(dialog.getByTestId('share-link')).toHaveValue(/\/shared\/[0-9a-f-]{36}\?r=[^&]+$/u);
});

/** The world in memory, as the page lends it to the browser tests (`app/backend.ts`). */
interface Dev {
  backend: {
    userId: string;
    world: {
      passwords: Map<string, string>;
      ownSpaces: Map<string, { passwordRequired?: boolean }>;
    };
  };
}

test('the own space that takes the password opens where it is once the password is given', async ({
  page,
}) => {
  await openApp(page);
  // As the server closes it to a session made through a provider, once the account has a password.
  await page.evaluate(() => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    backend.world.passwords.set(backend.userId, 'a long password');
    const own = backend.world.ownSpaces.get(backend.userId);
    if (!own) throw new Error('no own space');
    own.passwordRequired = true;
    // Gone with the page, if it were loaded again.
    (window as unknown as { stayed?: boolean }).stayed = true;
  });
  await showSidebar(page);
  await switcher(page).click();
  await workspacesMenu(page).getByRole('menuitem', { name: 'My space', exact: true }).click();
  const locked = page.getByTestId('own-space-locked');
  await expect(locked).toBeVisible();
  await expect(locked).toContainText('You stay signed in as you are.');
  const address = page.url();
  // Where the list of boards is a drawer, it is over the space until it is put away.
  await hideSidebar(page);
  const field = page.getByTestId('own-space-locked-password');

  // A wrong password is said at the field, which takes the focus to try again.
  await field.fill('not it at all');
  await field.press('Enter');
  await expect(page.getByTestId('own-space-locked-problem')).toHaveText(
    'The current password is wrong.',
  );
  await expect(field).toBeFocused();

  await field.fill('a long password');
  await locked.getByRole('button', { name: 'Open My space' }).click();
  await expect(page.getByTestId('workspace-start')).toBeVisible();
  await expect(locked).toHaveCount(0);
  expect(page.url()).toBe(address);
  expect(await page.evaluate(() => (window as unknown as { stayed?: boolean }).stayed)).toBe(true);
  await showSidebar(page);
  await expect(switcher(page)).toHaveAccessibleName('Switch workspace: My space');
  // The workspace shared with others opens as before, and the own space after it.
  await switchTo(page, 'fvoci');
  await expect(page.getByTestId('own-space-locked')).toHaveCount(0);
  expect(placeOf(page.url()).workspaceId).toBe(WORKSPACE);
  await switchTo(page, 'My space');
  await expect(page.getByTestId('workspace-start')).toBeVisible();
});
