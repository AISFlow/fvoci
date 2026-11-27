/**
 * The kanban view on the in-memory backend: columns and cards as the view
 * says, opening and making cards, dragging with a mouse, a finger and the
 * keyboard, several cards at once, the columns themselves, hidden columns,
 * one column at a time in a narrow pane, the menus, and a board the reader
 * may only look at.
 *
 * Every test starts from the same seed. The board is given the width it has
 * in its project under the real shell (kanban.support.ts).
 */
import { expect, test, type Page } from '@playwright/test';

import { primaryName } from './keyboard.support.ts';
import { scrollTo } from './scroll.support.ts';
import { ON_A_CARD, placeOf, viewIdOf } from './address.ts';
import {
  announcer,
  boardPage,
  card,
  cardsBox,
  choose,
  column,
  columnNames,
  dialog,
  expectNoDrag,
  Finger,
  fitPane,
  grab,
  heading,
  hover,
  kanban,
  nextFrame,
  notice,
  oneColumn,
  openView,
  overlay,
  pointOn,
  seenResized,
  titlesIn,
  touch,
  undo,
} from './kanban.support.ts';
import { screensDir } from './screens.ts';

const TASKS = ['Project schedule', 'Status board'] as const;
const PLANNED = ['Build', 'Write the help pages', 'Test', 'Launch'];
const IN_PROGRESS = ['Design the screens'];

/** Opens the menu of a card and chooses a line. */
async function cardMenu(page: Page, title: string, action: string): Promise<void> {
  await page.getByRole('button', { name: `Card menu: ${title}` }).click();
  await choose(page, action);
}

async function columnMenu(page: Page, name: string, action: string): Promise<void> {
  await page.getByRole('button', { name: `Column menu: ${name}` }).click();
  await choose(page, action);
}

/** Whether a column lies wholly inside the board's box. */
async function inView(page: Page, name: string): Promise<boolean> {
  const box = await column(page, name).boundingBox();
  const within = await kanban(page).boundingBox();
  if (!box || !within) return false;
  return box.x >= within.x - 1 && box.x + box.width <= within.x + within.width + 1;
}

test.describe('columns and cards', () => {
  test('the columns and their cards are those of the view, in its order', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    expect(await columnNames(page)).toEqual(['Planned', 'In progress', 'Blocked', 'Done']);
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
    expect(await titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);
    expect(await titlesIn(page, 'Blocked')).toEqual([]);
    await expect(column(page, 'Planned').locator('[data-column-count]')).toContainText('4');

    // A card shows the properties the view turned on, and that it has a body.
    const face = card(page, 'Design the screens');
    await expect(face).toContainText('December 04');
    await expect(face.locator('[data-slot="card-has-body"]')).toBeAttached();
  });

  test('a board view that names no property is grouped by the first that can group', async ({
    page,
  }, info) => {
    await openView(page, info, 'Customer contacts', 'By stage');
    await page.evaluate(
      async ({ boardId, viewId }) => {
        const { backend } = (
          window as unknown as {
            fvociDev: {
              backend: {
                patchView(boardId: string, viewId: string, patch: object): Promise<unknown>;
              };
            };
          }
        ).fvociDev;
        await backend.patchView(boardId, viewId, { updatedFields: { groupBy: '' } });
      },
      { boardId: placeOf(page.url()).boardId, viewId: viewIdOf(page.url()) },
    );
    expect((await columnNames(page)).slice(0, 3)).toEqual(['Lead', 'Talking', 'Proposal sent']);
    await expect(page.getByTestId('hidden-columns')).toContainText('No Stage');
  });

  test('a press on a card opens it, and so does Enter', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await card(page, 'Design the screens').click();
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toContainText('Design the screens');
    await expect(page).toHaveURL(ON_A_CARD);
    await page.goBack();
    await expect(dialog(page)).toBeHidden();

    await card(page, 'Build').focus();
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toContainText('Build');
  });

  test('"+" in the heading of a column makes a card there and opens it', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await page.getByRole('button', { name: 'New card in Blocked' }).click();
    await expect(dialog(page)).toBeVisible();
    await page.goBack();
    await expect(dialog(page)).toBeHidden();
    expect(await titlesIn(page, 'Blocked')).toEqual(['Untitled']);

    // Undo deletes exactly that card.
    await undo(page);
    await expect(column(page, 'Blocked').locator('[data-card]')).toHaveCount(0);
  });
});

