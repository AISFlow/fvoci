/**
 * The sidebar, end to end, on the in-memory backend: its three shapes and
 * what happens between them, going to a board, the drawer and the back
 * button, categories, moving boards (menu, mouse, finger, keyboard),
 * deleting and undo, the search, and "Add board".
 *
 * Every test runs at the three stages (the three projects) unless it says
 * that it is about one of them.
 */
import { expect, test } from '@playwright/test';

import { ON_A_BOARD, ON_A_VIEW } from './address.ts';
import {
  boardLink,
  boardMenu,
  boardsIn,
  categories,
  category,
  categoryMenu,
  categoryToggle,
  chooseFromMenu,
  Finger,
  FIRST_BOARD,
  hideSidebar,
  nextFrame,
  openApp,
  pane,
  pointOn,
  pressInBar,
  scrim,
  shell,
  showSidebar,
  sidebar,
  stageOf,
  strip,
  topBar,
} from './shell.support.ts';

const PROJECTS = ['Project schedule', 'Issue tracker', 'Meeting notes'];
const OPERATIONS = ['Content calendar', 'Customer contacts'];

test.describe('the shapes of the sidebar', () => {
  test('the list of boards is where the stage puts it', async ({ page }) => {
    await openApp(page);
    const stage = await stageOf(page);
    const board = await pane(page).boundingBox();

    if (stage === 'expanded') {
      await expect(shell(page)).toHaveAttribute('data-sidebar', 'fixed');
      await expect(sidebar(page)).toBeVisible();
      await expect(strip(page)).toBeHidden();
      await expect(page.getByTestId('open-sidebar')).toHaveCount(0);
      expect((await sidebar(page).boundingBox())?.width).toBe(256);
      expect(board?.x).toBe(256);
    } else if (stage === 'medium') {
      await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
      await expect(sidebar(page)).toBeHidden();
      await expect(strip(page)).toBeVisible();
      await expect(page.getByTestId('open-sidebar')).toHaveCount(0);
      expect((await strip(page).boundingBox())?.width).toBe(56);
      expect(board?.x).toBe(56);
      // One button a board, and the board on the page is marked.
      await expect(strip(page).getByRole('link')).toHaveCount(8);
      await expect(strip(page).getByRole('link', { name: FIRST_BOARD })).toHaveAttribute(
        'aria-current',
        'page',
      );
    } else {
      await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
      await expect(sidebar(page)).toBeHidden();
      await expect(strip(page)).toBeHidden();
      await expect(page.getByTestId('open-sidebar')).toBeVisible();
      expect(board?.x).toBe(0);
    }
  });

  test('the drawer lies over the page, takes the focus, and closes in every way it should', async ({
    page,
  }) => {
    await openApp(page);
    const stage = await stageOf(page);
    test.skip(stage === 'expanded', 'an expanded page has no drawer');
    const opener = page.getByTestId(stage === 'compact' ? 'open-sidebar' : 'expand-sidebar');
    const before = await pane(page).boundingBox();
    const width = page.viewportSize()?.width ?? 0;

    await opener.click();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'drawer');
    await expect(sidebar(page)).toHaveRole('dialog');
    await expect(sidebar(page)).toBeFocused();
    await expect(scrim(page)).toBeVisible();
    // Over the page: the board has not moved. Three quarters of the screen, 288px at most.
    expect(await pane(page).boundingBox()).toEqual(before);
    await expect
      .poll(async () => (await sidebar(page).boundingBox())?.x, { message: 'slid in' })
      .toBe(0);
    expect((await sidebar(page).boundingBox())?.width).toBe(Math.min(288, width * 0.75));
    // What is behind it cannot be reached.
    await expect(pane(page)).toHaveAttribute('inert');

    // A tap outside.
    await page.mouse.click(width - 20, 300);
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await expect(sidebar(page)).toBeHidden();
    await expect(pane(page)).not.toHaveAttribute('inert');
    await expect(opener).toBeFocused();

    // Escape.
    await opener.click();
    await expect(sidebar(page)).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');

    // Its own button.
    await opener.click();
    await sidebar(page).getByRole('button', { name: 'Close sidebar' }).click();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
  });

  test('the drawer still takes and returns the focus while a stylesheet is loading', async ({
    page,
  }) => {
    await openApp(page);
    const stage = await stageOf(page);
    test.skip(stage === 'expanded', 'an expanded page has no drawer');
    const opener = page.getByTestId(stage === 'compact' ? 'open-sidebar' : 'expand-sidebar');
    let release: (() => void) | undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    let requested = false;
    await page.route('**/fvoci-pending-sheet.css', async (route) => {
      requested = true;
      await held;
      await route.fulfill({ contentType: 'text/css', body: '' }).catch(() => undefined);
    });
    try {
      await page.evaluate(() => {
        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = '/fvoci-pending-sheet.css';
        document.head.append(link);
      });
      await expect.poll(() => requested).toBe(true);

      await opener.click();
      await expect(shell(page)).toHaveAttribute('data-sidebar', 'drawer');
      await expect(sidebar(page)).toBeFocused();

      await page.keyboard.press('Escape');
      await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
      await expect(opener).toBeFocused();
    } finally {
      release?.();
      await page.unroute('**/fvoci-pending-sheet.css').catch(() => undefined);
    }
  });

  test('the back button closes the drawer and leaves the address alone', async ({ page }) => {
    await openApp(page);
    test.skip((await stageOf(page)) === 'expanded', 'an expanded page has no drawer');
    const address = page.url();

    await showSidebar(page);
    await page.goBack();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    expect(page.url()).toBe(address);
    await expect(page.getByTestId('board-page')).toBeVisible();

    // Closed some other way, the drawer takes its step of the history back:
    // the next "back" is not swallowed by a drawer that is no longer there.
    await showSidebar(page);
    await hideSidebar(page);
    await showSidebar(page);
    await page.goBack();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    expect(page.url()).toBe(address);
  });

  test('a board chosen in the drawer: the drawer closes, and "back" returns to the board before', async ({
    page,
  }) => {
    await openApp(page);
    test.skip((await stageOf(page)) === 'expanded', 'an expanded page has no drawer');
    const first = page.url();

    await showSidebar(page);
    await boardLink(page, 'Issue tracker').click();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(page).not.toHaveURL(first);
    await expect(page).toHaveURL(ON_A_VIEW);
    const second = page.url();

    // The new address took the place of the drawer's step: one "back" is the board before.
    await page.goBack();
    await expect(page).toHaveURL(first);
    await expect(topBar(page).getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');

    await page.goForward();
    await expect(page).toHaveURL(second);
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
  });

  test('the strip goes to another board in one tap', async ({ page }) => {
    await openApp(page);
    test.skip((await stageOf(page)) !== 'medium', 'only a medium page starts with the strip');
    const first = page.url();

    await strip(page).getByRole('link', { name: 'Issue tracker' }).click();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(strip(page).getByRole('link', { name: 'Issue tracker' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await page.goBack();
    await expect(page).toHaveURL(first);
  });

  test('the fixed sidebar folds to the strip, the board gets the room, and the device remembers', async ({
    page,
  }) => {
    await openApp(page);
    test.skip((await stageOf(page)) !== 'expanded', 'only an expanded page has the fixed sidebar');
    const wide = (await pane(page).boundingBox())?.width ?? 0;

    await sidebar(page).getByRole('button', { name: 'Collapse sidebar' }).click();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await expect(strip(page)).toBeVisible();
    await expect(sidebar(page)).toBeHidden();
    // 256 less 56: the board was pushed, not covered.
    await expect.poll(async () => (await pane(page).boundingBox())?.width).toBe(wide + 200);
    await expect(page.getByTestId('expand-sidebar')).toBeFocused();

    await page.reload();
    await expect(page.getByTestId('board-page')).toBeVisible();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await expect(strip(page)).toBeVisible();

    // Unfolding puts it back beside the board: no drawer, nothing over the page.
    await page.getByTestId('expand-sidebar').click();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'fixed');
    await expect(scrim(page)).toHaveCount(0);
    await expect.poll(async () => (await pane(page).boundingBox())?.width).toBe(wide);
    await page.reload();
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'fixed');
  });

  test('a change of size keeps the list: an open drawer stays out until the sidebar is fixed, and the scroll position stays', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 500, height: 560 });
    await openApp(page);
    expect(await stageOf(page)).toBe('compact');
    const list = sidebar(page).getByTestId('sidebar-list');
    const scrollTop = (): Promise<number> => list.evaluate((element) => element.scrollTop);

    await showSidebar(page);
    await list.evaluate((element) => {
      element.scrollTop = 120;
    });
    expect(await scrollTop()).toBe(120);

    // Compact to medium: the drawer is still the drawer.
    await page.setViewportSize({ width: 700, height: 560 });
    await expect(shell(page)).toHaveAttribute('data-stage', 'medium');
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'drawer');
    await expect(strip(page)).toBeVisible();
    expect(await scrollTop()).toBe(120);

    // Medium to expanded: the list is on the page, and nothing covers the board.
    await page.setViewportSize({ width: 1000, height: 560 });
    await expect(shell(page)).toHaveAttribute('data-stage', 'expanded');
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'fixed');
    await expect(scrim(page)).toHaveCount(0);
    await expect(pane(page)).not.toHaveAttribute('inert');
    await expect(sidebar(page)).toBeVisible();
    expect(await scrollTop()).toBe(120);

    // Back to compact: the drawer does not come out by itself.
    await page.setViewportSize({ width: 500, height: 560 });
    await expect(shell(page)).toHaveAttribute('data-stage', 'compact');
    await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
    await expect(sidebar(page)).toBeHidden();
    await expect(page.getByTestId('board-page')).toBeVisible();

    // Opened again, it is where it was left.
    await showSidebar(page);
    expect(await scrollTop()).toBe(120);

    // A page too low for the wide layouts is compact whatever its width.
    await hideSidebar(page);
    await page.setViewportSize({ width: 1000, height: 420 });
    await expect(shell(page)).toHaveAttribute('data-stage', 'compact');
    await expect(page.getByTestId('open-sidebar')).toBeVisible();
  });
});

