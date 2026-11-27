/**
 * Pictures of a card's body in its room on the real server, wide and 390 px
 * wide, into `target/screens/collab/`: before the room's document has come,
 * a viewer reading along, the connection away, not let into the room, and
 * what was typed dropped with a document made anew. The page's socket goes
 * through the test (`routeWebSocket`), which holds or answers what it needs
 * to show each state.
 */
import path from 'node:path';

import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import { documentBoundary } from '../e2e/keyboard.support.ts';

import { screensDir } from '../e2e/screens.ts';

import { boardPath, makeBoard, person, register, signedIn, signInOnPage } from './support.ts';

test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');
test.use({ locale: 'en-US' });

const SCREENS = screensDir('collab');
const SIZES = [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'narrow', width: 390, height: 844 },
] as const;

/** What the route does with the page's socket. */
type Mode = 'pass' | 'no-join' | 'refuse' | 'hold-frames' | 'cut';

async function route(page: Page): Promise<{ mode: Mode; sockets: WebSocketRoute[] }> {
  const state: { mode: Mode; sockets: WebSocketRoute[] } = { mode: 'pass', sockets: [] };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    if (state.mode === 'cut') {
      void socket.close();
      return;
    }
    const server = socket.connectToServer();
    state.sockets.push(socket);
    socket.onMessage((message) => {
      if (typeof message !== 'string') {
        // What the reader typed does not reach the server.
        if (state.mode !== 'hold-frames') server.send(message);
        return;
      }
      const command = JSON.parse(message) as {
        type?: string;
        workspaceId?: string;
        cardId?: string;
      };
      if (command.type === 'joinDoc' && state.mode === 'refuse') {
        socket.send(
          JSON.stringify({
            type: 'docRefused',
            workspaceId: command.workspaceId,
            cardId: command.cardId,
            reason: 'document_unavailable',
          }),
        );
        return;
      }
      server.send(message);
    });
    server.onMessage((message) => {
      if (
        typeof message === 'string' &&
        state.mode === 'no-join' &&
        message.includes('"docJoined"')
      ) {
        return;
      }
      socket.send(message);
    });
  });
  return state;
}

test('the body in its room, in each state the reader can see', async ({ browser, baseURL }) => {
  test.setTimeout(240_000);
  const base = baseURL ?? '';
  const owner = person('pictures');
  const reader = person('looker');
  await register(base, owner);
  await register(base, reader);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Launch notes',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
    members: [{ who: reader, role: 'viewer' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  const cardPath = boardPath(board, board.viewId, cardId);
  const body = {
    type: 'doc',
    content: [
      { type: 'paragraph', content: [{ type: 'text', text: 'Ship on Monday, after the review.' }] },
      { type: 'paragraph', content: [{ type: 'text', text: 'Bora books the room.' }] },
    ],
  };
  const readAt = async (): Promise<number> =>
    (
      (await (await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)).json()) as {
        updatedAt: number;
      }
    ).updatedAt;
  const written = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
    data: { body, readAt: await readAt() },
  });
  expect(written.ok()).toBe(true);

  for (const size of SIZES) {
    const open = async (who: typeof owner, mode: Mode = 'pass') => {
      const context = await browser.newContext({
        locale: 'en-US',
        viewport: { width: size.width, height: size.height },
        reducedMotion: 'reduce',
      });
      const page = await context.newPage();
      const socket = await route(page);
      socket.mode = mode;
      await page.goto(cardPath);
      await signInOnPage(page, who);
      await expect(page.getByTestId('card-dialog')).toBeVisible();
      return { page, socket };
    };
    const shot = async (page: Page, name: string): Promise<void> => {
      await page.getByTestId('card-body').scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(SCREENS, `${name}-${size.name}.png`) });
    };

    // Before the room's document: the body as the card was read, and after a second the line.
    {
      const { page } = await open(owner, 'no-join');
      await expect(page.getByTestId('body-room-note')).toBeVisible();
      await expect(page.getByTestId('body-room-note')).toContainText('Opening the description');
      await shot(page, 'joining');
      await page.context().close();
    }
    // A viewer reads along.
    {
      const { page } = await open(reader);
      await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading');
      await shot(page, 'viewer');
      await page.context().close();
    }
    // The connection away: read only until it is back.
    {
      const { page, socket } = await open(owner);
      await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
      socket.mode = 'cut';
      await socket.sockets.at(-1)?.close();
      await expect(page.getByTestId('card-body')).toHaveAttribute('data-room', 'offline');
      await shot(page, 'offline');
      await page.context().close();
    }
    // Not let into the room: tried again by itself, and by hand.
    {
      const { page } = await open(owner, 'refuse');
      await expect(page.getByTestId('card-body')).toHaveAttribute('data-room', 'out');
      await shot(page, 'not-let-in');
      await page.context().close();
    }
    // What the reader typed was dropped with a document made anew.
    {
      const { page, socket } = await open(owner);
      await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
      socket.mode = 'hold-frames';
      await page.getByTestId('card-body').locator('[contenteditable]').first().click();
      await documentBoundary(page, 'end');
      await page.keyboard.type(' And the slides.');
      // Another client writes the body whole, something else than the server has.
      const other = {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [{ type: 'text', text: `Moved to Tuesday (${size.name}).` }],
          },
        ],
      };
      const again = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
        data: { body: other, readAt: await readAt() },
      });
      expect(again.ok()).toBe(true);
      await expect(page.getByTestId('body-lost')).toBeVisible();
      await shot(page, 'lost');
      await page.context().close();
    }
  }
  await api.dispose();
});
