/**
 * A finger on a large touch screen: an iPad held sideways (1180 by 820, the
 * Safari of an iPad Air 11 with its bars folded), where the layout is the
 * widest one yet everything is pressed with a finger.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { othersArrive } from './presence.support.ts';
import { openApp, sidebar } from './shell.support.ts';
import { newWiki, pageTree } from './wiki.support.ts';

test.use({ viewport: { width: 1180, height: 820 }, hasTouch: true });

// Each test sets its own screen: one project of a configuration is enough (Chromium's
// touched middle, WebKit's iPad).
test.beforeEach(() => {
  test.skip(
    test.info().project.name !== 'middle',
    'the screen is the test’s own; run in one project',
  );
});

/** A finger's 44 pixels, in both directions. */
async function expectFingerSized(target: Locator, name: string): Promise<void> {
  const box = await target.boundingBox();
  expect(box?.width, name).toBeGreaterThanOrEqual(44);
  expect(box?.height, name).toBeGreaterThanOrEqual(44);
}

/** The page itself does not scroll: every part of the layout scrolls inside its own box. */
async function expectPageStill(page: Page): Promise<void> {
  const page_ = await page.evaluate(() => ({
    wide: document.documentElement.scrollWidth - innerWidth,
    tall: document.documentElement.scrollHeight - innerHeight,
  }));
  expect(page_).toEqual({ wide: 0, tall: 0 });
}

test('the board: the page does not scroll, and what the bar offers a finger is 44 pixels', async ({
  page,
}) => {
  await openApp(page);
  await expect(page.getByTestId('app-shell')).toHaveAttribute('data-stage', 'expanded');
  await expect(sidebar(page)).toBeVisible();
  // The list of boards is longer than the screen with a finger's rows; it scrolls in itself.
  await expect
    .poll(() =>
      page.getByTestId('sidebar-list').evaluate((list) => list.scrollHeight > list.clientHeight),
    )
    .toBe(true);
  await expectPageStill(page);
  for (const id of ['share-board', 'board-members-count', 'board-title']) {
    await expectFingerSized(page.getByTestId(id), id);
  }
});