test.describe('boards in the sidebar', () => {
  test('choosing a board shows it, marks it, and puts it in the address', async ({ page }) => {
    await openApp(page);
    const first = page.url();
    await showSidebar(page);
    await expect(boardLink(page, FIRST_BOARD)).toHaveAttribute('aria-current', 'page');
    // The links are real addresses: a new tab can be opened from them.
    await expect(boardLink(page, 'Issue tracker')).toHaveAttribute('href', ON_A_BOARD);

    await boardLink(page, 'Issue tracker').click();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(page).not.toHaveURL(first);
    await expect(page.getByTestId('board-page')).toBeVisible();

    await showSidebar(page);
    await expect(boardLink(page, 'Issue tracker')).toHaveAttribute('aria-current', 'page');
    await expect(boardLink(page, FIRST_BOARD)).not.toHaveAttribute('aria-current');
  });

  test('the search narrows the boards down by title', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    const search = sidebar(page).getByTestId('sidebar-search');
    await expect(sidebar(page).getByRole('link')).toHaveCount(8);

    await search.fill('NOTES');
    await expect(sidebar(page).getByRole('link')).toHaveCount(2);
    await expect(boardLink(page, 'Meeting notes')).toBeVisible();
    await expect(boardLink(page, 'Study notes')).toBeVisible();
    expect(await categories(page)).toEqual(['Projects', 'Personal']);

    await search.fill('no such board');
    await expect(sidebar(page).getByRole('link')).toHaveCount(0);
    await expect(sidebar(page).getByText('No board or wiki has that in its title.')).toBeVisible();

    // The first Escape empties the field and leaves the drawer, if it is one, open.
    await search.press('Escape');
    await expect(search).toHaveValue('');
    await expect(sidebar(page).getByRole('link')).toHaveCount(8);
    await expect(shell(page)).not.toHaveAttribute('data-sidebar', 'closed');
  });

  test('a board moves to another category from its menu', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    await boardMenu(page, 'Issue tracker').click();
    await page.getByRole('menuitem', { name: 'Move to Operations' }).click();
    await expect.poll(() => boardsIn(page, 'Operations')).toEqual([...OPERATIONS, 'Issue tracker']);
    expect(await boardsIn(page, 'Projects')).toEqual(
      PROJECTS.filter((title) => title !== 'Issue tracker'),
    );
  });

  test('a board is dragged with the mouse: to another place, into another category, onto a folded one', async ({
    page,
    hasTouch,
  }) => {
    test.skip(hasTouch, 'the mouse belongs to the desktop; a finger has a test of its own');
    await openApp(page);
    await showSidebar(page);
    const address = page.url();
    const overlay = page.locator('[data-drag-overlay]');

    const drag = async (title: string, onto: string, down: number): Promise<void> => {
      const from = await pointOn(boardLink(page, title));
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(from.x + 6, from.y + 6, { steps: 3 });
      await expect(overlay).toBeVisible();
      // Twice: the gap that opens moves what is under the pointer.
      for (let pass = 0; pass < 2; pass += 1) {
        const target =
          onto === ''
            ? await pointOn(categoryToggle(page, 'Operations'))
            : await pointOn(boardLink(page, onto), down);
        await page.mouse.move(target.x, target.y, { steps: 8 });
        await nextFrame(page);
      }
      await page.mouse.up();
      await expect(overlay).toHaveCount(0);
    };

    // Within its category: under "Meeting notes".
    await drag('Project schedule', 'Meeting notes', 0.8);
    await expect
      .poll(() => boardsIn(page, 'Projects'))
      .toEqual(['Issue tracker', 'Meeting notes', 'Project schedule']);
    // The release that ended the drag was not a click on a link.
    expect(page.url()).toBe(address);

    // Into another category: above "Customer contacts".
    await drag('Meeting notes', 'Customer contacts', 0.2);
    await expect
      .poll(() => boardsIn(page, 'Operations'))
      .toEqual(['Content calendar', 'Meeting notes', 'Customer contacts']);

    // Onto the heading of a folded category: it is filed last there.
    await categoryToggle(page, 'Operations').click();
    await expect(categoryToggle(page, 'Operations')).toHaveAttribute('aria-expanded', 'false');
    await drag('Project schedule', '', 0.5);
    await categoryToggle(page, 'Operations').click();
    await expect
      .poll(() => boardsIn(page, 'Operations'))
      .toEqual(['Content calendar', 'Meeting notes', 'Customer contacts', 'Project schedule']);

    // Let go outside the list: nothing moves.
    const from = await pointOn(boardLink(page, 'Issue tracker'));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 6, from.y + 6, { steps: 3 });
    await expect(overlay).toBeVisible();
    await page.mouse.move(700, 300, { steps: 8 });
    await nextFrame(page);
    await expect(sidebar(page).getByText('Let go to cancel.')).toBeVisible();
    await page.mouse.up();
    await expect(overlay).toHaveCount(0);
    expect(await boardsIn(page, 'Projects')).toEqual(['Issue tracker']);
  });

  test('a finger that rests on a board picks it up; a finger that moves at once scrolls', async ({
    page,
    hasTouch,
  }) => {
    test.skip(!hasTouch, 'needs a touch screen');
    await openApp(page);
    await showSidebar(page);
    const overlay = page.locator('[data-drag-overlay]');
    const finger = await Finger.on(page);

    await finger.down(await pointOn(boardLink(page, 'Project schedule')));
    await expect(overlay).toBeVisible();
    for (let pass = 0; pass < 2; pass += 1) {
      await finger.move(await pointOn(boardLink(page, 'Meeting notes'), 0.8));
    }
    await finger.up();
    await expect(overlay).toHaveCount(0);
    await expect
      .poll(() => boardsIn(page, 'Projects'))
      .toEqual(['Issue tracker', 'Meeting notes', 'Project schedule']);
    // The lift of the finger opened nothing, and the list is still there.
    await expect(topBar(page).getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    await expect(shell(page)).not.toHaveAttribute('data-sidebar', 'closed');

    // A finger that moves before it has rested is scrolling: nothing is picked up.
    const start = await pointOn(boardLink(page, 'Meeting notes'));
    await finger.down(start);
    await finger.move({ x: start.x, y: start.y - 60 }, 4);
    await expect(overlay).toHaveCount(0);
    await finger.up();
    expect(await boardsIn(page, 'Projects')).toHaveLength(3);
  });

  test('a board is moved with the keyboard, and every step is announced', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    const announcer = sidebar(page).getByTestId('sidebar-announcer');

    await boardLink(page, 'Project schedule').focus();
    await page.keyboard.press('Space');
    await expect(announcer).toHaveText('Picked up Project schedule, position 1 of 3 in Projects.');
    await page.keyboard.press('ArrowDown');
    await expect(announcer).toHaveText('Project schedule, position 2 of 3 in Projects.');
    await page.keyboard.press('Space');
    await expect(announcer).toHaveText('Dropped Project schedule, position 2 of 3 in Projects.');
    await expect
      .poll(() => boardsIn(page, 'Projects'))
      .toEqual(['Issue tracker', 'Project schedule', 'Meeting notes']);
    await expect(boardLink(page, 'Project schedule')).toBeFocused();

    // Into the next category.
    await page.keyboard.press('Space');
    await page.keyboard.press('ArrowRight');
    await expect(announcer).toContainText('in Operations.');
    await page.keyboard.press('Enter');
    await expect.poll(() => boardsIn(page, 'Operations')).toContain('Project schedule');
    expect(await boardsIn(page, 'Projects')).not.toContain('Project schedule');

    // Escape puts the board back, and is the drag's alone: a drawer stays open.
    await boardLink(page, 'Issue tracker').focus();
    await page.keyboard.press('Space');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Escape');
    await expect(announcer).toContainText(
      'Cancelled. Issue tracker is back at position 1 of 2 in Projects.',
    );
    expect(await boardsIn(page, 'Projects')).toEqual(['Issue tracker', 'Meeting notes']);
    await expect(shell(page)).not.toHaveAttribute('data-sidebar', 'closed');
  });

  test('the "…" of a board waits for the pointer where there is one, and is always there where there is none', async ({
    page,
  }) => {
    await openApp(page);
    await showSidebar(page);
    const menu = boardMenu(page, 'Issue tracker');
    const canHover = await page.evaluate(() => matchMedia('(hover: hover)').matches);
    if (canHover) {
      await sidebar(page).getByTestId('sidebar-search').hover();
      await expect(menu).toHaveCSS('opacity', '0');
      await boardLink(page, 'Issue tracker').hover();
      await expect(menu).toHaveCSS('opacity', '1');
      // And for the keyboard: it shows when the row holds the focus.
      await sidebar(page).getByTestId('sidebar-search').hover();
      await boardLink(page, 'Issue tracker').focus();
      await expect(menu).toHaveCSS('opacity', '1');
    } else {
      await expect(menu).toHaveCSS('opacity', '1');
    }
  });

  test('a deleted board is gone, and undo brings it back', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    await chooseFromMenu(page, boardMenu(page, 'Meeting notes'), 'delete');
    const confirm = page.getByTestId('confirm-dialog');
    await expect(confirm).toContainText('Meeting notes');
    // Asked first: nothing is gone yet, and "Cancel" keeps it.
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(boardLink(page, 'Meeting notes')).toBeVisible();

    await chooseFromMenu(page, boardMenu(page, 'Meeting notes'), 'delete');
    await confirm.getByRole('button', { name: 'Delete' }).click();
    await expect(boardLink(page, 'Meeting notes')).toHaveCount(0);
    await expect(sidebar(page).getByRole('link')).toHaveCount(7);

    await hideSidebar(page);
    await pressInBar(page, 'undo');
    await showSidebar(page);
    await expect(boardLink(page, 'Meeting notes')).toBeVisible();
    await expect(sidebar(page).getByRole('link')).toHaveCount(8);
  });

  test('deleting the board on the page goes to the first board that is left', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    await chooseFromMenu(page, boardMenu(page, FIRST_BOARD), 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(page.getByTestId('board-page')).toBeVisible();
    await showSidebar(page);
    await expect(boardLink(page, FIRST_BOARD)).toHaveCount(0);
    await expect(boardLink(page, 'Issue tracker')).toHaveAttribute('aria-current', 'page');
  });

  test('"Add board" makes an empty board with a view and opens it', async ({ page }) => {
    await openApp(page);
    const first = page.url();
    await showSidebar(page);
    await sidebar(page).getByTestId('add-board').click();
    // It asks first: an empty board, or one from a template.
    await page.getByTestId('template-picker').getByTestId('empty-board').click();

    await expect(page).not.toHaveURL(first);
    await expect(page).toHaveURL(ON_A_VIEW);
    await expect(page.getByTestId('board-page')).toBeVisible();
    const title = topBar(page).getByTestId('board-title');
    await expect(title).toHaveValue('');
    await expect(title).toHaveAttribute('placeholder', 'Untitled board');

    await showSidebar(page);
    await expect(sidebar(page).getByRole('link')).toHaveCount(9);
    await expect(boardLink(page, '(Untitled Board)')).toHaveAttribute('aria-current', 'page');
    // A board nobody filed is shown in the default category.
    expect(await boardsIn(page, 'General')).toContain('(Untitled Board)');
  });
});

