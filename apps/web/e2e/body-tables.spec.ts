/**
 * Tables in a body, on the backend in memory: one is made, written in and
 * changed with the keyboard alone (the slash menu, Tab); a Markdown table and
 * an HTML table pasted are tables; at 390 pixels across only the table moves
 * sideways, never the page; and two tabs that add a column and a row to one
 * table at once both end with the same table, filled (the rooms in memory
 * mend it as the server does).
 *
 * `FVOCI_SCREENS=1` also takes the pictures of a table, wide and narrow,
 * light and dark, into `target/screens/table-body/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { documentBoundary, lineBoundary } from './keyboard.support.ts';
import { screensDir } from './screens.ts';
import { hideSidebar, openApp, sidebar, sidebarState } from './shell.support.ts';
import { cellsOf, paste, slash, slashMenu } from './tables.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Planning');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Week');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

/** The row and column of the cell the caret is in; null when it is in none. */
const caretCell = (page: Page): Promise<[number, number] | null> =>
  page.evaluate(() => {
    const node = getSelection()?.anchorNode;
    const cell = (node instanceof Element ? node : node?.parentElement)?.closest('td, th');
    const row = cell?.parentElement;
    if (!cell || !row?.parentElement) return null;
    return [[...row.parentElement.children].indexOf(row), [...row.children].indexOf(cell)];
  });

/**
 * Takes the caret to a cell of the first table with Tab and Shift+Tab, which
 * choose what a cell holds, and to its start (an arrow, when it holds anything).
 */
async function goTo(page: Page, body: Locator, [row, column]: [number, number]): Promise<void> {
  const width = (await cellsOf(body))[0]?.length ?? 0;
  const at = await caretCell(page);
  if (!at) throw new Error('the caret is in no cell');
  const steps = row * width + column - (at[0] * width + at[1]);
  for (let step = 0; step < Math.abs(steps); step += 1) {
    await page.keyboard.press(steps > 0 ? 'Tab' : 'Shift+Tab');
  }
  if (!(await page.evaluate(() => getSelection()?.isCollapsed))) {
    await page.keyboard.press('ArrowLeft');
  }
  expect(await caretCell(page)).toEqual([row, column]);
}

test('a table is made, written in and changed with the keyboard alone', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('/');
  // The kinds of block, then a table, and a file where files can go.
  await expect(slashMenu(page).getByRole('option')).toHaveText([
    'Heading 1',
    'Heading 2',
    'Heading 3',
    'To-do list',
    'Bulleted list',
    'Numbered list',
    'Quote',
    'Code block',
    'Formula block',
    'Divider',
    'Table',
    'File or image',
  ]);
  await page.keyboard.press('Escape');
  await page.keyboard.press('Backspace');
  await slash(page, 'table', 'Table');
  await expect(body.locator('table th')).toHaveCount(3);
  await expect(body.locator('table td')).toHaveCount(6);
  expect(await caretCell(page)).toEqual([0, 0]);

  // Tab goes from cell to cell, and on from the last into a new row; Shift+Tab goes back.
  for (const word of ['Task', 'Who', 'When', 'Plan', 'Ann', 'Mon', 'Ship', 'Bo', 'Fri']) {
    await page.keyboard.type(word);
    await page.keyboard.press('Tab');
  }
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.type('Sat');
  await page.keyboard.press('Tab');
  expect(await caretCell(page)).toEqual([3, 0]);
  expect(await cellsOf(body)).toEqual([
    ['Task', 'Who', 'When'],
    ['Plan', 'Ann', 'Mon'],
    ['Ship', 'Bo', 'Sat'],
    ['', '', ''],
  ]);

  // In a cell, the menu changes the table where the caret is.
  await goTo(page, body, [3, 2]);
  await slash(page, 'column', 'Add a column to the right');
  await goTo(page, body, [3, 3]);
  await slash(page, 'column', 'Delete this column');
  await goTo(page, body, [3, 1]);
  await slash(page, 'row', 'Delete this row');
  expect(await cellsOf(body)).toEqual([
    ['Task', 'Who', 'When'],
    ['Plan', 'Ann', 'Mon'],
    ['Ship', 'Bo', 'Sat'],
  ]);
  await goTo(page, body, [0, 1]);
  await slash(page, 'row', 'Add a row below');
  await goTo(page, body, [0, 2]);
  await slash(page, 'column', 'Add a column to the right');
  expect(await cellsOf(body)).toEqual([
    ['Task', 'Who', 'When', ''],
    ['', '', '', ''],
    ['Plan', 'Ann', 'Mon', ''],
    ['Ship', 'Bo', 'Sat', ''],
  ]);
  // The first row is plain cells, and headers again; then the table goes.
  await goTo(page, body, [0, 0]);
  await slash(page, 'first', 'Make the first row plain cells');
  await expect(body.locator('table th')).toHaveCount(0);
  await goTo(page, body, [0, 0]);
  await slash(page, 'first', 'Make the first row headers');
  await expect(body.locator('table th')).toHaveText(['Task', 'Who', 'When', '']);
  await goTo(page, body, [1, 1]);
  await slash(page, 'table', 'Delete the table');
  await expect(body.locator('table')).toHaveCount(0);
});