test.describe('dragging cards with a mouse', () => {
  test.skip(({ hasTouch }) => hasTouch, 'a mouse');

  test('a card goes to the place it is dropped at in another column, and undo takes it back', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await grab(page, card(page, 'Design the screens'));
    await hover(page, card(page, 'Write the help pages'), 0.25);
    await expect(page.locator('[data-drag-gap]')).toHaveCount(1);
    await page.mouse.up();
    await expectNoDrag(page);

    await expect
      .poll(() => titlesIn(page, 'Planned'))
      .toEqual(['Build', 'Design the screens', 'Write the help pages', 'Test', 'Launch']);
    expect(await titlesIn(page, 'In progress')).toEqual([]);
    // The drop did not open the card.
    await expect(dialog(page)).toBeHidden();

    // One undo takes back both the column and the place.
    await undo(page);
    await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
    expect(await titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);
  });

  test('a card is moved within its column, and into an empty one', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await grab(page, card(page, 'Test'));
    await hover(page, card(page, 'Build'), 0.25);
    await page.mouse.up();
    await expect
      .poll(() => titlesIn(page, 'Planned'))
      .toEqual(['Test', 'Build', 'Write the help pages', 'Launch']);

    await grab(page, card(page, 'Write the help pages'));
    await hover(page, cardsBox(page, 'Blocked'), 0.5);
    await page.mouse.up();
    await expect.poll(() => titlesIn(page, 'Blocked')).toEqual(['Write the help pages']);
  });

  test('letting go outside the board cancels, and says so while there', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await grab(page, card(page, 'Test'));
    const header = await pointOn(page.getByTestId('view-header'));
    await page.mouse.move(header.x, header.y, { steps: 8 });
    await expect(notice(page)).toHaveText('Let go to cancel.');
    await expect(kanban(page)).toHaveAttribute('data-outside', '');
    await page.mouse.up();
    await expectNoDrag(page);
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('a window that changes width under a drag cancels it', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await grab(page, card(page, 'Test'));
    await hover(page, card(page, 'Design the screens'), 0.25);
    const size = page.viewportSize();
    if (!size) throw new Error('no viewport');
    await page.setViewportSize({ width: size.width - 200, height: size.height });
    await expectNoDrag(page);
    await expect(announcer(page)).toContainText('Cancelled.');
    await page.mouse.up();
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
    expect(await titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);
    await expect(dialog(page)).toBeHidden();
  });

  test('the selected cards go together: Ctrl adds, Shift takes a range, Escape lets go', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    const modifier = await primaryName(page);
    // The other platform's modifier neither opens nor selects: pressed before the right one,
    // the card is selected once and nothing opened.
    await card(page, 'Write the help pages').focus();
    await page.keyboard.press(`${modifier === 'Meta' ? 'Control' : 'Meta'}+Enter`);
    await page.keyboard.press(`${modifier}+Enter`);
    await expect(page.getByTestId('selection-bar')).toContainText('1 selected');
    await expect(dialog(page)).toBeHidden();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('selection-bar')).toHaveCount(0);

    await card(page, 'Build').click({ modifiers: [modifier] });
    await card(page, 'Test').click({ modifiers: ['Shift'] });
    await expect(page.getByTestId('selection-bar')).toContainText('3 selected');
    await card(page, 'Write the help pages').click({ modifiers: [modifier] });
    await expect(page.getByTestId('selection-bar')).toContainText('2 selected');
    await expect(page.locator('[data-card][data-selected]')).toHaveCount(2);
    // Selecting opened nothing.
    await expect(dialog(page)).toBeHidden();

    await grab(page, card(page, 'Test'));
    await expect(overlay(page).locator('[data-slot="card-count"]')).toHaveText('2');
    await hover(page, card(page, 'Design the screens'), 0.25);
    await page.mouse.up();
    await expect
      .poll(() => titlesIn(page, 'In progress'))
      .toEqual(['Build', 'Test', ...IN_PROGRESS]);
    expect(await titlesIn(page, 'Planned')).toEqual(['Write the help pages', 'Launch']);
    // A selection that was dropped has done its work.
    await expect(page.getByTestId('selection-bar')).toHaveCount(0);

    await card(page, 'Write the help pages').click({ modifiers: [modifier] });
    await expect(page.getByTestId('selection-bar')).toBeVisible();
    await card(page, 'Write the help pages').focus();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('selection-bar')).toHaveCount(0);

    // One undo takes the two cards back.
    await undo(page);
    await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('a column is dragged by its heading, and its cards stay in it', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await grab(page, heading(page, 'In progress'));
    const first = await pointOn(column(page, 'Planned'), 0.2);
    await page.mouse.move(first.x - 40, first.y, { steps: 8 });
    await nextFrame(page);
    await expect(page.locator('[data-column-gap]')).toHaveCount(1);
    await page.mouse.up();
    await expectNoDrag(page);
    await expect
      .poll(() => columnNames(page))
      .toEqual(['In progress', 'Planned', 'Blocked', 'Done']);
    expect(await titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);

    // Pressing a card drags the card, not its column.
    await grab(page, card(page, 'Design the screens'));
    await expect(page.locator('[data-drag-gap]')).toHaveCount(1);
    await expect(page.locator('[data-column-gap]')).toHaveCount(0);
    await page.keyboard.press('Escape');
    await page.mouse.up();
    await expectNoDrag(page);

    await undo(page);
    await expect.poll(async () => (await columnNames(page))[0]).toBe('Planned');
  });

  test('a card dropped on a hidden column takes its value and leaves the board', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await columnMenu(page, 'Blocked', 'hide');
    const hidden = page.locator('[data-hidden-column]').filter({ hasText: 'Blocked' });
    await expect(hidden.locator('[data-hidden-count]')).toContainText('0');

    await grab(page, card(page, 'Test'));
    // The hidden columns stand after the last column, out of view: the right edge of the board brings them in.
    const box = await kanban(page).boundingBox();
    if (!box) throw new Error('no board');
    await page.mouse.move(box.x + box.width - 10, box.y + 200, { steps: 8 });
    await expect
      .poll(async () => {
        const zone = await hidden.boundingBox();
        return zone !== null && zone.x + zone.width / 2 < box.x + box.width - 80;
      })
      .toBe(true);
    await hover(page, hidden, 0.5);
    await expect(hidden).toHaveAttribute('data-over', '');
    await expect(page.locator('[data-drag-gap]')).toHaveCount(0);
    await page.mouse.up();
    await expectNoDrag(page);
    await expect(card(page, 'Test')).toHaveCount(0);
    await expect(hidden.locator('[data-hidden-count]')).toContainText('1');

    await page.getByRole('button', { name: 'Show Blocked' }).click();
    expect((await columnNames(page)).at(-1)).toBe('Blocked');
    expect(await titlesIn(page, 'Blocked')).toEqual(['Test']);
  });
});