test.describe('categories', () => {
  test('a category folds and unfolds', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    const toggle = categoryToggle(page, 'Operations');
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(boardLink(page, 'Content calendar')).toBeVisible();

    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(boardLink(page, 'Content calendar')).toHaveCount(0);
    await expect(sidebar(page).getByRole('link')).toHaveCount(6);
    // The search still finds a board of a folded category.
    await sidebar(page).getByTestId('sidebar-search').fill('calendar');
    await expect(boardLink(page, 'Content calendar')).toBeVisible();
    await sidebar(page).getByTestId('sidebar-search').fill('');

    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(boardLink(page, 'Content calendar')).toBeVisible();
  });

  test('a category is made, renamed, moved, filled and deleted', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    expect(await categories(page)).toEqual(['Projects', 'Operations', 'Personal', 'General']);
    const dialog = page.getByTestId('category-dialog');
    const name = dialog.getByRole('textbox', { name: 'Category name' });

    // Made: it needs a name, and goes first.
    await sidebar(page).getByTestId('new-category').click();
    await expect(dialog.getByRole('button', { name: 'Create' })).toBeDisabled();
    await name.fill('Ideas');
    await name.press('Enter');
    await expect(dialog).toBeHidden();
    await expect
      .poll(() => categories(page))
      .toEqual(['Ideas', 'Projects', 'Operations', 'Personal', 'General']);
    await expect(category(page, 'Ideas').getByText('No boards or wikis inside')).toBeVisible();

    // Renamed: the field starts with the name as it is.
    await chooseFromMenu(page, categoryMenu(page, 'Ideas'), 'rename');
    await expect(name).toHaveValue('Ideas');
    await name.fill('Later');
    await dialog.getByRole('button', { name: 'Update' }).click();
    await expect
      .poll(() => categories(page))
      .toEqual(['Later', 'Projects', 'Operations', 'Personal', 'General']);

    // Moved down and up again; the first one cannot go further up.
    await chooseFromMenu(page, categoryMenu(page, 'Later'), 'move-down');
    await expect
      .poll(() => categories(page))
      .toEqual(['Projects', 'Later', 'Operations', 'Personal', 'General']);
    await chooseFromMenu(page, categoryMenu(page, 'Later'), 'move-up');
    await expect
      .poll(() => categories(page))
      .toEqual(['Later', 'Projects', 'Operations', 'Personal', 'General']);
    await categoryMenu(page, 'Later').click();
    await expect(page.locator('[data-action="move-up"]')).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-action="move-up"]')).toHaveCount(0);

    // Filled from the menu of a board.
    await boardMenu(page, 'Issue tracker').click();
    await page.getByRole('menuitem', { name: 'Move to Later' }).click();
    await expect.poll(() => boardsIn(page, 'Later')).toEqual(['Issue tracker']);

    // Deleted, after a question: its board goes back to the default category.
    await chooseFromMenu(page, categoryMenu(page, 'Later'), 'delete');
    const confirm = page.getByTestId('confirm-dialog');
    await expect(confirm).toContainText('Delete this category?');
    await expect(confirm).toContainText('Later');
    await confirm.getByRole('button', { name: 'Delete' }).click();
    await expect
      .poll(() => categories(page))
      .toEqual(['Projects', 'Operations', 'Personal', 'General']);
    await expect.poll(() => boardsIn(page, 'General')).toContain('Issue tracker');
  });

  test('the default category cannot be deleted or renamed', async ({ page }) => {
    await openApp(page);
    await showSidebar(page);
    await categoryMenu(page, 'General').click();
    await expect(page.locator('[data-action="move-up"]')).toBeVisible();
    await expect(page.locator('[data-action="delete"]')).toHaveCount(0);
    // The server keeps its name whatever is sent.
    await expect(page.locator('[data-action="rename"]')).toHaveCount(0);
    await page.keyboard.press('Escape');

    await categoryMenu(page, 'Projects').click();
    await expect(page.locator('[data-action="delete"]')).toBeVisible();
    await expect(page.locator('[data-action="rename"]')).toBeVisible();
  });
});
