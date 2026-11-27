/**
 * An open board on the real server, opened by somebody of the workspace who is
 * no member of it: it is read as its members read it, nobody is made a member
 * by opening it, and the first write on it joins it, with no gate to press.
 */
import { expect, test } from '@playwright/test';

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

test('an open board is joined by the first write on it, not by opening it', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('opener');
  const visitor = person('visitor');
  await register(base, owner);
  await register(base, visitor);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Commons',
    visibility: 'open',
    description: 'For the whole workspace',
    cards: [{ title: 'Water the plants', status: 'To do' }],
  });
  const memberNames = async (): Promise<string[]> => {
    const members = (await (
      await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/members`)
    ).json()) as Schemas['ContainerMember'][];
    const users = (await (
      await api.post(`/api/v1/w/${board.workspaceId}/users`, {
        data: members.map((member) => member.userId),
      })
    ).json()) as Schemas['User'][];
    return users.map((user) => user.username).sort();
  };

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, visitor);
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page.getByText('Water the plants')).toBeVisible();
  await expect(page.getByTestId('board-join')).toHaveCount(0);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  // Opening it made nobody a member.
  expect(await memberNames()).toEqual([owner.username]);

  // A card written there joins it first, and is kept.
  await page.getByRole('button', { name: 'New card in To do' }).click();
  await page.getByTestId('card-title').fill('Feed the fish');
  await page.goBack();
  await expect(page.getByText('Feed the fish')).toBeVisible();
  await expect.poll(memberNames).toEqual([owner.username, visitor.username].sort());

  await page.reload();
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});
