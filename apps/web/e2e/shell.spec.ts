/**
 * The frame of the app, end to end: the files it is made of, and on the
 * in-memory backend the top bar (title, members, undo, a board that can only
 * be read), the layout readout, and the edges the device keeps. The sidebar
 * and the settings have files of their own.
 */
import path from 'node:path';

import { expect, test, type Page } from '@playwright/test';

import { primaryKey, primaryName } from './keyboard.support.ts';
import { boardPath } from './address.ts';
import {
  boardLink,
  boardMenu,
  FIRST_BOARD,
  hideSidebar,
  openApp,
  pane,
  pressInBar,
  READ_ONLY_BOARD,
  shell,
  showSidebar,
  sidebar,
  stageOf,
  strip,
  topBar,
  undoInBar,
} from './shell.support.ts';
import { screensDir } from './screens.ts';

test('the app starts and requests only hashed, compressed files', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', (message) => {
    // WebKit, where it reads the viewport tag (iOS, and Linux's WebKit always), calls a key it
    // does not know an error: `interactive-widget` is for Chromium's on-screen keyboard.
    if (message.text().startsWith('Viewport argument key "interactive-widget" not recognized'))
      return;
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));

  const responses: {
    url: string;
    status: number;
    cache: string;
    encoding: string;
    accepted: Promise<string | null>;
  }[] = [];
  page.on('response', (response) => {
    const headers = response.headers();
    responses.push({
      url: new URL(response.url()).pathname,
      status: response.status(),
      cache: headers['cache-control'] ?? '',
      encoding: headers['content-encoding'] ?? '',
      accepted: response.request().headerValue('accept-encoding'),
    });
  });

  await page.goto('/');
  await expect(page.getByTestId('app-shell')).toBeVisible();
  // Fonts are requested while the page settles; wait for them.
  await page.waitForLoadState('networkidle');

  expect(errors).toEqual([]);
  const assets = responses.filter((response) => response.url.includes('/static/'));
  expect(assets.length).toBeGreaterThanOrEqual(2);
  for (const asset of assets) {
    expect(asset.status, asset.url).toBe(200);
    expect(asset.cache, asset.url).toContain('immutable');
    if (/\.(js|css)$/.test(asset.url)) {
      const offered = (await asset.accepted)?.split(',').map((coding) => coding.trim()) ?? [];
      const preferred = ['br', 'zstd', 'gzip'].find((coding) => offered.includes(coding));
      expect(preferred, `a compressed representation is offered for ${asset.url}`).toBeDefined();
      expect(asset.encoding, asset.url).toBe(preferred);
    }
  }
});

test('the page can be zoomed and follows the text size of the browser', async ({ page }) => {
  await page.goto('/');
  const viewport = await page.locator('meta[name="viewport"]').getAttribute('content');
  expect(viewport).not.toContain('user-scalable');
  expect(viewport).not.toContain('maximum-scale');

  await expect(page.getByTestId('app-shell')).toBeVisible();
  const size = (): Promise<number> =>
    page.locator('body').evaluate((element) => parseFloat(getComputedStyle(element).fontSize));
  const before = await size();
  await page.evaluate(() => {
    document.documentElement.style.fontSize = '200%';
  });
  expect(await size()).toBeCloseTo(before * 2, 1);
});

test('a deep link gets the app, and its files resolve from the base path', async ({ page }) => {
  await page.goto(boardPath('some-board', 'some-view'));
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(page.getByRole('alert')).toHaveText('There is nothing at this address.');
});

test('an address that is not one of the app says so', async ({ page }) => {
  await page.goto('/nowhere/at/all');
  await expect(page.getByRole('alert')).toHaveText('There is nothing at this address.');
});