test.describe('dragging with a finger', () => {
  test.skip(({ hasTouch }) => !hasTouch, 'a finger');

  test('a card is picked up by resting on it and dropped at a place', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    const finger = await Finger.on(page);
    await finger.rest(card(page, 'Test'));
    await expect(kanban(page)).toHaveAttribute('data-dragging', 'pointer');
    await finger.hover(card(page, 'Build'), 0.25);
    await finger.up();
    await expectNoDrag(page);
    await expect
      .poll(() => titlesIn(page, 'Planned'))
      .toEqual(['Test', 'Build', 'Write the help pages', 'Launch']);
    await expect(dialog(page)).toBeHidden();
  });

  test('a swipe that starts on a card scrolls the board and picks nothing up', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    const finger = await Finger.on(page);
    const from = await pointOn(card(page, 'Write the help pages'));
    await finger.down(from);
    await finger.move({ x: from.x - 260, y: from.y }, 8);
    await finger.up();
    await expect(overlay(page)).toHaveCount(0);
    await expect
      .poll(() => kanban(page).evaluate((element) => element.scrollLeft))
      .toBeGreaterThan(0);
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('at the edge of the screen the board goes on to the next column, and the card lands there', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    const finger = await Finger.on(page);
    await finger.rest(card(page, 'Test'));
    // While a card is dragged the board does not come to rest on a column.
    const snap = (): Promise<string> =>
      kanban(page).evaluate((element) => getComputedStyle(element).scrollSnapType);
    expect(await snap()).toBe('none');

    const box = await kanban(page).boundingBox();
    if (!box) throw new Error('no board');
    const scrolled = (): Promise<number> => kanban(page).evaluate((element) => element.scrollLeft);
    const start = await scrolled();
    // Held at the right edge, the board goes on: far enough for another column to be in the middle.
    const edge = { x: box.x + box.width - 12, y: box.y + 140 };
    await finger.move(edge);
    await expect
      .poll(async () => {
        await finger.move(edge, 1);
        return (await scrolled()) - start;
      })
      .toBeGreaterThan(250);
    // Away from the edge the board stays where it is. The card goes to the column in the middle now:
    // which one that is depends on how long the finger was held.
    await finger.move({ x: box.x + box.width / 2, y: box.y + box.height / 2 });
    const stopped = await scrolled();
    await nextFrame(page);
    await nextFrame(page);
    expect(await scrolled()).toBe(stopped);
    const under = await page.evaluate(
      (middle) => {
        const columns = [...document.querySelectorAll<HTMLElement>('[data-column]')];
        const distance = (element: HTMLElement): number => {
          const rect = element.getBoundingClientRect();
          return Math.abs(rect.left + rect.width / 2 - middle);
        };
        columns.sort((first, second) => distance(first) - distance(second));
        return columns[0]?.querySelector('h3')?.textContent.trim() ?? '';
      },
      box.x + box.width / 2,
    );
    expect(under).not.toBe('Planned');
    const before = await titlesIn(page, under);
    await finger.hover(cardsBox(page, under), 0.9);
    await finger.up();
    await expectNoDrag(page);
    await expect.poll(() => titlesIn(page, under)).toEqual([...before, 'Test']);
    expect(await titlesIn(page, 'Planned')).toEqual(['Build', 'Write the help pages', 'Launch']);
    // Where one column shows at a time, the board rests on a column again.
    expect(await snap()).toBe(oneColumn(info) ? 'x mandatory' : 'none');
  });

  test('a column is picked up by resting on its heading', async ({ page }, info) => {
    test.skip(oneColumn(info), 'where columns stand side by side');
    await openView(page, info, ...TASKS);
    const finger = await Finger.on(page);
    await finger.rest(heading(page, 'Planned'));
    const second = await pointOn(column(page, 'In progress'), 0.2);
    await finger.move({ x: second.x + 60, y: second.y });
    await expect(page.locator('[data-column-gap]')).toHaveCount(1);
    await finger.up();
    await expectNoDrag(page);
    await expect
      .poll(async () => (await columnNames(page)).slice(0, 2))
      .toEqual(['In progress', 'Planned']);
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('the device buzzes when a card is picked up, unless the reader turned that off', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await page.evaluate(() => {
      const buzzes: number[] = [];
      Object.assign(window, { buzzes });
      Object.defineProperty(navigator, 'vibrate', {
        value: (duration: number) => {
          buzzes.push(duration);
          return true;
        },
        configurable: true,
      });
    });
    const buzzes = (): Promise<number[]> =>
      page.evaluate(() => (window as unknown as { buzzes: number[] }).buzzes);
    const finger = await Finger.on(page);
    await finger.rest(card(page, 'Test'));
    await finger.up();
    await expectNoDrag(page);
    expect(await buzzes()).toEqual([15]);

    await page.evaluate(() =>
      (
        window as unknown as {
          fvociDev: { workspace: { changeSettings(patch: object): Promise<void> } };
        }
      ).fvociDev.workspace.changeSettings({ haptics: false }),
    );
    await expect(kanban(page)).toHaveAttribute('data-haptics', 'false');
    await finger.rest(card(page, 'Test'));
    await finger.up();
    await expectNoDrag(page);
    expect(await buzzes()).toEqual([15]);
  });
});

