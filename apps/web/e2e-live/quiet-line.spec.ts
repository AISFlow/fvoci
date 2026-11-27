/**
 * A connection that dies without a word, on the real server: the line stands
 * but nothing comes back (the test passes nothing either way, as a network
 * gone quiet does). The page's `ping` is not answered, so it lets the
 * connection go: the body is read-only, it connects again by itself, and
 * what the other wrote meanwhile is there (`contracts/websocket.md` 3).
 */
import { expect, test, type Browser, type Locator, type Page } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const editor = (page: Page): Locator =>
  page.getByTestId('card-body').locator('[contenteditable]').first();

const ready = (page: Page): Promise<void> =>
  expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');

async function pageOf(
  browser: Browser,
  address: string,
  who: Parameters<typeof signInOnPage>[1],
): Promise<Page> {
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  await page.goto(address);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  return page;
}

test('a connection that goes quiet is let go after its PING is not answered, and comes back with what was written meanwhile', async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(150_000);
  const base = baseURL ?? '';
  const [ada, bora] = [person('ada'), person('bora')];
  for (const who of [ada, bora]) await register(base, who);
  const api = await signedIn(base, ada);
  const board = await makeBoard(api, {
    title: 'Quiet line',
    cards: [{ title: 'Field notes', status: 'To do' }],
    members: [{ who: bora, role: 'editor' }],
  });
  const address = boardPath(board, board.viewId, board.cards['Field notes'] ?? '');
  const first = await pageOf(browser, address, ada);
  const second = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const watched = [first, second].map((page) => watchErrors(page));
  // Bora's sockets go through here. The first one goes quiet when told: nothing
  // passes either way though the connection stands. While `away`, a new one is
  // closed at once; after that they pass all.
  let quiet = false;
  let away = false;
  let sockets = 0;
  let letGo: number | undefined;
  const pings: number[] = [];
  await second.routeWebSocket(/\/ws$/, (socket) => {
    if (away) {
      void socket.close();
      return;
    }
    sockets += 1;
    const deaf = sockets === 1;
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      if (typeof message === 'string' && message.includes('"ping"')) pings.push(Date.now());
      if (!(deaf && quiet)) server.send(message);
    });
    server.onMessage((message) => {
      if (!(deaf && quiet)) socket.send(message);
    });
    socket.onClose((code, reason) => {
      if (deaf) letGo = Date.now();
      void server.close({ code, reason });
    });
  });
  await second.goto(address);
  await signInOnPage(second, bora);
  await ready(first);
  await ready(second);
  await editor(first).click();
  await first.keyboard.type('Before the quiet.');
  await expect(editor(second).locator('p')).toHaveText(['Before the quiet.']);

  // The line goes quiet: nothing says so to Bora's page, and Ada writes on.
  quiet = true;
  away = true;
  const wentQuiet = Date.now();
  await first.keyboard.press('Enter');
  await first.keyboard.type('Ada while it was quiet.');
  // Her next PING (twenty seconds after the last answer at most) waits fifteen for
  // nothing: the page closes the connection, and opens another.
  // Longer than the bound below, so that the bound is what a slow page fails on.
  await expect.poll(() => letGo, { timeout: 70_000 }).toBeDefined();
  const noticed = (letGo ?? 0) - wentQuiet;
  expect(noticed).toBeGreaterThan(14_000);
  // At most: twenty seconds to the PING, fifteen for it, once more if its wait ran
  // late on a busy machine, and some time for that machine.
  expect(noticed).toBeLessThan(65_000);
  // Asked once (once more only if the page's timer ran late), and not answered.
  expect(pings.filter((at) => at >= wentQuiet && at <= (letGo ?? 0)).length).toBeGreaterThan(0);
  // Let go, the body can be read and not written, until the line is back.
  const body = second.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-room', 'offline');
  await expect(body.locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(second.getByTestId('body-room-note')).toContainText('The connection is lost');
  away = false;
  await expect.poll(() => sockets, { timeout: 30_000 }).toBe(2);
  await expect(body).toHaveAttribute('data-editor', 'ready', { timeout: 30_000 });
  const together = ['Before the quiet.', 'Ada while it was quiet.'];
  await expect(editor(second).locator('p')).toHaveText(together, { timeout: 15_000 });

  // And it carries again, both ways.
  await editor(second).locator('p').nth(1).click();
  await second.keyboard.press('End');
  await second.keyboard.type(' Bora is back.');
  await expect(editor(first).locator('p')).toHaveText([
    'Before the quiet.',
    'Ada while it was quiet. Bora is back.',
  ]);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});
