/**
 * What an anonymous reader of a shared board gets on the real server: every
 * mention of a body with an empty `userId`. The app has no screen for an
 * anonymous reader (a shared board by its read token), so that body is taken
 * from the server's answer here, and what is checked of it is the web app's
 * validator (it draws the body, `shown`, and would not write it) and the
 * server (it refuses the body back). The app's own card screen is checked to
 * name somebody it does not know "Unknown user", as it names nobody.
 */
import { expect, request, test } from '@playwright/test';

import { validateDoc } from '../src/lib/editor/schema.ts';

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

test('a body read with a read token alone names nobody in its mentions, is drawn, and is not written back', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('sharer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const me = (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
  const board = await makeBoard(api, {
    title: 'Shared',
    cards: [{ title: 'Ask around', status: 'To do' }],
  });
  const cardId = board.cards['Ask around'] ?? '';
  const card = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const written = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
    data: {
      readAt: card.updatedAt,
      body: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [
              { type: 'text', text: 'ask ' },
              { type: 'mention', attrs: { userId: me.id } },
              { type: 'text', text: ' and ' },
              // A user's identifier the server takes, of nobody the workspace has.
              { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-0000000000ff' } },
            ],
          },
        ],
      },
    },
  });
  expect(written.ok()).toBe(true);
  const shared = (await (
    await api.post(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];
  expect(shared.token).toBeTruthy();

  // Read with the read token alone, signed in as nobody.
  const anonymous = await request.newContext({ baseURL: base });
  const [block] = (await (
    await anonymous.get(`/api/v1/shared/${board.boardId}/blocks`, {
      params: { readToken: shared.token ?? '', blockId: cardId },
    })
  ).json()) as Schemas['Block'][];
  await anonymous.dispose();
  const body = block?.body;
  expect(JSON.stringify(body)).toContain('"userId":""');

  // The web app draws it and would not write it; the server refuses it too.
  expect(validateDoc(body, { shown: true })).toEqual([]);
  expect(validateDoc(body)).not.toEqual([]);
  const now = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const back = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
    data: { readAt: now.updatedAt, body },
  });
  expect(back.status()).toBe(400);
  expect(back.headers()['content-type']).toBe('application/problem+json');
  const problem = (await back.json()) as Schemas['Problem'];
  expect(problem.type).toBe('/problems/bad-request');
  // The first node of the document that is refused: the mention that names nobody.
  expect(problem.errors).toHaveLength(1);
  expect(problem.errors?.[0]?.pointer).toMatch(/^\/body(\/content\/\d+)+$/);

  // The owner's own card screen names the owner, and the one it does not know "Unknown user".
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('card-body').locator('[data-mention]')).toHaveText([
    `@${owner.username}`,
    '@Unknown user',
  ]);
  await api.dispose();
  expect(errors).toEqual([]);
});