test.describe('the keyboard', () => {
  test('carries a card to another column and a place in it, and says where it is', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await card(page, 'Design the screens').focus();
    await page.keyboard.press('Space');
    await expect(kanban(page)).toHaveAttribute('data-dragging', 'keyboard');
    await expect(announcer(page)).toHaveText(
      'Picked up Design the screens. In progress, position 1 of 1.',
    );
    await page.keyboard.press('ArrowLeft');
    await expect(announcer(page)).toHaveText('Design the screens: Planned, position 1 of 5.');
    await page.keyboard.press('ArrowDown');
    await expect(page.locator('[data-drag-carried]')).toBeFocused();
    await page.keyboard.press('Space');
    await expect(announcer(page)).toHaveText(
      'Dropped Design the screens in Planned, position 2 of 5.',
    );
    expect(await titlesIn(page, 'Planned')).toEqual([
      'Build',
      'Design the screens',
      'Write the help pages',
      'Test',
      'Launch',
    ]);
    // The focus is on the card where it now is.
    await expect(card(page, 'Design the screens')).toBeFocused();

    await undo(page);
    await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('Escape puts a carried card back', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await card(page, 'Build').focus();
    await page.keyboard.press('Space');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Escape');
    await expectNoDrag(page);
    await expect(announcer(page)).toContainText('Cancelled.');
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
    await expect(card(page, 'Build')).toBeFocused();
  });

  test('carries a column along the row', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await heading(page, 'Planned').focus();
    await page.keyboard.press('Space');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    await expect(page.getByTestId('column-announcer')).toHaveText(
      'Column Planned: position 3 of 4.',
    );
    await page.keyboard.press('Enter');
    expect((await columnNames(page)).slice(0, 3)).toEqual(['In progress', 'Blocked', 'Planned']);
    await expect(heading(page, 'Planned')).toBeFocused();
    expect(await titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('carries a card onto a hidden column', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await columnMenu(page, 'Done', 'hide');
    await card(page, 'Design the screens').focus();
    await page.keyboard.press('Space');
    // Past Blocked, the last visible column, to the hidden columns (No Status, then Done).
    for (let step = 0; step < 2; step += 1) await page.keyboard.press('ArrowRight');
    await expect(announcer(page)).toHaveText('Design the screens: hidden column No Status.');
    await page.keyboard.press('ArrowRight');
    await expect(announcer(page)).toHaveText('Design the screens: hidden column Done.');
    await expect(page.locator('[data-drag-carried]')).toBeFocused();
    await page.keyboard.press('Space');
    await expect(card(page, 'Design the screens')).toHaveCount(0);
    await expect(page.locator('[data-hidden-column]').filter({ hasText: 'Done' })).toContainText(
      '2',
    );
  });
});

test.describe('a sorted view', () => {
  const MEETINGS = ['Meeting notes', 'By kind'] as const;

  test('the keyboard chooses the column only, and the sort places the card', async ({
    page,
  }, info) => {
    await openView(page, info, ...MEETINGS);
    const before = await titlesIn(page, 'Weekly');
    expect(before).toHaveLength(2);
    await card(page, 'Design review').focus();
    await page.keyboard.press('Space');
    await expect(announcer(page)).toContainText('This view is sorted');
    await expect(notice(page)).toHaveText(
      'This view is sorted: choose a column, and the sort places the card.',
    );
    // Within a column there is nowhere to go.
    await page.keyboard.press('ArrowDown');
    await expect(announcer(page)).toContainText('This view is sorted');
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('Space');
    await expectNoDrag(page);
    await expect(notice(page)).toHaveCount(0);

    const after = await titlesIn(page, 'Weekly');
    expect(after).toHaveLength(3);
    expect(after.filter((title) => title !== 'Design review')).toEqual(before);
    // Where the card stands is where the board store's order of the view puts it.
    const expected = await page.evaluate((id) => {
      const { workspace } = (
        window as unknown as {
          fvociDev: {
            workspace: {
              board: {
                orderedViews: { id: string }[];
                view(id: string): object;
                cardsOf(view: object): { title: string }[];
              };
            };
          };
        }
      ).fvociDev;
      return workspace.board.cardsOf(workspace.board.view(id)).map((shown) => shown.title);
    }, viewIdOf(page.url()));
    expect(after).toEqual(expected.filter((title) => after.includes(title)));
  });

  test('a mouse sees the gap where the sort would put the card, wherever it points', async ({
    page,
  }, info) => {
    test.skip(touch(info), 'a mouse');
    await openView(page, info, ...MEETINGS);
    await grab(page, card(page, 'Design review'));
    await expect(notice(page)).toContainText('This view is sorted');
    await hover(page, cardsBox(page, 'Weekly'), 0.1);
    const high = await kanban(page).getAttribute('data-drop-index');
    await hover(page, cardsBox(page, 'Weekly'), 0.95);
    await expect(kanban(page)).toHaveAttribute('data-drop-index', high ?? '');
    await page.mouse.up();
    await expect.poll(async () => (await titlesIn(page, 'Weekly')).length).toBe(3);
    expect(await titlesIn(page, 'Review')).toEqual([]);
  });
});

test.describe('the menu of a card', () => {
  test('"Move to another column" moves the card to the end of that column', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await cardMenu(page, 'Build', 'move');
    await page.locator('[data-action^="to-"]').filter({ hasText: 'In progress' }).click();
    await expect.poll(() => titlesIn(page, 'In progress')).toEqual([...IN_PROGRESS, 'Build']);
    await undo(page);
    await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('"Select" takes cards into a selection that moves together', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await cardMenu(page, 'Build', 'select');
    await expect(page.getByTestId('selection-bar')).toContainText('1 selected');
    // From now on a plain press selects.
    await card(page, 'Test').click();
    await expect(page.getByTestId('selection-bar')).toContainText('2 selected');
    await expect(dialog(page)).toBeHidden();

    await cardMenu(page, 'Test', 'move');
    await page.locator('[data-action^="to-"]').filter({ hasText: 'Blocked' }).click();
    await expect.poll(() => titlesIn(page, 'Blocked')).toEqual(['Build', 'Test']);
    await expect(page.getByTestId('selection-bar')).toHaveCount(0);
    // A plain press opens again.
    await card(page, 'Write the help pages').click();
    await expect(dialog(page)).toBeVisible();
  });

  test('deletes without asking, and undo brings the card back', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await cardMenu(page, 'Write the help pages', 'delete');
    await expect(card(page, 'Write the help pages')).toHaveCount(0);
    await expect(page.getByTestId('confirm-dialog')).toHaveCount(0);
    await undo(page);
    await expect.poll(() => titlesIn(page, 'Planned')).toEqual(PLANNED);
  });

  test('duplicates the card into the same column', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await cardMenu(page, 'Design the screens', 'duplicate');
    await expect(card(page, 'Design the screens')).toHaveCount(2);
    expect(await titlesIn(page, 'In progress')).toEqual([...IN_PROGRESS, 'Design the screens']);
  });

  test('copies the address of the card', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await page.evaluate(() => {
      Object.defineProperty(navigator, 'clipboard', {
        value: {
          writeText: (text: string) => {
            document.body.dataset.copied = text;
            return Promise.resolve();
          },
        },
        configurable: true,
      });
    });
    await cardMenu(page, 'Design the screens', 'copy-link');
    await expect(page.getByTestId('link-copied').filter({ hasText: 'Copied!' })).toHaveCount(1);
    const copied = await page.evaluate(() => document.body.dataset.copied ?? '');
    expect(copied).toMatch(/^http/);
    expect(copied).toMatch(ON_A_CARD);
    // The address opens the card.
    await page.goto(copied);
    await expect(dialog(page)).toContainText('Design the screens');
  });
});

