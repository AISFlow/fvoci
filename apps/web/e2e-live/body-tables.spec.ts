/**
 * A table in a body, two people on the real server: what each changes at the
 * same moment (one adds a column, the other a row, before either hears of
 * the other's) the server merges and mends, so both pages and the stored
 * body hold one table that fills its grid. Both turn its first row into
 * plain cells at once: a cell turned is a cell replaced, so the row holds the
 * cells of both, and the server pads the other rows to its width.
 */
import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Locator,
  type Page,
  type WebSocketRoute,
} from '@playwright/test';

import { lineBoundary } from '../e2e/keyboard.support.ts';
import { cellsOf, paste, slash } from '../e2e/tables.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const editor = (page: Page): Locator =>
  page.getByTestId('card-body').locator('[contenteditable]').first();

const ready = (page: Page): Promise<void> =>
  expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');

interface Node {
  type: string;
  text?: string;
  content?: Node[];
}

/** The cells of the first table of a card's body as the server holds it, and their kinds. */
async function stored(
  api: APIRequestContext,
  workspaceId: string,
  cardId: string,
): Promise<{ cells: string[][]; kinds: string[][] }> {
  const card = (await (
    await api.get(`/api/v1/w/${workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const words = (node: Node): string =>
    node.text ?? (node.content ?? []).map((child) => words(child)).join('');
  const table = (card.body as Node | null)?.content?.find((block) => block.type === 'table');
  const rows = table?.content ?? [];
  return {
    cells: rows.map((row) => (row.content ?? []).map((cell) => words(cell))),
    kinds: rows.map((row) => (row.content ?? []).map((cell) => cell.type)),
  };
}

/** The kinds of the cells of the first table on a page. */
const kindsOf = (page: Page): Promise<string[][]> =>
  editor(page)
    .locator('table')
    .first()
    .evaluate((table) =>
      [...table.querySelectorAll('tr')].map((row) =>
        [...row.querySelectorAll('th, td')].map((cell) =>
          cell.tagName === 'TH' ? 'tableHeader' : 'tableCell',
        ),
      ),
    );

/**
 * The second person's page, whose socket goes through the test: while `held`,
 * what passes either way waits, in order, until `release` sends it on.
 */
async function heldPage(
  browser: Browser,
  address: string,
  who: Parameters<typeof signInOnPage>[1],
): Promise<{ page: Page; hold: () => void; release: () => void }> {
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  let held = false;
  const waiting: (() => void)[] = [];
  const pass = (send: () => void): void => {
    if (held) waiting.push(send);
    else send();
  };
  await page.routeWebSocket(/\/ws$/, (socket: WebSocketRoute) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      pass(() => {
        server.send(message);
      });
    });
    server.onMessage((message) => {
      pass(() => {
        socket.send(message);
      });
    });
  });
  await page.goto(address);
  await signInOnPage(page, who);
  return {
    page,
    hold: () => {
      held = true;
    },
    release: () => {
      held = false;
      for (const send of waiting.splice(0)) send();
    },
  };
}

test('two people change one table at once: both pages and the stored body hold one table, filled', async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(120_000);
  const base = baseURL ?? '';
  const [ada, bora] = [person('ada'), person('bora')];
  for (const who of [ada, bora]) await register(base, who);
  const api = await signedIn(base, ada);
  const board = await makeBoard(api, {
    title: 'Tables',
    cards: [{ title: 'Rota', status: 'To do' }],
    members: [{ who: bora, role: 'editor' }],
  });
  const cardId = board.cards.Rota ?? '';
  const address = boardPath(board, board.viewId, cardId);
  const first = await (await browser.newContext({ locale: 'en-US' })).newPage();
  await first.goto(address);
  await signInOnPage(first, ada);
  const { page: second, hold, release } = await heldPage(browser, address, bora);
  const watched = [first, second].map((page) => watchErrors(page));
  await ready(first);
  await ready(second);

  await editor(first).click();
  await first.keyboard.type('Who is on when.');
  await first.keyboard.press('Enter');
  await paste(editor(first), { 'text/plain': '| a | b |\n|---|---|\n| c | d |\n' });
  await expect(editor(second).locator('table td')).toHaveText(['c', 'd']);

  // Neither hears the other: Ada adds a column, Bora a row, and each writes in it.
  hold();
  await editor(first).locator('table th').last().click();
  await lineBoundary(first, 'start');
  await slash(first, 'column', 'Add a column to the right');
  await first.keyboard.press('Tab');
  await first.keyboard.type('x');
  await editor(second).locator('table td').first().click();
  await lineBoundary(second, 'start');
  await slash(second, 'row', 'Add a row below');
  await second.keyboard.press('ArrowDown');
  await second.keyboard.type('y');
  expect(await cellsOf(editor(first))).toEqual([
    ['a', 'b', 'x'],
    ['c', 'd', ''],
  ]);
  expect(await cellsOf(editor(second))).toEqual([
    ['a', 'b'],
    ['c', 'd'],
    ['y', ''],
  ]);
  release();

  // Bora's row was two cells wide: the server gives it the third.
  const merged = [
    ['a', 'b', 'x'],
    ['c', 'd', ''],
    ['y', '', ''],
  ];
  for (const page of [first, second]) {
    await expect.poll(() => cellsOf(editor(page)), { timeout: 15_000 }).toEqual(merged);
  }
  await expect
    .poll(async () => (await stored(api, board.workspaceId, cardId)).cells, { timeout: 30_000 })
    .toEqual(merged);

  // Both turn the first row into plain cells at once.
  hold();
  for (const page of [first, second]) {
    await editor(page).locator('table th').first().click();
    await lineBoundary(page, 'start');
    await slash(page, 'first', 'Make the first row plain cells');
    await expect(editor(page).locator('table th')).toHaveCount(0);
  }
  release();
  const plain = Array.from({ length: 6 }, () => 'tableCell');
  const doubled = {
    cells: [
      ['a', 'b', 'x', 'a', 'b', 'x'],
      ['c', 'd', '', '', '', ''],
      ['y', '', '', '', '', ''],
    ],
    kinds: [plain, plain, plain],
  };
  for (const page of [first, second]) {
    await expect
      .poll(async () => ({ cells: await cellsOf(editor(page)), kinds: await kindsOf(page) }), {
        timeout: 15_000,
      })
      .toEqual(doubled);
  }
  await expect
    .poll(() => stored(api, board.workspaceId, cardId), { timeout: 30_000 })
    .toEqual(doubled);

  // Neither page lost anything, nor holds what it cannot read; both go on writing.
  for (const page of [first, second]) {
    await expect(page.getByTestId('body-lost')).toHaveCount(0);
    await expect(page.getByTestId('card-body').locator('[data-unreadable]')).toHaveCount(0);
  }
  await editor(second).locator('table td').last().click();
  await second.keyboard.type('!');
  await expect(editor(first).locator('table td').last()).toHaveText('!');
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});
