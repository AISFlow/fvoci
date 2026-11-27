/**
 * A card's body on the real server, edited together in its room: written
 * from the editor and read back the same, two people typing at once and
 * both kept, a viewer who reads along, a connection that drops and comes
 * back, and a body written whole by another client of the API meanwhile.
 */
import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Page,
  type WebSocketRoute,
  type WebSocket,
} from '@playwright/test';
import { documentBoundary } from '../e2e/keyboard.support.ts';

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

interface Node {
  type: string;
  text?: string;
  content?: Node[];
}

/** The text of a body, a line per block of the document. */
function lines(body: unknown): string[] {
  const words = (node: Node): string =>
    node.text ?? (node.content ?? []).map((child) => words(child)).join('');
  return ((body as Node | null)?.content ?? []).map((block) => words(block));
}

async function stored(
  api: APIRequestContext,
  board: { workspaceId: string },
  cardId: string,
): Promise<Schemas['Card']> {
  return (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
}

function editor(page: Page) {
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

async function openCard(page: Page, path: string, who: Parameters<typeof signInOnPage>[1]) {
  await page.goto(path);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
}

test('a body written in the editor is the server’s, and opens again as it was', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('writer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Notes',
    cards: [{ title: 'Write the report', status: 'To do' }],
  });
  const cardId = board.cards['Write the report'] ?? '';
  const sockets: WebSocket[] = [];
  page.on('websocket', (socket) => {
    sockets.push(socket);
  });
  const writes: unknown[] = [];
  page.on('request', (request) => {
    if (request.method() === 'PATCH' && request.url().endsWith(`/cards/${cardId}`)) {
      writes.push(request.postDataJSON());
    }
  });

  const errors = watchErrors(page);
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await editor(page).click();
  await page.keyboard.type('First line');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Second line');
  // The server writes the card's body from the room's document.
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['First line', 'Second line']);
  // The body goes through the room alone: the app writes no body over HTTP.
  expect(writes).toEqual([]);
  // The list of cards says there is one, without carrying it.
  const [listed] = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
      params: { perPage: 1000 },
    })
  ).json()) as Schemas['Card'][];
  expect(listed).toMatchObject({ hasBody: true, body: null });

  const saved = (await stored(api, board, cardId)).body;
  // The board opened anew, the card from it.
  await page.goto(boardPath(board, board.viewId));
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  await page.locator('[data-card]').filter({ hasText: 'Write the report' }).click();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await expect(editor(page).locator('p')).toHaveText(['First line', 'Second line']);
  // Opening it is no change: nothing of the document is sent to the room before
  // the card is left (leaving sends what waits first), and the server keeps the
  // same body. Where the reader's caret is is told, which changes nothing: a
  // frame says its kind after its room, 0 for the document's.
  let sent = 0;
  for (const socket of sockets) {
    socket.on('framesent', (frame) => {
      if (typeof frame.payload !== 'string' && frame.payload[1] === 0) sent += 1;
    });
  }
  await editor(page).click();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  expect(sent).toBe(0);
  expect((await stored(api, board, cardId)).body).toEqual(saved);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

/** A page of its own for somebody, on a card of the board, signed in. */
async function pageOf(
  browser: Browser,
  path: string,
  who: Parameters<typeof signInOnPage>[1],
): Promise<Page> {
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  await page.goto(path);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  return page;
}

