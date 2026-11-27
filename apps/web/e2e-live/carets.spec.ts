/**
 * Two people in one body on the real server: each sees where the other is,
 * under the name the server knows them by; a viewer sees the carets and
 * shows none; and a connection cut for five seconds leaves the body
 * read-only, both go on from what they had, and both hold the same once it
 * is back, with their carets.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of the carets, into
 * `target/screens/caret/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Locator,
  type Page,
  type WebSocketRoute,
} from '@playwright/test';

import { scrollTo, uncovered } from '../e2e/carets.support.ts';
import { documentBoundary, lineBoundary } from '../e2e/keyboard.support.ts';
import { screensDir } from '../e2e/screens.ts';

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
const carets = (page: Page): Locator => editor(page).locator('.doc-caret');

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

const ready = (page: Page): Promise<void> =>
  expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');

async function userId(api: APIRequestContext): Promise<string> {
  return ((await (await api.get('/api/v1/users/me')).json()) as Schemas['User']).id;
}

interface Node {
  text?: string;
  content?: Node[];
}

/** The lines of a card's body as the server holds it. */
async function stored(
  api: APIRequestContext,
  workspaceId: string,
  cardId: string,
): Promise<string[]> {
  const card = (await (
    await api.get(`/api/v1/w/${workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const words = (node: Node): string =>
    node.text ?? (node.content ?? []).map((child) => words(child)).join('');
  return ((card.body as Node | null)?.content ?? []).map((block) => words(block));
}

/** The lines of the body as an editor shows them. */
const lines = (page: Page): Promise<string[]> => editor(page).locator('p').allTextContents();

async function picture(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const wide = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.screenshot({ path: path.join(screensDir('caret'), `live-${name}-wide.png`) });
  await page.setViewportSize({ width: 390, height: 844 });
  // The card has taken its frame for this width, and the names their places in it.
  await expect
    .poll(async () => {
      const box = await editor(page).boundingBox();
      const tag = await carets(page).first().locator('.doc-caret-name').boundingBox();
      return box && tag && box.x + box.width <= 391 && tag.x + tag.width <= 391;
    })
    .toBe(true);
  await page.screenshot({ path: path.join(screensDir('caret'), `live-${name}-390.png`) });
  await page.setViewportSize(wide);
}

test('two people see where the other is, by the name the server knows them by; a viewer sees them and shows none', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const [ada, bora, vera] = [person('ada'), person('bora'), person('vera')];
  for (const who of [ada, bora, vera]) await register(base, who);
  const api = await signedIn(base, ada);
  const board = await makeBoard(api, {
    title: 'Carets',
    cards: [{ title: 'Write together', status: 'To do' }],
    members: [
      { who: bora, role: 'editor' },
      { who: vera, role: 'viewer' },
    ],
  });
  const address = boardPath(board, board.viewId, board.cards['Write together'] ?? '');
  const [adaId, boraId] = [await userId(api), await userId(await signedIn(base, bora))];
  const first = await pageOf(browser, address, ada);
  const second = await pageOf(browser, address, bora);
  const third = await pageOf(browser, address, vera);
  const watched = [first, second, third].map((page) => watchErrors(page));
  await ready(first);
  await ready(second);
  await expect(third.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading');

  // Ada writes two lines; Bora goes into the second.
  await editor(first).click();
  await first.keyboard.type('The plan for the week.');
  await first.keyboard.press('Enter');
  await first.keyboard.type('Who books the room?');
  await expect(editor(second).locator('p')).toHaveText([
    'The plan for the week.',
    'Who books the room?',
  ]);
  await editor(second).locator('p').nth(1).click();
  await lineBoundary(second, 'end');

  // Each sees the other, where they are, under the server's word of who they are.
  await expect(carets(first)).toHaveCount(1);
  await expect(carets(first)).toHaveAttribute('data-user', boraId);
  await expect(carets(first).locator('.doc-caret-name')).toHaveAttribute(
    'data-name',
    new RegExp(bora.username),
  );
  await expect(editor(first).locator('p').nth(1).locator('.doc-caret')).toHaveCount(1);
  await expect(carets(second)).toHaveCount(1);
  await expect(carets(second)).toHaveAttribute('data-user', adaId);
  await expect(carets(second).locator('.doc-caret-name')).toHaveAttribute(
    'data-name',
    new RegExp(ada.username),
  );
  // The carets are no part of the text: both hold the same lines.
  expect(await lines(first)).toEqual(await lines(second));

  // The viewer sees both, and shows none though they press on the body.
  await expect(carets(third)).toHaveCount(2);
  await editor(third).click({ force: true });
  await second.keyboard.type(' Bora does.');
  await expect(editor(first).locator('p').nth(1)).toHaveText('Who books the room? Bora does.');
  await expect(carets(first)).toHaveCount(1);
  await expect(carets(second)).toHaveCount(1);
  await expect(carets(first)).toHaveAttribute('data-named', '');
  await picture(first, 'two-people');
  // A selection of Bora's is tinted for the others.
  await lineBoundary(second, 'start', { select: true });
  await expect(editor(first).locator('.doc-caret-selection').first()).toBeVisible();
  await expect(carets(first)).toHaveAttribute('data-named', '');
  await picture(first, 'selection');

  // Bora leaves the card: her caret goes at once, for the writer and the viewer.
  await second.context().close();
  await expect(carets(first)).toHaveCount(0);
  await expect(carets(third)).toHaveCount(1);
  for (const page of [first, third]) await expect(page.locator('[data-problem]')).toHaveCount(0);
  await first.context().close();
  await third.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a connection cut for five seconds: the body is read-only meanwhile, and both hold the same once it is back, also after a reload', async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(120_000);
  const base = baseURL ?? '';
  const [ada, bora] = [person('ada'), person('bora')];
  for (const who of [ada, bora]) await register(base, who);
  const api = await signedIn(base, ada);
  const board = await makeBoard(api, {
    title: 'Tunnel',
    cards: [{ title: 'Travel notes', status: 'To do' }],
    members: [{ who: bora, role: 'editor' }],
  });
  const cardId = board.cards['Travel notes'] ?? '';
  const address = boardPath(board, board.viewId, cardId);
  const [adaId, boraId] = [await userId(api), await userId(await signedIn(base, bora))];
  const first = await pageOf(browser, address, ada);
  const second = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const watched = [first, second].map((page) => watchErrors(page));
  // Bora's sockets go through here: `held`, nothing passes either way though the
  // connection stands (a line gone quiet); `away`, a new one is closed at once.
  let open: { page: WebSocketRoute; server: WebSocketRoute } | undefined;
  let held = false;
  let away = false;
  // What her page says to come into the room, and the binary frames it is sent after each.
  const joins: { said: string; frames: Buffer[] }[] = [];
  await second.routeWebSocket(/\/ws$/, (socket) => {
    if (away) {
      void socket.close();
      return;
    }
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      if (typeof message === 'string' && message.includes('joinDoc')) {
        joins.push({ said: message, frames: [] });
      }
      if (!held) server.send(message);
    });
    server.onMessage((message) => {
      if (typeof message !== 'string') joins.at(-1)?.frames.push(message);
      if (!held) socket.send(message);
    });
    open = { page: socket, server };
  });
  await second.goto(address);
  await signInOnPage(second, bora);
  await ready(first);
  await ready(second);

  await editor(first).click();
  await first.keyboard.type('Shared line: one two three.');
  await first.keyboard.press('Enter');
  await first.keyboard.type('Bora:');
  await expect(editor(second).locator('p')).toHaveText(['Shared line: one two three.', 'Bora:']);
  await editor(second).locator('p').nth(1).click();
  await lineBoundary(second, 'end');
  await second.keyboard.type(' before the tunnel.');
  await expect(editor(first).locator('p').nth(1)).toHaveText('Bora: before the tunnel.');
  await expect(carets(first)).toHaveCount(1);

  // The line goes quiet. What Bora writes and deletes now reaches nobody yet.
  // She deletes a word both hold, and writes on.
  held = true;
  for (const _letter of 'tunnel.') await second.keyboard.press('Backspace');
  await second.keyboard.type('cave. In the tunnel.');
  await expect(editor(second).locator('p')).toHaveText([
    'Shared line: one two three.',
    'Bora: before the cave. In the tunnel.',
  ]);
  // Then it drops, and stays away for five seconds: the body can be read, not written.
  away = true;
  await open?.page.close();
  const body = second.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-room', 'offline');
  await expect(second.getByTestId('body-room-note')).toContainText('The connection is lost');
  await expect(body.locator('[contenteditable="true"]')).toHaveCount(0);
  // Bora's caret is gone for Ada, who goes on writing; Ada's is let go on Bora's page,
  // which is told nothing now.
  await expect(carets(first)).toHaveCount(0);
  await expect(carets(second)).toHaveCount(0);
  await editor(first).locator('p').first().click();
  await lineBoundary(first, 'end');
  await first.keyboard.type(' Ada meanwhile.');
  await second.waitForTimeout(5000);
  // Nothing of the other's reached either.
  await expect(editor(first).locator('p').nth(1)).toHaveText('Bora: before the tunnel.');
  await expect(editor(second).locator('p').first()).toHaveText('Shared line: one two three.');

  // Back: Bora's page joins again by itself, and what both wrote is in both.
  held = false;
  away = false;
  let together = [
    'Shared line: one two three. Ada meanwhile.',
    'Bora: before the cave. In the tunnel.',
  ];
  await expect(body).toHaveAttribute('data-editor', 'ready', { timeout: 40_000 });
  for (const page of [first, second]) {
    await expect(editor(page).locator('p')).toHaveText(together, { timeout: 15_000 });
    await expect(page.getByTestId('body-lost')).toHaveCount(0);
  }
  await expect(second.getByTestId('body-room-note')).toHaveCount(0);
  // She came in the first time holding nothing, and was sent the whole (a sync step 2); the
  // second time she named the document she held, and was sent the server's state vector (a
  // step 1) and then what she lacked. (A frame: room, kind, epoch, kind of sync message.)
  const [firstJoin, again] = [joins[0], joins.at(-1)];
  expect(firstJoin?.said).not.toContain('stateVector');
  expect(firstJoin?.frames[0]?.[3]).toBe(1);
  expect(JSON.parse(again?.said ?? '{}')).toMatchObject({ epoch: 1, stateVector: /^[\w+/]+=*$/ });
  expect(again?.frames.slice(0, 2).map((frame) => frame[3])).toEqual([0, 1]);
  // Each sees the other again: Ada's caret is told to who came in, Bora's once she is in the body.
  await expect(carets(second)).toHaveCount(1);
  await expect(carets(second)).toHaveAttribute('data-user', adaId);
  await editor(second).locator('p').nth(1).click();
  await expect(carets(first)).toHaveCount(1);
  await expect(carets(first)).toHaveAttribute('data-user', boraId);
  await expect(carets(first).locator('.doc-caret-name')).toHaveAttribute(
    'data-name',
    new RegExp(bora.username),
  );
  await lineBoundary(second, 'end');
  await second.keyboard.type(' Out again.');
  together = [together[0] ?? '', `${together[1] ?? ''} Out again.`];
  await expect(editor(first).locator('p')).toHaveText(together);
  await picture(first, 'after-the-tunnel');

  // The card holds it, and both read it again after a reload.
  await expect
    .poll(() => stored(api, board.workspaceId, cardId), { timeout: 15_000 })
    .toEqual(together);
  for (const page of [first, second]) {
    await page.reload();
    await ready(page);
    await expect(editor(page).locator('p')).toHaveText(together);
    await expect(page.locator('[data-problem]')).toHaveCount(0);
  }
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('the browser going offline leaves the body read-only at once, and what was written is there once it is back', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('offline');
  const errors = watchErrors(page);
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Offline',
    cards: [{ title: 'On the train', status: 'To do' }],
  });
  const cardId = board.cards['On the train'] ?? '';
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await ready(page);
  await editor(page).click();
  await page.keyboard.type('Before the tunnel.');
  await expect
    .poll(() => stored(api, board.workspaceId, cardId), { timeout: 15_000 })
    .toEqual(['Before the tunnel.']);

  // The browser says so before the connection does: the page does not wait for it.
  await page.context().setOffline(true);
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-room', 'offline');
  await expect(page.getByTestId('body-room-note')).toContainText('The connection is lost');
  await expect(body.locator('[contenteditable="true"]')).toHaveCount(0);
  await page.context().setOffline(false);
  await expect(body).toHaveAttribute('data-editor', 'ready', { timeout: 30_000 });
  await editor(page).click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' After it.');
  await expect
    .poll(() => stored(api, board.workspaceId, cardId), { timeout: 15_000 })
    .toEqual(['Before the tunnel. After it.']);
  await expect(page.getByTestId('body-lost')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test("a wiki's page shows the carets of who write it, by the name the server knows them by", async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const [ada, bora] = [person('page-ada'), person('page-bora')];
  for (const who of [ada, bora]) await register(base, who);
  const api = await signedIn(base, ada);
  const { workspaceId } = await makeBoard(api, { title: 'Roadmap' });
  const inWorkspace = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${inWorkspace}/wikis`, { data: { title: 'Handbook', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  const boraApi = await signedIn(base, bora);
  const [adaId, boraId] = [await userId(api), await userId(boraApi)];
  await api.post(`${inWorkspace}/containers/${wiki.id}/members`, {
    data: { userId: boraId, role: 'editor' },
  });
  const made = (await (
    await api.post(`${inWorkspace}/wikis/${wiki.id}/pages`, { data: { title: 'Welcome' } })
  ).json()) as Schemas['Page'];
  const address = `/w/${workspaceId}/wikis/${wiki.id}/${made.id}`;
  const [first, second] = await Promise.all(
    [ada, bora].map(async (who) => {
      const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
      await page.goto(address);
      await signInOnPage(page, who);
      await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
      return page;
    }),
  );
  if (!first || !second) throw new Error('two pages');
  const watched = [first, second].map((page) => watchErrors(page));
  const body = (page: Page): Locator =>
    page.getByTestId('page-body').locator('[contenteditable]').first();
  const caretsOn = (page: Page): Locator => body(page).locator('.doc-caret');

  await body(first).click();
  await first.keyboard.type('The handbook.');
  await first.keyboard.press('Enter');
  await first.keyboard.type('Its second line.');
  await expect(body(second).locator('p')).toHaveText(['The handbook.', 'Its second line.']);
  await body(second).locator('p').first().click();
  await lineBoundary(second, 'end');

  for (const [page, whose, who] of [
    [first, boraId, bora.username],
    [second, adaId, ada.username],
  ] as const) {
    await expect(caretsOn(page)).toHaveCount(1);
    await expect(caretsOn(page)).toHaveAttribute('data-user', whose);
    await expect(caretsOn(page).locator('.doc-caret-name')).toHaveAttribute(
      'data-name',
      new RegExp(who),
    );
  }
  await expect(body(first).locator('p').first().locator('.doc-caret')).toHaveCount(1);

  // On a phone the page scrolls under a bar that stays at its top: a name on the line
  // just below the bar is drawn under its line, not under the bar.
  await body(first).locator('p').nth(1).click();
  await lineBoundary(first, 'end');
  for (let line = 3; line <= 70; line += 1) {
    await first.keyboard.press('Enter');
    await first.keyboard.insertText(`Line ${String(line)}.`);
  }
  await expect(body(second).locator('p')).toHaveCount(70);
  await first.setViewportSize({ width: 390, height: 844 });
  expect(await scrollTo(body(first).locator('p').nth(29), 30)).toBe(true);
  await body(second).locator('p').nth(29).click();
  await lineBoundary(second, 'end');
  await expect(body(first).locator('p').nth(29).locator('.doc-caret')).toHaveCount(1);
  await expect(caretsOn(first)).toHaveAttribute('data-named', '');
  await expect.poll(() => uncovered(body(first))).toBe(true);
  if (process.env.FVOCI_SCREENS === '1') {
    await first.screenshot({
      path: path.join(screensDir('caret'), 'live-page-under-the-bar-390.png'),
    });
  }

  // Bora leaves the page: her caret goes with her.
  expect(watched[1]).toEqual([]);
  await second.context().close();
  await expect(caretsOn(first)).toHaveCount(0);
  expect(watched[0]).toEqual([]);
  await first.context().close();
  await Promise.all([api.dispose(), boraApi.dispose()]);
});