test.describe('the top bar', () => {
  test('shows the board, and of the rest what the pane has room for', async ({ page }) => {
    await openApp(page);
    await expect(topBar(page).getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    await expect(topBar(page).getByRole('heading', { level: 1 })).toBeVisible();

    // The page of the board has all that is left under the bar, down to the bottom edge.
    const bar = await topBar(page).boundingBox();
    const board = await page.getByTestId('board-page').boundingBox();
    expect(board?.y).toBe((bar?.y ?? 0) + (bar?.height ?? 0));
    expect((board?.y ?? 0) + (board?.height ?? 0)).toBe(page.viewportSize()?.height);
    expect(board?.width).toBe(bar?.width);

    // Presence is said on every width; with nobody else here there is no button of
    // faces (no "0"), at any width. The description is behind "ⓘ", not in the bar.
    const width = (await pane(page).boundingBox())?.width ?? 0;
    await expect(topBar(page).getByTestId('board-people')).toHaveAccessibleName(
      /^People viewing this board: 0, members: \d+$/,
    );
    await expect(topBar(page).getByTestId('board-presence')).toBeHidden();
    await expect(topBar(page).getByText('The periods and the order of your tasks')).toBeHidden();
    const about = topBar(page).getByTestId('container-description');
    if (width >= 544) await expect(about).toBeVisible();
    else await expect(about).toBeHidden();
  });

  test('the description is behind "ⓘ" next to the title, and its editors change it there', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page);
    const about = topBar(page).getByTestId('container-description');
    await expect(about).toHaveAccessibleName('Description');
    await about.click();
    const field = page.getByTestId('container-description-field');
    await expect(field).toHaveValue(/^The periods and the order of your tasks/);
    await field.fill('Where the launch stands.');
    await page.keyboard.press('Escape');
    await expect(field).toHaveCount(0);
    // Escape kept nothing of what was typed.
    await about.click();
    await expect(field).toHaveValue(/^The periods and the order of your tasks/);
    await field.fill('Where the launch stands.');
    // Leaving it by a click elsewhere keeps it.
    await topBar(page).getByTestId('board-title').click();
    await expect(field).toHaveCount(0);
    await about.click();
    await expect(field).toHaveValue('Where the launch stands.');
    await page.keyboard.press('Escape');

    // Emptied, it is still there for its editors, to add one again.
    await about.click();
    await field.fill('');
    await topBar(page).getByTestId('board-title').click();
    await expect(about).toHaveAccessibleName('Add description');
    await about.click();
    await expect(field).toHaveValue('');
    await page.keyboard.press('Escape');

    // A board the reader may only read shows the description, and no field.
    await showSidebar(page);
    await boardLink(page, READ_ONLY_BOARD).click();
    await expect(topBar(page).getByTestId('read-only')).toBeVisible();
    await about.click();
    await expect(page.getByTestId('container-description-text')).toHaveText(
      /^One card for each thing you learn/,
    );
    await expect(page.getByTestId('container-description-field')).toHaveCount(0);
  });

  test('the description is opened, written, saved and given up from the keyboard alone', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard and a mouse');
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page);
    const about = topBar(page).getByTestId('container-description');
    const field = page.getByTestId('container-description-field');
    const done = page.getByRole('button', { name: 'Done', exact: true });
    const openByKeys = async (): Promise<void> => {
      await about.focus();
      await page.keyboard.press('Enter');
      await expect(field).toBeFocused();
    };

    // Ctrl+Enter (⌘Enter on Apple devices) saves and closes; the focus is back on "ⓘ".
    await openByKeys();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('Saved by the keys.');
    await page.keyboard.press(await primaryKey(page, 'Enter'));
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();
    await openByKeys();
    await expect(field).toHaveValue('Saved by the keys.');

    // Tab reaches "Done", which saves and closes too.
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('Saved by Done.');
    await page.keyboard.press('Tab');
    await expect(done).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();

    // Escape gives up what was typed, from the field and from "Done".
    await openByKeys();
    await expect(field).toHaveValue('Saved by Done.');
    await page.keyboard.type(' Not kept.');
    await page.keyboard.press('Tab');
    await expect(done).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();
    await openByKeys();
    await expect(field).toHaveValue('Saved by Done.');
    await page.keyboard.type(' And more.');
    await page.keyboard.press('Escape');
    await expect(field).toHaveCount(0);
    await expect(about).toBeFocused();
    await openByKeys();
    await expect(field).toHaveValue('Saved by Done.');
  });

  test('a description being written is saved to its own board when the reader goes back, and does not open again', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'a keyboard and a mouse');
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page);
    await showSidebar(page);
    await boardLink(page, 'Issue tracker').click();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await boardLink(page, FIRST_BOARD).click();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    const first = new URL(page.url()).pathname;

    const about = topBar(page).getByTestId('container-description');
    const field = page.getByTestId('container-description-field');
    await about.click();
    await field.fill('Written, then left.');
    // Back to the other board with no click outside: nothing closed the description first.
    await page.goBack();
    await expect(topBar(page).getByTestId('board-title')).toHaveValue('Issue tracker');
    await expect(field).toHaveCount(0);
    const descriptions = (): Promise<Record<string, string>> =>
      page.evaluate(async () => {
        const { backend } = (
          window as unknown as {
            fvociDev: {
              backend: {
                workspaces: () => Promise<{ id: string }[]>;
                boards: (id: string) => Promise<{ title: string; description: string }[]>;
              };
            };
          }
        ).fvociDev;
        const [workspace] = await backend.workspaces();
        const boards = workspace ? await backend.boards(workspace.id) : [];
        return Object.fromEntries(boards.map((board) => [board.title, board.description]));
      });
    await expect.poll(async () => (await descriptions())[FIRST_BOARD]).toBe('Written, then left.');
    expect((await descriptions())['Issue tracker']).not.toBe('Written, then left.');
    // The bar still follows the board it shows: emptied here, its "ⓘ" offers to add one.
    await expect(about).toHaveAccessibleName('Description');
    await about.click();
    await field.fill('');
    await page.keyboard.press(await primaryKey(page, 'Enter'));
    await expect(about).toHaveAccessibleName('Add description');

    // Forward again: the description it was open on stays closed, and keeps no focus.
    await page.goForward();
    await expect(page).toHaveURL((url) => url.pathname === first);
    await expect(topBar(page).getByTestId('board-title')).toHaveValue(FIRST_BOARD);
    await expect(about).toBeVisible();
    await expect(field).toHaveCount(0);
    await expect(about).not.toBeFocused();
    await about.click();
    await expect(field).toHaveValue('Written, then left.');
  });

  test('the title of a board is changed in place', async ({ page }) => {
    await openApp(page);
    const title = topBar(page).getByTestId('board-title');

    await title.fill('Launch plan');
    await title.press('Enter');
    await expect(title).not.toBeFocused();
    await expect(title).toHaveValue('Launch plan');
    await showSidebar(page);
    await expect(boardLink(page, 'Launch plan')).toHaveAttribute('aria-current', 'page');
    await hideSidebar(page);

    // Escape gives the typing up.
    await title.fill('Something else');
    await title.press('Escape');
    await expect(title).toHaveValue('Launch plan');

    // Leaving the field saves, without the spaces around the text.
    await title.fill('  Launch  ');
    await title.blur();
    await expect(title).toHaveValue('Launch');
    await showSidebar(page);
    await expect(boardLink(page, 'Launch')).toBeVisible();
    await expect(boardLink(page, 'Launch plan')).toHaveCount(0);
  });

  test('undo and redo: the buttons, the keys, and a field keeps its own', async ({ page }) => {
    await openApp(page);
    const title = topBar(page).getByTestId('board-title');
    const undo = topBar(page).getByTestId('undo');
    const redo = topBar(page).getByTestId('redo');
    await expect(undo).toBeDisabled();
    await expect(redo).toBeDisabled();

    await title.fill('One');
    await title.press('Enter');
    await expect(undo).toBeEnabled();

    await pressInBar(page, 'undo');
    await expect(title).toHaveValue(FIRST_BOARD);
    await expect(redo).toBeEnabled();
    await pressInBar(page, 'redo');
    await expect(title).toHaveValue('One');

    // The keys, pressed outside of any field.
    await undoInBar(page).focus();
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect(title).toHaveValue(FIRST_BOARD);
    await page.keyboard.press(await primaryKey(page, 'Shift+z'));
    await expect(title).toHaveValue('One');
    // The other platform's modifier never changes the board: each is pressed before the
    // right key, which then takes the one step it would alone.
    const modifier = await primaryName(page);
    const other = modifier === 'Meta' ? 'Control' : 'Meta';
    await title.fill('Two');
    await title.press('Enter');
    await undoInBar(page).focus();
    await page.keyboard.press(`${other}+z`);
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect(title).toHaveValue('One');
    await expect(redo).toBeEnabled();
    await page.keyboard.press(`${other}+Shift+z`);
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect(title).toHaveValue(FIRST_BOARD);
    await page.keyboard.press(await primaryKey(page, 'Shift+z'));
    await expect(title).toHaveValue('One');
    await page.keyboard.press(await primaryKey(page, 'Shift+z'));
    await expect(title).toHaveValue('Two');
    await expect(redo).toBeDisabled();
    await expect(undo).toHaveAttribute(
      'title',
      modifier === 'Meta' ? 'Undo (⌘Z)' : 'Undo (Ctrl+Z)',
    );
    await expect(redo).toHaveAttribute(
      'title',
      modifier === 'Meta' ? 'Redo (⇧⌘Z)' : 'Redo (Ctrl+Shift+Z)',
    );
    await expect(undo).toHaveAttribute('aria-keyshortcuts', `${modifier}+Z`);

    // In a field they are the field's: what was typed is undone, not the board.
    await title.focus();
    await page.keyboard.press('End');
    await page.keyboard.type('x');
    await expect(title).toHaveValue('Twox');
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect(redo).toBeDisabled();
    await title.press('Escape');
    await expect(title).toHaveValue('Two');
  });

  test('a board that can only be read says so and offers no changes', async ({ page }) => {
    await openApp(page);
    await expect(topBar(page).getByTestId('read-only')).toHaveCount(0);

    await showSidebar(page);
    await boardLink(page, READ_ONLY_BOARD).click();
    await expect(topBar(page).getByTestId('read-only')).toHaveText('Read only');
    await expect(topBar(page).getByTestId('board-title-text')).toHaveText(READ_ONLY_BOARD);
    // The title is text, not a field.
    await expect(topBar(page).getByTestId('board-title')).toHaveCount(0);
    await expect(topBar(page).getByRole('textbox')).toHaveCount(0);

    // Its menu still files it under another category (the sidebar is the
    // reader's own), and does not offer to delete it.
    await showSidebar(page);
    await boardMenu(page, READ_ONLY_BOARD).click();
    await expect(page.getByRole('menuitem', { name: 'Move to Projects' })).toBeEnabled();
    await expect(page.locator('[data-action="delete"]')).toBeDisabled();
  });
});