test('a card: its title and the boxes of its tasks are a finger wide', async ({ page }) => {
  await openApp(page);
  const place = await page.evaluate(async () => {
    type Backend = {
      workspaces(): Promise<{ id: string }[]>;
      boards(id: string): Promise<{ id: string; title: string }[]>;
      views(id: string): Promise<{ id: string }[]>;
      cards(id: string): Promise<{ id: string; title: string }[]>;
    };
    const { backend } = (window as unknown as { fvociDev: { backend: Backend } }).fvociDev;
    const [workspace] = await backend.workspaces();
    const board = (await backend.boards(workspace?.id ?? '')).find(
      (each) => each.title === 'Project schedule',
    );
    if (!board) throw new Error('no board of tasks');
    const [view] = await backend.views(board.id);
    const card = (await backend.cards(board.id)).find(
      (each) => each.title === 'Design the screens',
    );
    if (!card) throw new Error('no card');
    return { boardId: board.id, viewId: view?.id ?? '', cardId: card.id };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await expectPageStill(page);
  const title = page.getByTestId('card-title');
  expect((await title.boundingBox())?.height).toBeGreaterThanOrEqual(44);

  // The box of a task is drawn small; around it, a finger's square answers to it and to no other.
  const box = page.getByTestId('card-body').locator('ul[data-type="taskList"] input').nth(1);
  await box.scrollIntoViewIfNeeded();
  const drawn = await box.boundingBox();
  if (!drawn) throw new Error('no box of a task');
  const middle = { x: drawn.x + drawn.width / 2, y: drawn.y + drawn.height / 2 };
  const corners = [
    [-20, -20],
    [-20, 20],
    [8, -20],
    [8, 20],
  ];
  const answers = await box.evaluate(
    (input, { x, y, offsets }) =>
      offsets.map(([dx = 0, dy = 0]) => {
        const hit = document.elementFromPoint(x + dx, y + dy);
        return hit?.closest('label') === input.closest('label');
      }),
    { ...middle, offsets: corners },
  );
  expect(answers).toEqual([true, true, true, true]);
  // Just past the square, where the words begin, a press is for the text.
  const past = await box.evaluate((input, { x, y }) => {
    const hit = document.elementFromPoint(x + 24, y);
    return hit?.closest('label') === null && hit.closest('li') === input.closest('li');
  }, middle);
  expect(past).toBe(true);
  const checked = await box.isChecked();
  await page.touchscreen.tap(middle.x - 18, middle.y + 18);
  await expect(box).toBeChecked({ checked: !checked });
});

test('a wiki: the pages of its tree are a finger high where they are pressed', async ({ page }) => {
  await openApp(page);
  await newWiki(page, 'Handbook');
  await pageTree(page).getByRole('button', { name: 'New page' }).first().click();
  await expect(page.getByTestId('page-title')).toBeFocused();
  await page.keyboard.type('Getting started');
  await page.keyboard.press('Enter');
  const row = pageTree(page).getByRole('treeitem', { name: 'Getting started' });
  await expectFingerSized(row.getByRole('link'), 'the page in the tree');
  expect((await page.getByTestId('page-title').boundingBox())?.height).toBeGreaterThanOrEqual(44);
  await expectPageStill(page);
});

/** Of the bar's undo, redo, deleted cards and sharing, and its menu, those that show. */
async function shownInBar(page: Page): Promise<string[]> {
  return page
    .getByTestId('top-bar')
    .locator(
      '[data-testid="undo"], [data-testid="redo"], [data-testid="open-card-trash"], [data-testid="share-board"], [data-testid="bar-menu"]',
    )
    .evaluateAll((buttons) =>
      buttons
        .filter((button) => button.getBoundingClientRect().width > 0)
        .map((button) => (button as HTMLElement).dataset.testid ?? ''),
    );
}

for (const width of [360, 393]) {
  test.describe(`a phone held upright, ${width} wide`, () => {
    // 393: an iPhone's Safari with its bars folded; 360: the narrowest Android phones.
    test.use({ viewport: { width, height: 800 }, hasTouch: true });

    /**
     * The bar's buttons, a finger wide and on the screen, none of them over another or
     * over the title; the bell among them, and undo, redo, the deleted cards and sharing in
     * the menu at the end.
     */
    async function expectBarFits(page: Page, people: string): Promise<void> {
      const bar = page.getByTestId('top-bar');
      const ids = ['open-sidebar', people, 'open-search', 'open-notifications', 'bar-menu'];
      const boxes = [];
      for (const id of ids) {
        await expectFingerSized(bar.getByTestId(id), id);
        const box = await bar.getByTestId(id).boundingBox();
        if (!box) throw new Error(`no ${id}`);
        boxes.push(box);
      }
      expect(await shownInBar(page)).toEqual(['bar-menu']);
      for (const [index, box] of boxes.entries()) {
        const next = boxes[index + 1];
        if (next) expect(box.x + box.width, ids[index]).toBeLessThanOrEqual(next.x);
      }
      const last = boxes.at(-1);
      expect((last?.x ?? 0) + (last?.width ?? 0)).toBeLessThanOrEqual(width);
      const title = await bar.getByTestId('board-title').boundingBox();
      const group = await bar.getByTestId('board-people').boundingBox();
      if (!title || !group) throw new Error('no title or people');
      expect(title.x + title.width).toBeLessThanOrEqual(group.x);
      // Room for the name: 74 pixels at 360 and 107 at 393 in Chromium, alone or not (14 at
      // both while undo, redo and the deleted cards were buttons beside the search's; the bell
      // took the room sharing left for the menu).
      expect(title.width).toBeGreaterThanOrEqual(width === 360 ? 70 : 100);
      await expectPageStill(page);
    }

    test('alone or with others, the bar fits and the title keeps its room', async ({ page }) => {
      await openApp(page);
      // The search's button comes once the server is known to search.
      await expect(page.getByTestId('open-search')).toBeVisible();
      // Alone: the members, as a figure and the number.
      await expectBarFits(page, 'board-members-count');
      await expect(page.getByTestId('board-presence')).toBeHidden();
      // Somebody else comes: their face takes the members' place, not the title's.
      await othersArrive(page, 1);
      await expect(page.getByTestId('board-presence').locator('[data-avatar]')).toHaveCount(1);
      await expect(page.getByTestId('board-members-count')).toBeHidden();
      await expectBarFits(page, 'board-presence');
    });
  });
}

test.describe('the menu of a phone’s bar', () => {
  test.use({ viewport: { width: 393, height: 800 }, hasTouch: true });

  const line = (page: Page, action: string) =>
    page.getByRole('menu').locator(`[data-action="${action}"]`);

  test('undoes and redoes as the buttons do, and offers neither with nothing to', async ({
    page,
  }) => {
    await openApp(page);
    const menu = page.getByTestId('bar-menu');
    const title = page.getByTestId('board-title');
    await menu.click();
    await expect(line(page, 'undo')).toBeDisabled();
    await expect(line(page, 'redo')).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(menu).toBeFocused();

    await title.fill('Launch plan');
    await title.press('Enter');
    await menu.click();
    await expect(line(page, 'redo')).toBeDisabled();
    await line(page, 'undo').click();
    await expect(title).toHaveValue('Project schedule');
    // The sheet gives the focus back to what opened it.
    await expect(menu).toBeFocused();
    await menu.click();
    await expect(line(page, 'undo')).toBeDisabled();
    await line(page, 'redo').click();
    await expect(title).toHaveValue('Launch plan');
  });

  test('opens the deleted cards, and the focus comes back to the menu', async ({ page }) => {
    await openApp(page);
    const menu = page.getByTestId('bar-menu');
    await menu.click();
    await line(page, 'card-trash').click();
    const trash = page.getByTestId('card-trash');
    await expect(trash).toBeVisible();
    // The focus is in the dialog while it is open, also once the sheet has gone: the sheet
    // gives the focus back to "…" as it goes, which must not pull it out of the dialog.
    await expect(page.getByRole('menu')).toHaveCount(0);
    await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
    await expect
      .poll(() => trash.evaluate((dialog) => dialog.contains(document.activeElement)))
      .toBe(true);
    await page.keyboard.press('Escape');
    await expect(trash).toBeHidden();
    await expect(menu).toBeFocused();
  });

  test('where two columns fit, the buttons and not the menu: each once on either side', async ({
    page,
  }) => {
    await openApp(page);
    await expect(page.getByTestId('open-search')).toBeVisible();
    // A compact page's pane is the whole window: 544 is where two kanban columns fit.
    for (const [width, shown] of [
      [543, ['bar-menu']],
      [544, ['share-board', 'open-card-trash', 'undo', 'redo']],
    ] as const) {
      await page.setViewportSize({ width, height: 800 });
      await expect.poll(() => shownInBar(page)).toEqual(shown);
      await expectPageStill(page);
    }
  });
});