test.describe('the menu of a column', () => {
  test('hides a column and shows it again', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await columnMenu(page, 'In progress', 'hide');
    expect(await columnNames(page)).not.toContain('In progress');
    const hidden = page.locator('[data-hidden-column]').filter({ hasText: 'In progress' });
    await expect(hidden.locator('[data-hidden-count]')).toContainText('1');
    await page.getByRole('button', { name: 'Show In progress' }).click();
    expect((await columnNames(page)).at(-1)).toBe('In progress');
    expect(await titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);
  });

  test('moves a column a place to the right and back', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await columnMenu(page, 'Planned', 'right');
    expect((await columnNames(page)).slice(0, 2)).toEqual(['In progress', 'Planned']);
    await columnMenu(page, 'Planned', 'left');
    expect((await columnNames(page)).slice(0, 2)).toEqual(['Planned', 'In progress']);
    await page.getByRole('button', { name: 'Column menu: Planned' }).click();
    await expect(page.locator('[data-action="left"]')).toBeDisabled();
  });

  test('renames a group, gives it another colour, and deletes it after asking', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await columnMenu(page, 'Blocked', 'rename');
    const field = page.getByRole('textbox', { name: 'Group name' });
    await expect(field).toBeFocused();
    await field.fill('Waiting');
    await field.press('Enter');
    expect(await columnNames(page)).toContain('Waiting');

    await columnMenu(page, 'Waiting', 'color');
    await page.getByRole('radio', { name: 'Purple' }).click();
    await expect(column(page, 'Waiting').locator('h3 [data-slot="option-chip"]')).toHaveAttribute(
      'data-color',
      'purple',
    );

    await page.getByRole('button', { name: 'Show No Status' }).click();
    await columnMenu(page, 'In progress', 'delete');
    await expect(page.getByTestId('confirm-dialog')).toContainText(
      'Delete the group “In progress”?',
    );
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    expect(await columnNames(page)).not.toContain('In progress');
    // Its cards have a value that names nothing now: they show without one.
    expect(await titlesIn(page, 'No Status')).toEqual(IN_PROGRESS);
    await undo(page);
    await expect.poll(() => titlesIn(page, 'In progress')).toEqual(IN_PROGRESS);
  });

  test('"+ Add a group" adds a column and asks for its name', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await page.getByTestId('add-group').click();
    const field = page.getByRole('textbox', { name: 'Group name' });
    await expect(field).toBeFocused();
    await expect(field).toHaveValue('New group');
    await field.fill('Review');
    await field.press('Enter');
    expect((await columnNames(page)).at(-1)).toBe('Review');
    await page.getByRole('button', { name: 'New card in Review' }).click();
    await expect(dialog(page)).toBeVisible();
  });
});