test.describe('the layout readout', () => {
  test('is off unless asked for; the address turns it on and off, and the device remembers', async ({
    page,
  }) => {
    const readout = page.getByTestId('dev-readout');
    await openApp(page);
    await expect(readout).toHaveCount(0);

    await openApp(page, '/?dev=1');
    await expect(readout).toBeVisible();
    const viewport = page.viewportSize();
    await expect(readout).toHaveAttribute('data-stage', await stageOf(page));
    await expect(readout).toHaveAttribute('data-window', String(viewport?.width));
    await expect(readout).toHaveAttribute('data-page', String(viewport?.width));
    const width = Math.round((await pane(page).boundingBox())?.width ?? 0);
    await expect(readout).toHaveAttribute('data-pane', String(width));
    await expect(readout).toContainText(`window ${viewport?.width}×${viewport?.height}`);
    await expect(readout).toContainText(`pane ${width}`);

    // It follows the page as it changes.
    await page.setViewportSize({ width: 700, height: 600 });
    await expect(readout).toHaveAttribute('data-stage', 'medium');
    await expect(readout).toHaveAttribute('data-pane', '644');
    await expect(readout).toContainText('page 700 · medium');

    await openApp(page);
    await expect(readout).toBeVisible();
    await openApp(page, '/?dev=0');
    await expect(readout).toHaveCount(0);
    await openApp(page);
    await expect(readout).toHaveCount(0);
  });

  test('takes no presses: what is under it works as if it were not there', async ({ page }) => {
    await openApp(page, '/?dev=1');
    const readout = page.getByTestId('dev-readout');
    await expect(readout).toHaveCSS('pointer-events', 'none');
    const box = await readout.boundingBox();
    if (!box) throw new Error('the readout is not on the page');
    const under = await page.evaluate(
      ({ x, y }) =>
        document.elementFromPoint(x, y)?.closest('[data-testid]')?.getAttribute('data-testid'),
      { x: box.x + box.width / 2, y: box.y + box.height / 2 },
    );
    expect(under).not.toBe('dev-readout');
  });
});