test('two people type in one body at once: each sees the other’s, and both stay after a reload', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('ada');
  const colleague = person('bora');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Together',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  const path = boardPath(board, board.viewId, cardId);
  const first = await pageOf(browser, path, owner);
  const second = await pageOf(browser, path, colleague);
  // Read at the end: errors that come later are in them too.
  const watched = [watchErrors(first), watchErrors(second)];
  for (const page of [first, second]) {
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  }

  // Both type at once, each in a line of their own.
  await editor(first).click();
  await editor(second).click();
  await Promise.all([
    first.keyboard.type('Ada writes the plan', { delay: 15 }),
    (async () => {
      await second.keyboard.press('Enter');
      await second.keyboard.type('Bora books the room', { delay: 15 });
    })(),
  ]);
  for (const page of [first, second]) {
    await expect(editor(page)).toContainText('Ada writes the plan');
    await expect(editor(page)).toContainText('Bora books the room');
  }
  expect(await editor(first).textContent()).toBe(await editor(second).textContent());
  const together = await editor(first).locator('p').allTextContents();

  // Written to the card, and there after a reload.
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(together);
  await second.reload();
  await expect(second.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await expect(editor(second).locator('p')).toHaveText(together);
  for (const page of [first, second]) await expect(page.locator('[data-problem]')).toHaveCount(0);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a viewer reads along and cannot write', async ({ browser, baseURL }) => {
  const base = baseURL ?? '';
  const owner = person('editor');
  const reader = person('viewer');
  await register(base, owner);
  await register(base, reader);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Read along',
    cards: [{ title: 'Minutes', status: 'To do' }],
    members: [{ who: reader, role: 'viewer' }],
  });
  const cardId = board.cards.Minutes ?? '';
  const path = boardPath(board, board.viewId, cardId);
  const writer = await pageOf(browser, path, owner);
  const looker = await pageOf(browser, path, reader);
  // Read at the end: errors that come later are in them too.
  const watched = [watchErrors(writer), watchErrors(looker)];
  await expect(looker.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading');
  await expect(looker.getByTestId('body-room-note')).toHaveText(
    'You can read this description but not change it.',
  );
  await expect(looker.getByTestId('card-body').locator('[contenteditable="true"]')).toHaveCount(0);

  await expect(writer.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await editor(writer).click();
  await writer.keyboard.type('Decided: ship on Monday');
  await expect(editor(looker)).toHaveText('Decided: ship on Monday');
  // Typing on the viewer's side changes nothing.
  await editor(looker).click({ force: true });
  await looker.keyboard.type('nope');
  await expect(editor(looker)).toHaveText('Decided: ship on Monday');
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['Decided: ship on Monday']);
  await writer.context().close();
  await looker.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a connection that drops leaves the body read-only, and once back the reader goes on writing', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('dropper');
  const errors = watchErrors(page);
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Dropped',
    cards: [{ title: 'Travel notes', status: 'To do' }],
  });
  const cardId = board.cards['Travel notes'] ?? '';
  // The page's sockets go through here, so that the test can cut them.
  const sockets: WebSocketRoute[] = [];
  await page.routeWebSocket(/\/ws$/, (socket) => {
    socket.connectToServer();
    sockets.push(socket);
  });
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await editor(page).click();
  await page.keyboard.type('Before the tunnel.');
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['Before the tunnel.']);

  const count = sockets.length;
  await sockets.at(-1)?.close();
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-room', 'offline');
  await expect(page.getByTestId('body-room-note')).toContainText('The connection is lost');
  await expect(body.locator('[contenteditable="true"]')).toHaveCount(0);
  await expect(editor(page)).toHaveText('Before the tunnel.');

  // The page opens another connection by itself and joins the room again.
  await expect.poll(() => sockets.length).toBeGreaterThan(count);
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(page.getByTestId('body-room-note')).toHaveCount(0);
  await editor(page).click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(' After it.');
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['Before the tunnel. After it.']);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a body written whole by another client of the API takes the place of the one in the editor', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('replacer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Replaced',
    cards: [{ title: 'Pack the boxes', status: 'To do' }],
  });
  const cardId = board.cards['Pack the boxes'] ?? '';
  const errors = watchErrors(page);
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await editor(page).click();
  await page.keyboard.type('My plan');
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['My plan']);

  const readAt = (await stored(api, board, cardId)).updatedAt;
  const written = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
    data: {
      body: {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Their plan' }] }],
      },
      readAt,
    },
  });
  expect(written.ok()).toBe(true);
  await expect(editor(page).locator('p')).toHaveText(['Their plan']);
  // Nothing of the reader's was waiting: nothing is said lost.
  await expect(page.getByTestId('body-lost')).toHaveCount(0);
  await editor(page).click();
  await documentBoundary(page, 'end');
  await page.keyboard.type(', and mine');
  await expect
    .poll(async () => lines((await stored(api, board, cardId)).body), { timeout: 15_000 })
    .toEqual(['Their plan, and mine']);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a card duplicated right after typing copies what was typed', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);
  const owner = person('copier');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Copies',
    cards: [{ title: 'Original', status: 'To do' }],
  });
  const cardId = board.cards.Original ?? '';
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await editor(page).click();
  await page.keyboard.type('Typed just now');
  // Every change acknowledged by the room ("Saved" comes 0.6 s after the last answer), and
  // the copy asked for at once: the room writes the card 2 s after the last change.
  await expect(page.getByTestId('card-save-state')).toHaveAttribute('data-state', 'saved');
  const copied = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks/${cardId}/duplicate`,
  );
  expect(copied.ok()).toBe(true);
  const [copy] = (await copied.json()) as Schemas['Card'][];
  await api.dispose();
  expect(JSON.stringify(copy?.body ?? null)).toContain('Typed just now');
  expect(errors).toEqual([]);
});

test('a board copied right after typing copies what was typed', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);
  const owner = person('boardcopier');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Copied whole',
    cards: [{ title: 'Original', status: 'To do' }],
  });
  const cardId = board.cards.Original ?? '';
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await editor(page).click();
  await page.keyboard.type('Typed just now');
  await expect(page.getByTestId('card-save-state')).toHaveAttribute('data-state', 'saved');
  const copied = await api.post(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/duplicate`,
  );
  expect(copied.ok()).toBe(true);
  const copyId = ((await copied.json()) as Schemas['ContainersAndBlocks']).containers[0]?.id ?? '';
  const blocks = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${copyId}/blocks`)
  ).json()) as Schemas['Block'][];
  const card = blocks.find((block) => block.type === 'card' && block.title === 'Original');
  await api.dispose();
  expect(JSON.stringify(card?.body ?? null)).toContain('Typed just now');
  expect(errors).toEqual([]);
});