test.describe('a pane that fits one column', () => {
  /** A phone held upright, whatever the project. */
  const NARROW = 400;

  test('shows a column at a time, with a tab for each', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await fitPane(page, NARROW);
    const tabs = page.getByTestId('column-tabs');
    await expect(tabs).toBeVisible();
    await expect(tabs.getByRole('tab')).toHaveCount(5);
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('Planned');
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('4');

    // A column is as wide as the pane less a sliver of its neighbours at either side.
    const box = await column(page, 'Planned').boundingBox();
    expect(box?.width).toBe(Math.min(NARROW - 56, 384));
    expect(await inView(page, 'Planned')).toBe(true);
    expect(await inView(page, 'In progress')).toBe(false);

    // A tab goes to its column.
    await tabs.getByRole('tab', { name: /Blocked/ }).click();
    await expect.poll(() => inView(page, 'Blocked')).toBe(true);
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('Blocked');

    // Scrolled a little further, the board comes to rest on a column, not between two.
    await kanban(page).evaluate((element) => {
      element.scrollBy({ left: 150 });
    });
    await expect
      .poll(async () => {
        const names = ['Blocked', 'Done'];
        const resting = await Promise.all(names.map((name) => inView(page, name)));
        return resting.filter(Boolean).length;
      })
      .toBe(1);
  });

  test('a pane that changes width but still fits one column goes on following the reader', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    // From 440 pixels on, the board and its columns keep their inner width of 24rem.
    await fitPane(page, 480);
    await fitPane(page, 520);
    const tabs = page.getByTestId('column-tabs');
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('Planned');

    // Scrolled on by a column, as a finger or the wheel does: the tab follows.
    await kanban(page).evaluate((element) => {
      element.scrollBy({ left: 384 });
    });
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('In progress');
    await expect.poll(() => inView(page, 'In progress')).toBe(true);
  });

  test('a column picked up by its heading leaves the board on the column looked at', async ({
    page,
  }, info) => {
    test.skip(touch(info), 'a mouse');
    await openView(page, info, ...TASKS);
    await fitPane(page, NARROW);
    const tabs = page.getByTestId('column-tabs');
    await tabs.getByRole('tab', { name: /Blocked/ }).click();
    await expect.poll(() => inView(page, 'Blocked')).toBe(true);
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('Blocked');

    // The column is taken out of the row while it is carried, and put back where it was: the
    // board has not gone to its start meanwhile, nor does the tab name a neighbour.
    await grab(page, heading(page, 'Blocked'));
    await expect(page.locator('[data-column-gap]')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await page.mouse.up();
    await expectNoDrag(page);
    await expect.poll(() => inView(page, 'Blocked')).toBe(true);
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('Blocked');
  });

  test('wider, the tabs go and the columns stand side by side; narrow again, the column looked at is back', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await fitPane(page, NARROW);
    // Enough cards in a column for it to scroll.
    await page.evaluate(async (boardId) => {
      const { backend } = (
        window as unknown as {
          fvociDev: {
            backend: {
              cards(
                boardId: string,
              ): Promise<
                { id: string; title: string; fields: { properties: Record<string, unknown> } }[]
              >;
              createCard(boardId: string, draft: object): Promise<unknown>;
            };
          };
        }
      ).fvociDev;
      const model = (await backend.cards(boardId)).find(
        (shown) => shown.title === 'Design the screens',
      );
      for (let count = 1; count <= 14; count += 1) {
        await backend.createCard(boardId, {
          title: `Filler ${count}`,
          properties: model?.fields.properties,
        });
      }
    }, placeOf(page.url()).boardId);
    await expect(column(page, 'In progress').locator('[data-card]')).toHaveCount(15);

    const tabs = page.getByTestId('column-tabs');
    await tabs.getByRole('tab', { name: /In progress/ }).click();
    await expect.poll(() => inView(page, 'In progress')).toBe(true);
    // Scrolled down in the column, with the focus on a card there.
    const list = cardsBox(page, 'In progress');
    const looked = card(page, 'Filler 9');
    await looked.focus();
    expect(await list.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    /** The card is in the part of the column that shows. A column that reflows keeps what was in view, not a number of pixels. */
    const stillThere = async (): Promise<boolean> => {
      const box = await looked.boundingBox();
      const within = await list.boundingBox();
      if (!box || !within) return false;
      return box.y + box.height > within.y && box.y < within.y + within.height;
    };
    expect(await stillThere()).toBe(true);

    await fitPane(page, 900);
    await expect(tabs).toBeHidden();
    expect((await column(page, 'In progress').boundingBox())?.width).toBe(256);
    expect(await inView(page, 'In progress')).toBe(true);
    expect(await list.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    expect(await stillThere()).toBe(true);
    await expect(looked).toBeFocused();

    await fitPane(page, 1040);
    expect((await column(page, 'In progress').boundingBox())?.width).toBe(272);

    await fitPane(page, NARROW);
    await expect(tabs.getByRole('tab', { selected: true })).toContainText('In progress');
    const box = await column(page, 'In progress').boundingBox();
    const within = await kanban(page).boundingBox();
    if (!box || !within) throw new Error('no column');
    expect(Math.abs(box.x + box.width / 2 - (within.x + within.width / 2))).toBeLessThan(2);
    expect(await list.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    expect(await stillThere()).toBe(true);
    await expect(looked).toBeFocused();
  });
});

test.describe('a pane that fits several columns', () => {
  test('a pane that changes width leaves the board where it was scrolled to', async ({
    page,
  }, info) => {
    await openView(page, info, ...TASKS);
    await fitPane(page, 700);
    await expect(page.getByTestId('column-tabs')).toBeHidden();
    // The first column half out of view at the left: still the one looked at.
    await scrollTo(kanban(page), 150, 0);
    const size = page.viewportSize();
    if (!size) throw new Error('the page has no viewport');
    await seenResized(kanban(page), () =>
      page.setViewportSize({ width: size.width + 40, height: size.height }),
    );
    expect(await kanban(page).evaluate((element) => element.scrollLeft)).toBe(150);
  });
});

test.describe('what a pointer decides', () => {
  test('the menu of a card waits for a pointer that can hover, and is always there for a finger', async ({
    page,
    hasTouch,
  }, info) => {
    await openView(page, info, ...TASKS);
    const button = page.getByRole('button', { name: 'Card menu: Design the screens' });
    const opacity = (): Promise<string> =>
      button.evaluate((element) => getComputedStyle(element).opacity);
    if (hasTouch) {
      expect(await opacity()).toBe('1');
    } else {
      // The button fades in and out: each state is waited for.
      await page.mouse.move(0, 0);
      await expect.poll(opacity).toBe('0');
      await card(page, 'Design the screens').hover();
      await expect.poll(opacity).toBe('1');
      // The keyboard gets to it too: it shows while the card or the button has the focus.
      await page.mouse.move(0, 0);
      await expect.poll(opacity).toBe('0');
      await card(page, 'Design the screens').focus();
      await expect.poll(opacity).toBe('1');
    }
  });

  test('what a finger presses is at least 44 pixels', async ({ page, hasTouch }, info) => {
    test.skip(!hasTouch, 'a finger');
    await openView(page, info, ...TASKS);
    for (const name of [
      'Card menu: Design the screens',
      'Column menu: In progress',
      'New card in In progress',
    ]) {
      const box = await page.getByRole('button', { name }).boundingBox();
      expect(box?.width, name).toBeGreaterThanOrEqual(44);
      expect(box?.height, name).toBeGreaterThanOrEqual(44);
    }
    const bottom = await column(page, 'In progress').locator('[data-column-new]').boundingBox();
    expect(bottom?.height).toBeGreaterThanOrEqual(44);
  });
});

test.describe('in Korean', () => {
  test.use({ locale: 'ko-KR' });

  test('the words of the board are Korean', async ({ page }, info) => {
    await openView(page, info, ...TASKS);
    await expect(page.getByTestId('hidden-columns')).toContainText('Status 속성 없음');
    await page.getByRole('button', { name: '카드 메뉴: Design the screens' }).click();
    await expect(page.locator('[data-action="move"]')).toHaveText(/다른 열로 이동/);
    await expect(page.locator('[data-action="duplicate"]')).toHaveText(/복제/);
    await page.keyboard.press('Escape');
    await card(page, 'Test').focus();
    await page.keyboard.press('Space');
    await expect(announcer(page)).toHaveText('집었습니다: Test. Planned, 4개 중 3번째.');
    await page.keyboard.press('Escape');
  });
});

test.describe('a board the reader may only look at', () => {
  test('shows its columns and opens its cards, and offers nothing that changes it', async ({
    page,
  }, info) => {
    await openView(page, info, 'Study notes', 'By subject');
    expect((await columnNames(page)).slice(0, 2)).toEqual(['Algorithms', 'Statistics']);
    await expect(page.getByRole('button', { name: /^New card in/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Column menu/ })).toHaveCount(0);
    await expect(page.locator('[data-column-new]')).toHaveCount(0);
    await expect(page.getByTestId('add-group')).toHaveCount(0);
    await expect(page.locator('[data-drag-handle]')).toHaveCount(0);

    // The menu of a card only copies its link.
    await page.getByRole('button', { name: 'Card menu: Big O notation' }).click();
    await expect(page.locator('[data-action]')).toHaveCount(1);
    await expect(page.locator('[data-action="copy-link"]')).toBeVisible();
    await page.keyboard.press('Escape');

    // Space picks nothing up, and neither does the pointer.
    const first = card(page, 'Big O notation');
    await first.focus();
    await page.keyboard.press('Space');
    await expect(kanban(page)).not.toHaveAttribute('data-dragging');
    if (!touch(info)) {
      const from = await pointOn(first);
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      // Move well outside the card so releasing the button does not fire a click that opens it.
      await page.mouse.move(from.x + 160, from.y + 160, { steps: 4 });
      await expect(overlay(page)).toHaveCount(0);
      await page.mouse.up();
    }
    await first.click({ modifiers: [await primaryName(page)] });
    await expect(dialog(page)).toBeVisible();
    await expect(page.getByTestId('selection-bar')).toHaveCount(0);
  });
});

test.describe('screens', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');

  const STAGE: Record<string, string> = {
    mobile: 'compact',
    middle: 'medium',
    desktop: 'expanded',
  };

  for (const scheme of ['light', 'dark'] as const) {
    test(`the board, ${scheme}`, async ({ page }, info) => {
      const stage = STAGE[info.project.name] ?? 'expanded';
      await page.emulateMedia({ colorScheme: scheme });
      await openView(page, info, ...TASKS);
      await columnMenu(page, 'Done', 'hide');
      // Back to the first column: the menu of the hidden one was at the far end.
      await kanban(page).evaluate((element) => {
        element.scrollLeft = 0;
      });
      await expect(boardPage(page)).toBeVisible();
      await expect.poll(() => inView(page, 'Planned')).toBe(true);
      await page.screenshot({ path: `${screensDir('kanban')}/board-${stage}-${scheme}.png` });

      await cardMenu(page, 'Write the help pages', 'select');
      await page.getByRole('button', { name: 'Card menu: Build' }).click();
      await expect(page.locator('[data-action="move"]')).toBeVisible();
      // The menu fades in: the picture waits for it.
      await page.evaluate(() =>
        Promise.all(document.getAnimations().map((animation) => animation.finished)),
      );
      await page.screenshot({ path: `${screensDir('kanban')}/card-menu-${stage}-${scheme}.png` });
      await page.keyboard.press('Escape');

      if (oneColumn(info)) return;
      await openView(page, info, 'Meeting notes', 'By kind');
      await card(page, 'Design review').focus();
      await page.keyboard.press('Space');
      await page.keyboard.press('ArrowLeft');
      await expect(notice(page)).toBeVisible();
      await page.screenshot({ path: `${screensDir('kanban')}/sorted-drag-${stage}-${scheme}.png` });
    });
  }
});