test.describe('the edges the device keeps', () => {
  const keep = (page: Page, edges: Record<string, string>): Promise<void> =>
    page.evaluate((values) => {
      for (const [name, value] of Object.entries(values)) {
        document.documentElement.style.setProperty(`--safe-${name}`, value);
      }
    }, edges);

  test('left and right are cleared each by its own value, and the stage counts what is left', async ({
    page,
  }) => {
    // 660 wide is a medium window; with 84px kept at the right the page is 576 and compact.
    await page.setViewportSize({ width: 660, height: 700 });
    await openApp(page);
    await expect(shell(page)).toHaveAttribute('data-stage', 'medium');

    await keep(page, { right: '84px' });
    await expect(shell(page)).toHaveAttribute('data-stage', 'compact');
    const frame = await shell(page).boundingBox();
    expect(frame?.x).toBe(0);
    expect(frame?.width).toBe(576);
    // Nothing of the top bar reaches into the kept edge, and the left is not pushed.
    const bar = await topBar(page).boundingBox();
    expect((bar?.x ?? 0) + (bar?.width ?? 0)).toBeLessThanOrEqual(576);
    expect((await page.getByTestId('open-sidebar').boundingBox())?.x).toBe(12);

    // Kept at the left, the top and the bottom instead: the page starts after them.
    await keep(page, { right: '0px', left: '30px', top: '24px', bottom: '34px' });
    await expect(shell(page)).toHaveAttribute('data-stage', 'medium');
    expect(await shell(page).boundingBox()).toEqual({ x: 30, y: 24, width: 630, height: 676 });
    expect((await strip(page).boundingBox())?.x).toBe(30);

    // The drawer is laid over the whole screen: its fill reaches the left
    // edge, its content starts after what the device keeps there and at the
    // top, and its last button ends above what is kept at the bottom.
    await page.getByTestId('expand-sidebar').click();
    await expect.poll(async () => (await sidebar(page).boundingBox())?.x).toBe(0);
    const drawer = await sidebar(page).boundingBox();
    expect(drawer?.y).toBe(0);
    expect(drawer?.height).toBe(700);
    expect(drawer?.width).toBe(288 + 30);
    const search = await sidebar(page).getByTestId('sidebar-search').boundingBox();
    expect(search?.x).toBeGreaterThanOrEqual(30);
    const close = await sidebar(page).getByTestId('close-sidebar').boundingBox();
    expect(close?.y).toBeGreaterThanOrEqual(24);
    const account = await sidebar(page).getByTestId('open-settings').boundingBox();
    expect((account?.y ?? 0) + (account?.height ?? 0)).toBeLessThanOrEqual(700 - 34);
  });
});

