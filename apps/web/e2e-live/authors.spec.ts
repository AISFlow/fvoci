/**
 * A departed writer is named where their work is read: an earlier comment revision, a
 * comment, a card's last change. (That they are not offered as a member is the unit tests'.)
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import {
  ADMIN,
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

const body = (text: string) => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

async function revise(api: APIRequestContext, path: string, text: string): Promise<void> {
  const split = path.lastIndexOf('/');
  const read = await api.get(path.slice(0, split));
  expect(read.ok(), await read.text()).toBe(true);
  const id = path.slice(split + 1);
  const current = ((await read.json()) as Schemas['Block'][]).find((one) => one.id === id);
  if (!current) throw new Error('the comment was not returned');
  const response = await api.patch(path, { data: { body: body(text), readAt: current.updatedAt } });
  expect(response.ok(), await response.text()).toBe(true);
}

test('a departed writer of an earlier revision is named when the comment history opens', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('current-author');
  const earlier = person('earlier-author');
  for (const who of [owner, earlier]) await register(base, who);
  const api = await signedIn(base, owner);
  const writer = await signedIn(base, earlier);
  const writerId = ((await (await writer.get('/api/v1/users/me')).json()) as Schemas['User']).id;
  const board = await makeBoard(api, {
    title: 'Kept revisions',
    cards: [{ title: 'A conversation', status: 'To do' }],
  });
  const prefix = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const added = await api.post(`${prefix}/members`, {
    data: { userId: writerId, role: 'admin' },
  });
  expect(added.ok(), await added.text()).toBe(true);
  const posted = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${board.cards['A conversation'] ?? ''}/comments`,
    {
      data: { body: body('Original words') },
    },
  );
  expect(posted.ok(), await posted.text()).toBe(true);
  const comment = (await posted.json()) as Schemas['Block'];
  const path = `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${comment.parentId}/comments/${comment.id}`;
  await revise(writer, path, 'Words from the departed writer');
  await revise(api, path, 'Current words');
  const admin = await signedIn(base, ADMIN);
  const removed = await admin.delete(`/api/v1/w/${board.workspaceId}/members/${writerId}`);
  expect(removed.ok(), await removed.text()).toBe(true);
  const listed = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/users`, {
      params: { search: earlier.username },
    })
  ).json()) as Schemas['User'][];
  expect(listed).toEqual([]);
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, board.cards['A conversation']));
  await signInOnPage(page, owner);
  await page.getByRole('button', { name: /^What the comment from .* said before$/ }).click();
  const version = page.getByTestId('comment-history').locator('[data-version="2"]');
  await expect(version).toContainText(`Changed by ${earlier.username}`);
  await expect(version).toContainText('Words from the departed writer');
  await expect(page.getByTestId('comment-history')).not.toContainText('Unknown user');
  expect(errors).toEqual([]);
  for (const client of [api, writer, admin]) await client.dispose();
});

test('a departed comment author is named without opening the comment history', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('comment-reader');
  const departed = person('comment-writer');
  for (const who of [owner, departed]) await register(base, who);
  const api = await signedIn(base, owner);
  const writer = await signedIn(base, departed);
  const writerId = ((await (await writer.get('/api/v1/users/me')).json()) as Schemas['User']).id;
  const board = await makeBoard(api, {
    title: 'Remembered authors',
    cards: [{ title: 'Kept conversation', status: 'To do' }],
  });
  const prefix = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const added = await api.post(`${prefix}/members`, {
    data: { userId: writerId, role: 'commenter' },
  });
  expect(added.ok(), await added.text()).toBe(true);
  const posted = await writer.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${board.cards['Kept conversation'] ?? ''}/comments`,
    {
      data: { body: body('Words that remain after leaving') },
    },
  );
  expect(posted.ok(), await posted.text()).toBe(true);
  const admin = await signedIn(base, ADMIN);
  const removed = await admin.delete(`/api/v1/w/${board.workspaceId}/members/${writerId}`);
  expect(removed.ok(), await removed.text()).toBe(true);
  const listed = await api.get(`/api/v1/w/${board.workspaceId}/users`, {
    params: { search: departed.username },
  });
  expect(listed.ok()).toBe(true);
  expect(await listed.json()).toEqual([]);
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, board.cards['Kept conversation']));
  await signInOnPage(page, owner);
  await expect(page.getByText('Words that remain after leaving')).toBeVisible();
  await expect(
    page.getByTestId('card-comments').getByText(departed.username, { exact: true }),
  ).toBeVisible();
  await expect(page.getByTestId('card-comments')).not.toContainText('Unknown user');
  await expect(page.getByTestId('comment-history')).toHaveCount(0);
  expect(errors).toEqual([]);
  for (const client of [api, writer, admin]) await client.dispose();
});

test('a departed editor of a card is named under the card’s “Last updated by”', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('card-reader');
  const departed = person('card-editor');
  for (const who of [owner, departed]) await register(base, who);
  const api = await signedIn(base, owner);
  const writer = await signedIn(base, departed);
  const board = await makeBoard(api, {
    title: 'Changed cards',
    cards: [{ title: 'Before the change', status: 'To do' }],
    members: [{ who: departed, role: 'editor' }],
  });
  const workspace = `/api/v1/w/${board.workspaceId}`;
  const cardId = board.cards['Before the change'] ?? '';
  const patched = await api.patch(`${workspace}/containers/${board.boardId}`, {
    data: {
      updatedCardProperties: [
        { id: 'changer', name: 'Changed by', type: 'updatedBy', options: [] },
      ],
    },
  });
  expect(patched.ok(), await patched.text()).toBe(true);
  const card = (await (await writer.get(`${workspace}/cards/${cardId}`)).json()) as Schemas['Card'];
  const changed = await writer.patch(`${workspace}/cards/${cardId}`, {
    data: { readAt: card.updatedAt, title: 'After the change' },
  });
  expect(changed.ok(), await changed.text()).toBe(true);
  const writerId = ((await (await writer.get('/api/v1/users/me')).json()) as Schemas['User']).id;
  const admin = await signedIn(base, ADMIN);
  const removed = await admin.delete(`${workspace}/members/${writerId}`);
  expect(removed.ok(), await removed.text()).toBe(true);

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  const properties = page.getByTestId('card-properties');
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(properties).toContainText('Changed by');
  await expect(properties.getByText(departed.username, { exact: true })).toBeVisible();
  await expect(properties).not.toContainText('Unknown user');
  expect(errors).toEqual([]);
  for (const client of [api, writer, admin]) await client.dispose();
});
