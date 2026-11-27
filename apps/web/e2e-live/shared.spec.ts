/**
 * A board read by its public link on the real server: a browser that is
 * signed in as nobody opens the link, reads the board, switches views, opens
 * a card and reads its body and comments, and announces an anonymous visit.
 * Somebody signed in reads it the same way. A wrong token, and the link
 * turned off, show "not found". A wiki is read by its link the same way: its
 * tree and its pages, naming nobody; the organization's public links turned
 * off, or its own, it is not found. Pictures, wide and at 390 pixels, go to
 * `target/screens/shared/` (a wiki's, in Korean, to `target/screens/wiki-people/`)
 * when `FVOCI_SCREENS` is set.
 */
import { expect, test, type Page } from '@playwright/test';

import { screensDir } from '../e2e/screens.ts';

import {
  ADMIN,
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

/** The address of the board's public link, a view and a card on it. */
function sharedPath(boardId: string, token: string, viewId?: string, cardId?: string): string {
  const parts = [boardId, viewId, cardId].filter((part) => part !== undefined);
  return `/shared/${parts.map(encodeURIComponent).join('/')}?r=${encodeURIComponent(token)}`;
}

/**
 * What the page asks of the server that a reader by the link must not: a
 * request to the API that is no reading, or not of the board under
 * `/api/v1/shared`, or that carries a cookie; or any socket subscription/roster.
 */
function strayRequestsOf(page: Page, boardId: string): string[] {
  const stray: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith('/api/')) return;
    const said = `${request.method()} ${url.pathname}`;
    if (request.method() !== 'GET' || !url.pathname.startsWith(`/api/v1/shared/${boardId}`)) {
      stray.push(said);
    }
    void request.allHeaders().then((headers) => {
      if (headers.cookie !== undefined) stray.push(`${said} with a cookie`);
    });
  });
  page.on('websocket', (socket) => {
    socket.on('framesent', ({ payload }) => {
      const command = JSON.parse(String(payload)) as Record<string, unknown>;
      if (
        command.type !== 'setPresence' ||
        !command.readToken ||
        (command.containerId !== '' && command.containerId !== boardId) ||
        command.workspaceId !== ''
      ) {
        stray.push(`unexpected socket command ${String(command.type)}`);
      }
    });
    socket.on('framereceived', () => stray.push('a public reader received a socket message'));
  });
  return stray;
}

