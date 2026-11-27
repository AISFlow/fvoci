/**
 * The time of a card's last comment on the real server: "last updated
 * time" counts it, so a view sorted by it moves a card when somebody else
 * comments on it, and back when that comment is deleted.
 */
import { expect, test, type Page } from '@playwright/test';

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

const paragraph = (text: string) => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** The titles of the cards in the column, top to bottom. */
const titles = (page: Page) => page.locator('[data-card]').allInnerTexts();

test('a comment somebody else posts moves the card in a view sorted by last update, and its deletion moves it back', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('sorter');
  const guest = person('remarker');
  await register(base, owner);
  await register(base, guest);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Latest first',
    cards: [{ title: 'Quiet', status: 'To do' }],
    members: [{ who: guest, role: 'commenter' }],
  });
  // "Talked about" is older than "Quiet": made first, changed never.
  const older = board.cards.Quiet ?? '';
  const made = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
    data: { title: 'Talked about', properties: { status: board.options['To do'] } },
  });
  const talked = ((await made.json()) as Schemas['Card']).id;
  await api.patch(`/api/v1/w/${board.workspaceId}/cards/${older}`, { data: { title: 'Quiet' } });
  // The board gets a "last updated time" and the view is sorted by it, newest first.
  await api.patch(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`, {
    data: {
      updatedCardProperties: [{ id: 'updated', name: 'Updated', type: 'updatedTime', options: [] }],
    },
  });
  await api.patch(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks/${board.viewId}`, {
    data: {
      updatedFields: { sort: [{ propertyId: 'updated', reversed: true }] },
    },
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect
    .poll(() => titles(page))
    .toEqual([expect.stringContaining('Quiet'), expect.stringContaining('Talked about')]);

  // The guest comments on the older card: it is the newest change now.
  const guestApi = await signedIn(base, guest);
  const posted = await guestApi.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`,
    {
      data: [
        {
          containerId: board.boardId,
          parentId: talked,
          type: 'comment',
          title: '',
          fields: {},
          createdAt: 1,
          updatedAt: 1,
          body: paragraph('Any news?'),
        },
      ],
    },
  );
  expect(posted.ok()).toBe(true);
  const [comment] = (await posted.json()) as Schemas['Block'][];
  await expect
    .poll(() => titles(page))
    .toEqual([expect.stringContaining('Talked about'), expect.stringContaining('Quiet')]);
  const card = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${talked}`)
  ).json()) as Schemas['Card'];
  expect(card.lastCommentAt).toBe(comment?.createdAt);

  // The guest's comment goes (the admin deletes it): the card is where it was.
  const deleted = await api.delete(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks/${comment?.id ?? ''}`,
  );
  expect(deleted.ok()).toBe(true);
  await expect
    .poll(() => titles(page))
    .toEqual([expect.stringContaining('Quiet'), expect.stringContaining('Talked about')]);

  await guestApi.dispose();
  await api.dispose();
  expect(errors).toEqual([]);
});