/**
 * Pictures of the shell, into `target/screens/shell/` (`e2e/screens.ts`): each screen at the stage of
 * the project, on the light and on the dark screen. Not part of a usual run:
 *
 *   FVOCI_SCREENS=1 pnpm exec playwright test e2e/shell.spec.ts -g pictures
 */
test.describe('pictures', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'only when FVOCI_SCREENS is set');

  const FOLDER = screensDir('shell');

  /** What is put on the screen before each picture is taken. */
  const SCREENS: Record<string, (page: Page) => Promise<void>> = {
    // The page as it loads: the sidebar in the shape of the stage.
    frame: async () => undefined,
    // The whole list of boards: the drawer, or on an expanded page (where the
    // list is on the page anyway) the fixed sidebar with its search in use.
    sidebar: async (page) => {
      await showSidebar(page);
      if ((await stageOf(page)) === 'expanded') {
        await sidebar(page).getByTestId('sidebar-search').fill('plan');
      }
    },
    // The menu of a board: a sheet on a compact page, a dropdown elsewhere.
    menu: async (page) => {
      await showSidebar(page);
      await boardMenu(page, 'Issue tracker').click();
      await expect(page.getByRole('menuitem', { name: 'Delete board' })).toBeVisible();
    },
    // The fixed sidebar folded to the strip (expanded), or the strip and the
    // drawer's opener as they stand (medium, compact): a read-only board on the page.
    readonly: async (page) => {
      await showSidebar(page);
      await boardLink(page, READ_ONLY_BOARD).click();
      await expect(topBar(page).getByTestId('read-only')).toBeVisible();
      if ((await stageOf(page)) === 'expanded') {
        await sidebar(page).getByRole('button', { name: 'Collapse sidebar' }).click();
        await expect(strip(page)).toBeVisible();
      }
    },
    settings: async (page) => {
      if (await strip(page).isVisible()) {
        await strip(page).getByTestId('strip-open-settings').click();
      } else {
        await showSidebar(page);
        await sidebar(page).getByTestId('open-settings').click();
      }
      await expect(page.getByTestId('settings-dialog')).toBeVisible();
    },
  };

  for (const scheme of ['light', 'dark'] as const) {
    for (const [name, arrange] of Object.entries(SCREENS)) {
      test(`${name}, ${scheme}`, async ({ page }) => {
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        await openApp(page, '/?dev=1');
        if (scheme === 'dark') await expect(page.locator('html')).toHaveClass(/\bdark\b/);
        const stage = await stageOf(page);
        await arrange(page);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForLoadState('networkidle');
        await page.screenshot({
          path: path.join(FOLDER, `${name}-${stage}-${scheme}.png`),
          scale: 'css',
          animations: 'disabled',
        });
      });
    }
  }
});
