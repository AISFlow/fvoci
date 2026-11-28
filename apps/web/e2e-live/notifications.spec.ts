/**
 * Notifications on the real server, two people each in a browser of their
 * own: a comment one writes on a card the other wrote rings the other's bell
 * without a reload; the notification opens the card at the comment and is
 * read by it; the switch of comments, turned off in the settings, is kept
 * by the server, which tells nothing of comments while it is off; and a
 * mention typed into a body two people write together, in the browser's
 * editor (yjs), is told as the doing of who typed it.
 */
import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Locator,
  type Page,
} from '@playwright/test';

import { showSidebar, sidebar, strip } from '../e2e/shell.support.ts';
import {
  boardPath,
  makeBoard,
  type MadeBoard,
  type Person,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const bell = (page: Page): Locator => page.getByTestId('open-notifications');
const badge = (page: Page): Locator => page.getByTestId('unread-notifications');
const inbox = (page: Page): Locator => page.getByTestId('notifications');

async function send(page: Page, text: string): Promise<void> {
  await page.getByTestId('comment-field').fill(text);
  await page.getByTestId('comment-send').click();
}

/** The switch of comments in the settings, opened from the reader's account. */
async function commentsSwitch(page: Page): Promise<Locator> {
  if (await strip(page).isVisible()) {
    await strip(page).getByTestId('strip-open-settings').click();
  } else {
    await showSidebar(page);
    await sidebar(page).getByTestId('open-settings').click();
  }
  const dialog = page.getByTestId('settings-dialog');
  await expect(dialog).toBeVisible();
  return dialog.locator('[data-setting="notifyComments"]').getByRole('switch');
}

async function closeSettings(page: Page): Promise<void> {
  await page.getByTestId('close-settings').click();
  await expect(page.getByTestId('settings-dialog')).toBeHidden();
}

async function me(api: APIRequestContext): Promise<Schemas['User']> {
  return (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
}

async function settings(api: APIRequestContext): Promise<Record<string, unknown>> {
  return (await (await api.get('/api/v1/users/me/settings')).json()) as Record<string, unknown>;
}

/**
 * A writer and a commenter of a board, each signed in to the API: the writer
 * made its two cards, and who writes a card watches it.
 */
async function aWatchedBoard(base: string) {
  const writer = person('watcher');
  const talker = person('talker');
  await register(base, writer);
  await register(base, talker);
  const api = await signedIn(base, writer);
  const board = await makeBoard(api, {
    title: 'Told',
    cards: [
      { title: 'Plan the week', status: 'To do' },
      { title: 'Book the room', status: 'To do' },
    ],
    members: [{ who: talker, role: 'commenter' }],
  });
  return { writer, talker, api, board, me: await me(api) };
}

type Inline = { type: 'text'; text: string } | { type: 'mention'; attrs: { userId: string } };

/** Writes a comment on a card through the API, as whoever `api` is signed in as. */
async function comment(
  api: APIRequestContext,
  board: MadeBoard,
  cardId: string,
  content: Inline[],
): Promise<void> {
  const written = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`,
    {
      data: { body: { type: 'doc', content: [{ type: 'paragraph', content }] } },
    },
  );
  expect(written.ok(), await written.text()).toBe(true);
}

test('a comment on a card one wrote rings their bell at once, and opens the card at it', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { writer, talker, api, board } = await aWatchedBoard(base);
  const planned = board.cards['Plan the week'] ?? '';

  const theirs = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const mine = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const errors = [theirs, mine].map(watchErrors);
  await mine.goto(boardPath(board, board.viewId));
  await signInOnPage(mine, writer);
  await expect(bell(mine)).toBeVisible();
  await expect(badge(mine)).toBeHidden();
  await theirs.goto(boardPath(board, board.viewId, planned));
  await signInOnPage(theirs, talker);
  await expect(theirs.getByTestId('card-comments')).toBeVisible();

  // The other person comments: the writer's bell rings, no reload.
  await send(theirs, 'How about Friday?');
  const said = theirs.getByTestId('card-comments').locator('[data-comment]');
  await expect(said).toHaveCount(1);
  const commentId = (await said.first().getAttribute('data-comment')) ?? '';
  await expect(badge(mine)).toHaveText('1');

  // Opened from the bell: the card, at the comment, and read.
  await bell(mine).click();
  const line = inbox(mine).getByRole('link', { name: /commented on Plan the week/ });
  await expect(line).toBeVisible();
  await line.click();
  await expect(mine.getByTestId('card-dialog')).toBeVisible();
  await expect(mine.getByTestId('card-title')).toHaveValue('Plan the week');
  await expect(mine.locator(`[data-comment="${commentId}"]`)).toBeInViewport();
  await expect(badge(mine)).toBeHidden();
  // Read on the server too.
  const unread = async (): Promise<unknown> =>
    ((await (await api.get('/api/v1/notifications/unread')).json()) as { count: number }).count;
  await expect.poll(unread).toBe(0);

  await api.dispose();
  expect(errors.flat()).toEqual([]);
});

test('the switch of comments is kept by the server, which tells nothing of comments while it is off', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const { writer, talker, api, board, me } = await aWatchedBoard(base);
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, writer);

  const off = await commentsSwitch(page);
  await expect(off).toBeChecked();
  await off.click();
  await expect(off).not.toBeChecked();
  await expect.poll(() => settings(api)).toMatchObject({ notifyComments: false });
  await closeSettings(page);
  await page.reload();
  await expect(bell(page)).toBeVisible();
  await expect(await commentsSwitch(page)).not.toBeChecked();
  await closeSettings(page);

  // Off, a comment tells nothing. A mention after it, on the other card, is
  // told: by the time it rings, the comment before it was worked off.
  const talkerApi = await signedIn(base, talker);
  await comment(talkerApi, board, board.cards['Plan the week'] ?? '', [
    { type: 'text', text: 'How about Friday?' },
  ]);
  await comment(talkerApi, board, board.cards['Book the room'] ?? '', [
    { type: 'text', text: 'Can you book it, ' },
    { type: 'mention', attrs: { userId: me.id } },
  ]);
  await expect(badge(page)).toHaveText('1');
  await bell(page).click();
  await expect(
    inbox(page).getByRole('link', { name: /mentioned you in Book the room/ }),
  ).toBeVisible();
  await expect(inbox(page).locator('[data-notification]')).toHaveCount(1);

  await talkerApi.dispose();
  await api.dispose();
  expect(errors).toEqual([]);
});

const editor = (page: Page): Locator =>
  page.getByTestId('card-body').locator('[contenteditable]').first();

/** A browser of its own, at the address, signed in, with the card open and its body ready. */
async function writing(browser: Browser, base: string, address: string, who: Person) {
  const page = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  await page.goto(address);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return page;
}

test('a mention typed into a body two people write is told as the doing of who typed it, not of who wrote last', async ({
  browser,
  baseURL,
}) => {
  // The server writes a body from its document within ten seconds of the last stroke.
  test.setTimeout(60_000);
  const base = baseURL ?? '';
  const [ada, bora, cleo] = [person('ada'), person('bora'), person('cleo')];
  for (const who of [ada, bora, cleo]) await register(base, who);
  const api = await signedIn(base, ada);
  const board = await makeBoard(api, {
    title: 'Typed',
    cards: [{ title: 'Write together', status: 'To do' }],
    members: [
      { who: bora, role: 'editor' },
      { who: cleo, role: 'viewer' },
    ],
  });
  const cardId = board.cards['Write together'] ?? '';
  const address = boardPath(board, board.viewId, cardId);
  const [boraApi, cleoApi] = [await signedIn(base, bora), await signedIn(base, cleo)];
  const [adaId, boraId] = [(await me(api)).id, (await me(boraApi)).id];
  const first = await writing(browser, base, address, ada);
  const second = await writing(browser, base, address, bora);
  const errors = [first, second].map(watchErrors);
  try {
    // Ada writes a line; Bora names Cleo after it, picking her from the people offered.
    await editor(first).click();
    await first.keyboard.type('Who books the room?');
    await expect(editor(second)).toContainText('Who books the room?');
    await editor(second).click();
    await second.keyboard.press('End');
    await second.keyboard.type(` @${cleo.username}`);
    const offered = second.locator('[data-suggestions="mention"]').getByRole('option');
    await expect(offered.first()).toContainText(cleo.username);
    await second.keyboard.press('Enter');
    await expect(editor(first).locator('[data-mention]')).toHaveCount(1);
    // Ada writes last: the body is written as hers.
    await editor(first).click();
    await first.keyboard.press('End');
    await first.keyboard.type('Thanks.');
    await expect(editor(second)).toContainText('Thanks.');

    // Wait for both Ada's final edit projection and Cleo's notification by Bora; nobody of their own.
    await expect
      .poll(
        async () => {
          const card = (await (
            await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
          ).json()) as Schemas['Card'];
          return {
            modifiedBy: card.modifiedBy,
            hasThanks: JSON.stringify(card.body).includes('Thanks.'),
          };
        },
        { timeout: 30_000 },
      )
      .toEqual({ modifiedBy: adaId, hasThanks: true });
    const told = async (who: APIRequestContext) =>
      ((await (await who.get('/api/v1/notifications')).json()) as Schemas['Notification'][])
        .map((one) => one.subject)
        .filter((subject) => subject?.blockId === cardId)
        .map((subject) => ({ kind: subject?.kind, actorId: subject?.actorId }));
    await expect
      .poll(() => told(cleoApi), { timeout: 30_000 })
      .toEqual([{ kind: 'mention', actorId: boraId }]);
    expect(await told(boraApi)).toEqual([]);
  } finally {
    for (const page of [first, second]) await page.context().close();
    for (const one of [api, boraApi, cleoApi]) await one.dispose();
  }
  expect(errors.flat()).toEqual([]);
});
