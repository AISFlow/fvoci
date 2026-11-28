/**
 * Comments on the real server: written, changed and deleted from the card,
 * their text a body document, by whom the server lets; seen by another
 * person as they happen, with who changed them and what they said before;
 * and a change begun before somebody else's not stored over it unseen.
 */
import { expect, request, test, type APIRequestContext, type Page } from '@playwright/test';

import { primaryKey, primaryName } from '../e2e/keyboard.support.ts';
import {
  boardPath,
  makeBoard,
  type Person,
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

/** The text of a body, a line per block. */
function lines(body: unknown): string[] {
  const words = (node: Node): string =>
    node.text ?? (node.content ?? []).map((child) => words(child)).join('');
  return ((body as Node | null)?.content ?? []).map((block) => words(block));
}

async function commentsOn(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
  cardId: string,
): Promise<Schemas['Block'][]> {
  return (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`)
  ).json()) as Schemas['Block'][];
}

async function openCard(page: Page, path: string, who: Person): Promise<void> {
  await page.goto(path);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
}

async function send(page: Page, text: string): Promise<void> {
  await page.getByTestId('comment-field').fill(text);
  await page.getByTestId('comment-send').click();
}

test('a comment is written, changed and deleted on the server, its text a document', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('talker');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Talk',
    cards: [{ title: 'Choose a date', status: 'To do' }],
  });
  const cardId = board.cards['Choose a date'] ?? '';
  const stored = () => commentsOn(api, board, cardId);

  const errors = watchErrors(page);
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await send(page, 'How about Friday?\nOr Monday');
  const comments = page.getByTestId('card-comments');
  await expect(comments.locator('[data-comment]')).toHaveCount(1);
  await expect
    .poll(async () => (await stored()).map((one) => lines(one.body)))
    .toEqual([['How about Friday?', 'Or Monday']]);

  await comments.getByRole('button', { name: /^Edit the comment from/ }).click();
  const field = comments.getByTestId('comment-edit').locator('textarea');
  await expect(field).toHaveValue('How about Friday?\nOr Monday');
  await field.fill('Friday it is');
  const modifier = await primaryName(page);
  await expect(field).toHaveAttribute('aria-keyshortcuts', `${modifier}+Enter`);
  await field.press(`${modifier === 'Meta' ? 'Control' : 'Meta'}+Enter`);
  // Saving disables this button synchronously; a forbidden key cannot start a write.
  await expect(comments.getByRole('button', { name: 'Save' })).toBeEnabled();
  await expect(field).toBeVisible();
  expect((await stored()).map((one) => lines(one.body))).toEqual([
    ['How about Friday?', 'Or Monday'],
  ]);
  await field.fill('Friday it is');
  await field.press(await primaryKey(page, 'Enter'));
  await expect(comments.getByTestId('comment-edit')).toHaveCount(0);
  await expect(comments.locator('[data-comment]')).toHaveText(/Friday it is/);
  await expect
    .poll(async () => (await stored()).map((one) => lines(one.body)))
    .toEqual([['Friday it is']]);

  await page.reload();
  await expect(comments.locator('[data-comment]')).toHaveText(/Friday it is/);
  await comments.getByRole('button', { name: /^Delete the comment from/ }).click();
  await expect(comments.locator('[data-comment]')).toHaveCount(0);
  await expect.poll(stored).toEqual([]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a commenter changes their own comment, an admin anybody’s, an editor nobody else’s; what it said before is kept', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('host');
  const guest = person('guest');
  const helper = person('helper');
  for (const who of [owner, guest, helper]) await register(base, who);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Guests',
    cards: [{ title: 'Menu', status: 'To do' }],
    members: [
      { who: guest, role: 'commenter' },
      { who: helper, role: 'editor' },
    ],
  });
  const cardId = board.cards.Menu ?? '';
  const path = boardPath(board, board.viewId, cardId);

  const pages: Page[] = [];
  const watched: string[][] = [];
  for (const who of [owner, guest, helper]) {
    const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
    pages.push(page);
    watched.push(watchErrors(page));
    await openCard(page, path, who);
  }
  const [hostPage, guestPage, helperPage] = pages as [Page, Page, Page];
  const editOf = (page: Page) => page.getByRole('button', { name: /^Edit the comment from/ });
  const deleteOf = (page: Page) => page.getByRole('button', { name: /^Delete the comment from/ });

  await send(guestPage, 'No mushrooms, please');
  for (const page of pages) {
    await expect(page.getByTestId('card-comments').locator('[data-comment]')).toHaveText(
      /No mushrooms, please/,
    );
  }
  // The commenter changes and deletes their own; the editor neither on somebody
  // else's; the admin both.
  await expect(editOf(guestPage)).toHaveCount(1);
  await expect(deleteOf(guestPage)).toHaveCount(1);
  await expect(editOf(helperPage)).toHaveCount(0);
  await expect(deleteOf(helperPage)).toHaveCount(0);
  await expect(editOf(hostPage)).toHaveCount(1);
  await expect(deleteOf(hostPage)).toHaveCount(1);

  // The commenter changes theirs, then the admin changes it again.
  await editOf(guestPage).click();
  await guestPage.getByTestId('comment-edit').locator('textarea').fill('No mushrooms or olives');
  await guestPage.getByRole('button', { name: 'Save' }).click();
  await expect(hostPage.getByTestId('card-comments').locator('[data-comment]')).toHaveText(
    /No mushrooms or olives/,
  );
  await editOf(hostPage).click();
  await hostPage.getByTestId('comment-edit').locator('textarea').fill('No mushrooms, no olives');
  await hostPage.getByRole('button', { name: 'Save' }).click();
  await expect(hostPage.getByTestId('comment-edit')).toHaveCount(0);
  expect((await commentsOn(api, board, cardId)).map((one) => lines(one.body))).toEqual([
    ['No mushrooms, no olives'],
  ]);

  // Saving the same text again is no change: the admin stays the one who changed it.
  await expect(guestPage.getByTestId('card-comments').locator('[data-comment]')).toHaveText(
    /No mushrooms, no olives/,
  );
  await editOf(guestPage).click();
  await guestPage.getByRole('button', { name: 'Save' }).click();
  await expect(guestPage.getByTestId('comment-edit')).toHaveCount(0);
  const me = (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
  expect((await commentsOn(api, board, cardId))[0]?.modifiedBy).toBe(me.id);

  // Everybody sees who changed it last, as it happens, and can read what it said before.
  for (const page of pages) {
    const comment = page.getByTestId('card-comments').locator('[data-comment]');
    await expect(comment).toHaveText(/No mushrooms, no olives/);
    await expect(comment.getByTestId('comment-edited')).toHaveText(
      new RegExp(`^edited by ${owner.username} `),
    );
  }
  const history = helperPage.getByRole('button', {
    name: /^What the comment from .* said before$/,
  });
  await history.click();
  await expect(history).toHaveAttribute('aria-expanded', 'true');
  const versions = helperPage.getByTestId('comment-history').locator('[data-version]');
  await expect(versions).toHaveCount(3);
  await expect(versions.nth(0)).toContainText(`Posted by ${guest.username}`);
  await expect(versions.nth(0)).toContainText('No mushrooms, please');
  await expect(versions.nth(0).locator('ins, del')).toHaveCount(0);
  // Each later text says what it changed from the one before, the text now last.
  await expect(versions.nth(1)).toContainText(`Changed by ${guest.username}`);
  await expect(versions.nth(1).locator('del')).toHaveText([', please']);
  await expect(versions.nth(1).locator('ins')).toHaveText([' or olives']);
  await expect(versions.nth(2)).toContainText(`Now, changed by ${owner.username}`);
  await expect(versions.nth(2).locator('del')).toHaveText([' or']);
  await expect(versions.nth(2).locator('ins')).toHaveText([', no']);
  // Only to read: nothing in it puts a text back.
  await expect(helperPage.getByTestId('comment-history').getByRole('button')).toHaveCount(0);

  // A reader by the public link gets the comment, but not what it said before.
  const shared = (await (
    await api.post(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];
  const anonymous = await request.newContext({ baseURL: base });
  const [comment] = await commentsOn(api, board, cardId);
  const asReader = await anonymous.get(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments/${comment?.id ?? ''}/versions`,
    { params: { readToken: shared.token ?? '' } },
  );
  expect(asReader.status()).toBe(401);
  await anonymous.dispose();

  for (const page of pages) await page.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a comment changed elsewhere while its field is open is not written over unseen', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('reviser');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Revise',
    cards: [{ title: 'Draft the letter', status: 'To do' }],
  });
  const cardId = board.cards['Draft the letter'] ?? '';
  const stored = () => commentsOn(api, board, cardId);
  const paragraph = (text: string) => ({
    type: 'doc',
    content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
  });

  const errors = watchErrors(page);
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await send(page, 'First words');
  const comments = page.getByTestId('card-comments');
  await expect(comments.locator('[data-comment]')).toHaveText(/First words/);
  await comments.getByRole('button', { name: /^Edit the comment from/ }).click();
  const field = comments.getByTestId('comment-edit').locator('textarea');
  await field.fill('Better words');

  // Meanwhile the same person changes it in another tab; the page hears of it.
  const [comment] = await stored();
  const changed = await api.patch(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments/${comment?.id ?? ''}`,
    {
      data: { body: paragraph('Words from the other tab'), readAt: comment?.updatedAt },
    },
  );
  expect(changed.status()).toBe(200);

  // Saving on the version the field was opened on is refused: theirs is shown, nothing written over.
  await comments.getByRole('button', { name: 'Save' }).click();
  const theirs = comments.getByTestId('comment-theirs');
  await expect(theirs).toContainText('Words from the other tab');
  expect((await stored()).map((one) => lines(one.body))).toEqual([['Words from the other tab']]);
  await expect(field).toHaveValue('Better words');

  // Seen, saving again writes over it.
  await comments.getByRole('button', { name: 'Save' }).click();
  await expect(comments.getByTestId('comment-edit')).toHaveCount(0);
  await expect
    .poll(async () => (await stored()).map((one) => lines(one.body)))
    .toEqual([['Better words']]);
  // The focus is back on the comment's edit button.
  await expect(comments.getByRole('button', { name: /^Edit the comment from/ })).toBeFocused();
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a commenter deletes their own comment and brings it back with undo; somebody else’s has no delete button', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('keeper');
  const guest = person('remover');
  await register(base, owner);
  await register(base, guest);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Second thoughts',
    cards: [{ title: 'Plan', status: 'To do' }],
    members: [{ who: guest, role: 'commenter' }],
  });
  const cardId = board.cards.Plan ?? '';
  const path = boardPath(board, board.viewId, cardId);
  const stored = async () =>
    (await commentsOn(api, board, cardId)).map((one) => lines(one.body)).sort();

  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const errors = watchErrors(page);
  await openCard(page, path, guest);
  // The owner's comment first, then the guest's own.
  await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`,
    {
      data: {
        body: {
          type: 'doc',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'The owner says' }] }],
        },
      },
    },
  );
  const comments = page.getByTestId('card-comments').locator('[data-comment]');
  await expect(comments).toHaveCount(1);
  await send(page, 'I take it back');
  await expect(comments).toHaveCount(2);

  const theirs = comments.filter({ hasText: 'The owner says' });
  const mine = comments.filter({ hasText: 'I take it back' });
  const deleteMine = mine.getByRole('button', { name: /^Delete the comment from/ });
  await expect(deleteMine).toBeVisible();
  await expect(theirs.getByRole('button', { name: /^Delete the comment from/ })).toHaveCount(0);
  await deleteMine.click();
  await expect(comments).toHaveCount(1);
  await expect.poll(stored).toEqual([['The owner says']]);

  // Undo, from the card's menu, brings it back on the server.
  await page.getByTestId('card-menu').click();
  await page.locator('[data-action="undo"]').click();
  await expect(comments).toHaveCount(2);
  await expect.poll(stored).toEqual([['I take it back'], ['The owner says']]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  await page.context().close();
  await api.dispose();
  expect(errors).toEqual([]);
});