test('a board is read by its public link without signing in, and not when the link is off', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('sharer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Allotment plan',
    cards: [
      { title: 'Sow the beans', status: 'To do', note: 'after the frost' },
      { title: 'Mend the fence', status: 'Doing' },
      { title: 'Order compost', status: 'Done' },
    ],
  });
  const at = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const cardId = board.cards['Sow the beans'] ?? '';
  const card = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const paragraph = (text: string) => ({ type: 'paragraph', content: [{ type: 'text', text }] });
  const body = { type: 'doc', content: [paragraph('Two rows, a hand apart.')] };
  expect(
    (
      await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
        data: { readAt: card.updatedAt, body },
      })
    ).ok(),
  ).toBe(true);
  const commented = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`,
    {
      data: [
        {
          type: 'comment',
          parentId: cardId,
          containerId: board.boardId,
          title: '',
          body: { type: 'doc', content: [paragraph('The broad beans did well last year.')] },
          createdAt: Date.now(),
          updatedAt: Date.now(),
        },
      ],
    },
  );
  expect(commented.ok()).toBe(true);
  const table = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
    data: [
      {
        type: 'view',
        containerId: board.boardId,
        title: 'Table of it all',
        fields: { viewType: 'table', shownProperties: ['status', 'note'] },
        createdAt: Date.now(),
        updatedAt: Date.now(),
      },
    ],
  });
  expect(table.ok()).toBe(true);
  const shared = (await (
    await api.post(`${at}/public-link`, { data: { enabled: true } })
  ).json()) as Schemas['PublicLink'];
  const token = shared.token ?? '';
  expect(token).not.toBe('');

  // Signed in as nobody.
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const writes = strayRequestsOf(page, board.boardId);
  await page.goto(sharedPath(board.boardId, token));
  const reader = page.getByTestId('shared-board');
  await expect(reader.getByTestId('board-page')).toBeVisible();
  await expect(reader.getByText('Sow the beans')).toBeVisible();
  await expect(reader.getByText('Mend the fence')).toBeVisible();
  await expect(reader.getByTestId('shared-title')).toHaveText('Allotment plan');
  // The board alone: no sidebar, no settings, no sharing, no way to add a card.
  await expect(page.getByTestId('sidebar')).toHaveCount(0);
  await expect(page.getByTestId('share-board')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /settings/i })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^new$/i })).toHaveCount(0);
  // The first view, in the address of the link.
  await expect(page).toHaveURL(new RegExp(`/shared/${board.boardId}/${board.viewId}\\?r=`));

  const shoot = async (name: string): Promise<void> => {
    if (process.env.FVOCI_SCREENS) {
      await page.screenshot({
        path: `${screensDir('shared')}/shared-${name}.png`,
        animations: 'disabled',
      });
    }
  };
  await page.setViewportSize({ width: 1280, height: 800 });
  await shoot('board-wide');

  // Another view, from the list behind the button of the views: the views and nothing that
  // changes them.
  await reader.getByTestId('view-header').locator('[data-tool="views"]').click();
  const list = page.getByRole('menu', { name: 'Views' });
  await expect(list.locator('a[role="menuitem"]')).toHaveText([/.+/, 'Table of it all']);
  await expect(list.locator('[role="menuitem"]:not(a)')).toHaveCount(0);
  await expect(reader.locator('[data-tool="rename"], [data-tool="more"]')).toHaveCount(0);
  await list.getByRole('menuitem', { name: 'Table of it all' }).click();
  await expect(reader.getByTestId('view-header').locator('[data-tool="views"]')).toHaveText(
    'Table of it all',
  );
  await expect(page).toHaveURL(/\/shared\/[^/]+\/[^/?]+\?r=/);
  await expect(reader.getByText('after the frost')).toBeVisible();
  await shoot('table-wide');
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`/shared/${board.boardId}/${board.viewId}\\?r=`));

  // A card: its body and its comment, nobody named, no way to write.
  await reader.getByText('Sow the beans').click();
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('Two rows, a hand apart.')).toBeVisible();
  const comments = dialog.getByTestId('card-comments');
  await expect(comments.getByText('The broad beans did well last year.')).toBeVisible();
  await expect(comments.getByText(owner.username)).toHaveCount(0);
  await expect(dialog.getByRole('textbox')).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`/${cardId}\\?r=`));
  await shoot('card-wide');

  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  // At 390 pixels, from the card's own address, as from the board's.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(sharedPath(board.boardId, token, board.viewId, cardId));
  await expect(dialog.getByText('Two rows, a hand apart.')).toBeVisible();
  await shoot('card-390');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(reader.getByText('Sow the beans')).toBeVisible();
  await shoot('board-390');
  expect(writes).toEqual([]);

  // Somebody signed in, its admin here, reads it the same way, as a reader: no
  // cookie goes along, and nothing of the admin's shows.
  const ownerContext = await browser.newContext({ locale: 'en-US', baseURL: base });
  const ownerPage = await ownerContext.newPage();
  const ownerErrors = watchErrors(ownerPage);
  await ownerPage.goto(boardPath(board, board.viewId));
  await signInOnPage(ownerPage, owner);
  await expect(ownerPage.getByTestId('share-board')).toBeVisible();
  const ownerReader = await ownerContext.newPage();
  const readerErrors = watchErrors(ownerReader);
  const ownerStray = strayRequestsOf(ownerReader, board.boardId);
  await ownerReader.goto(sharedPath(board.boardId, token));
  await expect(ownerReader.getByTestId('shared-board').getByText('Sow the beans')).toBeVisible();
  await expect(ownerReader.getByTestId('sidebar')).toHaveCount(0);
  await expect(ownerReader.getByTestId('share-board')).toHaveCount(0);
  await expect(ownerReader.getByRole('button', { name: /^new$/i })).toHaveCount(0);
  await ownerReader.getByTestId('shared-board').getByText('Sow the beans').click();
  const ownerDialog = ownerReader.getByTestId('card-dialog');
  await expect(ownerDialog.getByText('Two rows, a hand apart.')).toBeVisible();
  await expect(ownerDialog.getByRole('textbox')).toHaveCount(0);
  await expect(ownerDialog.getByTestId('card-comments').getByText(owner.username)).toHaveCount(0);
  expect(ownerStray).toEqual([]);

  // A wrong token: not found.
  await page.goto(sharedPath(board.boardId, `${token}x`));
  await expect(page.getByTestId('not-found')).toBeVisible();
  await page.setViewportSize({ width: 1280, height: 800 });
  await shoot('not-found-wide');

  // The link turned off: not found, at the address that worked.
  expect((await api.post(`${at}/public-link`, { data: { enabled: false } })).ok()).toBe(true);
  await page.goto(sharedPath(board.boardId, token));
  await expect(page.getByTestId('not-found')).toBeVisible();
  await expect(page.getByText('Sow the beans')).toHaveCount(0);
  expect(writes).toEqual([]);
  // Its way home is the app's: the sign-in page, the page loaded anew.
  await page.getByRole('link', { name: 'Back to home' }).click();
  await expect(page.getByRole('button', { name: /^sign in$/i })).toBeVisible();

  await api.dispose();
  await context.close();
  await ownerContext.close();
  expect([...errors, ...ownerErrors, ...readerErrors]).toEqual([]);
});

test('a wiki is read by its public link without signing in: its tree and pages, naming nobody', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-publisher');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Beside the wiki' });
  const at = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${at}/wikis`, { data: { title: 'Field notes', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  const pages = `${at}/wikis/${wiki.id}/pages`;
  const made = async (title: string, parentId?: string) =>
    (await (
      await api.post(pages, { data: { title, ...(parentId ? { parentId } : {}) } })
    ).json()) as Schemas['Page'];
  const planting = await made('Planting');
  const harvest = await made('Harvest', planting.id);
  // Enough pages after them that the tree is longer than a phone's screen.
  const notes = Array.from({ length: 30 }, (_, at) => `Note ${String(at + 1).padStart(2, '0')}`);
  for (const title of notes) await made(title);
  const ownerId = ((await (await api.get('/api/v1/users/me')).json()) as Schemas['User']).id;
  const body = {
    type: 'doc',
    content: [
      {
        type: 'paragraph',
        content: [
          { type: 'text', text: 'Sow in rows, then see ' },
          { type: 'reference', attrs: { id: harvest.id } },
          { type: 'text', text: ' and ask ' },
          { type: 'mention', attrs: { userId: ownerId } },
        ],
      },
    ],
  };
  expect(
    (
      await api.patch(`${pages}/${planting.id}`, { data: { readAt: planting.updatedAt, body } })
    ).ok(),
  ).toBe(true);
  const shared = (await (
    await api.post(`${at}/containers/${wiki.id}/public-link`, { data: { enabled: true } })
  ).json()) as Schemas['PublicLink'];
  const token = shared.token ?? '';

  // Signed in as nobody: the tree and the first page, in the address of the link.
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const stray = strayRequestsOf(page, wiki.id);
  await page.goto(sharedPath(wiki.id, token));
  const reader = page.getByTestId('shared-wiki');
  await expect(reader.getByTestId('page-title')).toHaveText('Planting');
  await expect(page).toHaveURL(new RegExp(`/shared/${wiki.id}/${planting.id}\\?r=`));
  await expect(page.getByTestId('shared-title')).toHaveText('Field notes');
  await expect(reader.getByTestId('shared-page-tree').getByRole('link')).toHaveText([
    'Planting',
    'Harvest',
    ...notes,
  ]);
  await expect(reader.getByTestId('page-body')).toContainText('Sow in rows, then see');
  // Nobody named, nothing to write, nothing of the workspace.
  await expect(page.getByText(owner.username)).toHaveCount(0);
  await expect(page.getByRole('textbox')).toHaveCount(0);
  await expect(page.getByTestId('sidebar')).toHaveCount(0);

  // The reference to the other page of the wiki opens it.
  await reader.getByTestId('page-body').getByText('Harvest').click();
  await expect(page).toHaveURL(new RegExp(`/shared/${wiki.id}/${harvest.id}\\?r=`));
  await expect(reader.getByTestId('page-title')).toHaveText('Harvest');

  // At 390 pixels the tree is a list that opens above the page.
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(reader.getByTestId('shared-page-tree')).toHaveCount(0);
  await reader.getByRole('button', { name: 'Pages' }).click();
  // The last page of a long tree is reached by scrolling the list.
  await reader.getByTestId('shared-page-tree').getByRole('link', { name: 'Note 30' }).click();
  await expect(reader.getByTestId('page-title')).toHaveText('Note 30');
  await reader.getByRole('button', { name: 'Pages' }).click();
  await reader.getByTestId('shared-page-tree').getByRole('link', { name: 'Planting' }).click();
  await expect(reader.getByTestId('page-title')).toHaveText('Planting');
  expect(stray).toEqual([]);

  // Pictured in Korean, wide and at 390 pixels, as nobody.
  if (process.env.FVOCI_SCREENS) {
    const korean = await (await browser.newContext({ locale: 'ko-KR', baseURL: base })).newPage();
    for (const size of [
      { width: 1280, height: 800, label: 'wide' },
      { width: 390, height: 844, label: '390' },
    ]) {
      await korean.setViewportSize(size);
      await korean.goto(sharedPath(wiki.id, token, planting.id));
      await expect(korean.getByTestId('page-title')).toHaveText('Planting');
      await korean.screenshot({
        path: `${screensDir('wiki-people')}/public-reader-${size.label}.png`,
        animations: 'disabled',
      });
    }
    await korean.context().close();
  }

  // The organization's public links turned off, and on again: not found, then read.
  const mine = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = mine.find((one) => one.id === workspaceId)?.orgId ?? '';
  const admin = await signedIn(base, ADMIN);
  const links = async (publicLinks: boolean) => {
    const changed = await admin.patch(`/api/v1/org/${orgId}`, { data: { publicLinks } });
    expect(changed.ok()).toBe(true);
  };
  try {
    await links(false);
    await page.reload();
    await expect(page.getByTestId('not-found')).toBeVisible();
  } finally {
    await links(true);
  }
  await page.reload();
  await expect(reader.getByTestId('page-title')).toHaveText('Planting');
  await admin.dispose();

  // The link turned off: the same "not found" as any other.
  expect(
    (await api.post(`${at}/containers/${wiki.id}/public-link`, { data: { enabled: false } })).ok(),
  ).toBe(true);
  await page.reload();
  await expect(page.getByTestId('not-found')).toBeVisible();
  expect(errors).toEqual([]);
  await context.close();
  await api.dispose();
});