test('a Markdown table and an HTML table pasted are tables', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('Fruit');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Letters');
  await body.getByText('Fruit').click();
  await lineBoundary(page, 'end');
  await paste(body, { 'text/plain': '| Name | Count |\n|:-----|------:|\n| apples | **3** |\n' });
  await expect(body.locator('table')).toHaveCount(1);
  await expect(body.locator('table th')).toHaveText(['Name', 'Count']);
  await expect(body.locator('table td strong')).toHaveText('3');
  await body.getByText('Letters').click();
  await lineBoundary(page, 'end');
  await page.keyboard.press('Enter');
  await paste(body, {
    'text/html':
      '<table><tr><th>x</th><th>y</th></tr><tr><td colspan="2">both</td></tr><script>1</script></table>',
    'text/plain': 'x y both',
  });
  await expect(body.locator('table')).toHaveCount(2);
  await expect(body.locator('table').nth(1).locator('td')).toHaveAttribute('colspan', '2');
  await expect(body.locator('script')).toHaveCount(0);
  await expect(page.getByTestId('page-body').locator('[data-unreadable]')).toHaveCount(0);
});

test('at 390 pixels across only the table moves sideways, never the page', async ({
  page,
}, testInfo) => {
  const body = await newPage(page);
  await body.click();
  const head = `| ${Array.from({ length: 8 }, (_, at) => `Column ${String(at + 1)}`).join(' | ')} |`;
  const rule = `|${' --- |'.repeat(8)}`;
  const line = `| ${Array.from({ length: 8 }, (_, at) => `cell ${String(at + 1)}`).join(' | ')} |`;
  await paste(body, { 'text/plain': [head, rule, line, line].join('\n') });
  await expect(body.locator('table th')).toHaveCount(8);
  await page.setViewportSize({ width: 390, height: 844 });
  await hideSidebar(page);
  const frame = body.locator('.tableWrapper');
  await expect
    .poll(() =>
      page.evaluate(() => {
        const root = document.scrollingElement ?? document.documentElement;
        return root.scrollWidth <= root.clientWidth;
      }),
    )
    .toBe(true);
  expect(await frame.evaluate((element) => element.scrollWidth > element.clientWidth)).toBe(true);
  const box = await frame.boundingBox();
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);

  if (process.env.FVOCI_SCREENS !== '1' || testInfo.project.name !== 'desktop') return;
  const dir = screensDir('table-body');
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
    for (const [name, width] of [
      ['narrow', 390],
      ['wide', 1280],
    ] as const) {
      await page.setViewportSize({ width, height: 844 });
      // Narrow again, the list of boards is a drawer again, open over the page: it is closed.
      if (name === 'narrow') {
        await expect.poll(() => sidebarState(page)).not.toBe('fixed');
        await hideSidebar(page);
        await expect
          .poll(async () => {
            const box = await sidebar(page).boundingBox();
            return box ? box.x + box.width : 0;
          })
          .toBeLessThanOrEqual(0);
      }
      await page.screenshot({ path: path.join(dir, `table-${name}-${scheme}.png`) });
    }
  }
});

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
}

/** The address of a card of the board of tasks, by its title. */
async function cardAddress(page: Page, card: string): Promise<string> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async (cardTitle) => {
    const { backend } = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const found = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!found) throw new Error('no board of tasks');
    const views = await backend.views(found.id);
    const wanted = (await backend.cards(found.id)).find((each) => each.title === cardTitle);
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: found.id, viewId: first?.id ?? '', cardId: wanted?.id ?? '' };
  }, card);
  return boardPath(place.boardId, place.viewId, place.cardId);
}

async function cardBody(page: Page): Promise<Locator> {
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

/** Holds what a tab's rooms tell the others until `release`: as if its messages were slow. */
const hold = (page: Page): Promise<void> =>
  page.evaluate(() => {
    const held: (() => void)[] = [];
    const { prototype } = BroadcastChannel;
    const post = Object.getOwnPropertyDescriptor(prototype, 'postMessage')?.value as (
      this: BroadcastChannel,
      message: unknown,
    ) => void;
    prototype.postMessage = function (this: BroadcastChannel, message: unknown) {
      held.push(() => {
        post.call(this, message);
      });
    };
    (window as unknown as { release: () => void }).release = () => {
      prototype.postMessage = post;
      for (const send of held.splice(0)) send();
    };
  });
const release = (page: Page): Promise<void> =>
  page.evaluate(() => {
    (window as unknown as { release: () => void }).release();
  });

test('two tabs that add a column and a row at once both end with the same table, filled', async ({
  page,
}) => {
  const address = await cardAddress(page, 'Design the screens');
  await page.goto(address);
  const mine = await cardBody(page);
  const other = await page.context().newPage();
  await other.goto(address);
  const theirs = await cardBody(other);
  await mine.click();
  await documentBoundary(page, 'end');
  await page.keyboard.press('Enter');
  await paste(mine, { 'text/plain': '| a | b |\n|---|---|\n| c | d |\n' });
  await expect(theirs.locator('table td')).toHaveText(['c', 'd']);

  // While what the first tab's room says is held, each changes the table.
  await hold(page);
  await mine.locator('table th').last().click();
  await lineBoundary(page, 'start');
  await slash(page, 'column', 'Add a column to the right');
  await page.keyboard.press('Tab');
  await page.keyboard.type('x');
  await theirs.locator('table td').first().click();
  await lineBoundary(other, 'start');
  await slash(other, 'row', 'Add a row below');
  await other.keyboard.press('ArrowDown');
  await other.keyboard.type('y');
  await expect
    .poll(() => cellsOf(theirs))
    .toEqual([
      ['a', 'b'],
      ['c', 'd'],
      ['y', ''],
    ]);
  await release(page);

  const merged = [
    ['a', 'b', 'x'],
    ['c', 'd', ''],
    ['y', '', ''],
  ];
  await expect.poll(() => cellsOf(mine)).toEqual(merged);
  await expect.poll(() => cellsOf(theirs)).toEqual(merged);
  for (const each of [page, other]) {
    await expect(each.getByTestId('card-body').locator('[data-unreadable]')).toHaveCount(0);
  }
  await other.close();
});
