/**
 * Boards on the real server: the list of boards in the sidebar, and a board
 * opened from it with its views, cards and members. The board is made
 * through the API by its owner; another person, a member who may only view
 * it, opens it on the page.
 */
import { expect, test } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  putAwayWelcomeBoard,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

test('a board of the server is listed and opens with its views, cards and members', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  // The server lists only the ten oldest people of a workspace: with ten before
  // them, the owner and the reader are past that list.
  for (let made = 0; made < 10; made += 1) await register(base, person('earlier'));
  const owner = person('owner');
  const reader = person('reader');
  await register(base, owner);
  await register(base, reader);
  // The reader has the one board: what the app opens by itself is it.
  const readerApi = await signedIn(base, reader);
  await putAwayWelcomeBoard(readerApi);
  await readerApi.dispose();
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Garden plan',
    cards: [
      { title: 'Dig the beds', status: 'To do' },
      { title: 'Order seeds', status: 'To do' },
      { title: 'Build a fence', status: 'Done' },
    ],
    members: [{ who: reader, role: 'viewer' }],
  });
  await api.dispose();

  const errors = watchErrors(page);
  await page.goto('/');
  // Waited for before signing in: the app opens the first board of the
  // sidebar by itself, and with it reads its members' names.
  const byId = page.waitForRequest(
    (request) => request.method() === 'POST' && /\/api\/v1\/w\/[^/]+\/users$/.test(request.url()),
  );
  await signInOnPage(page, reader);

  const sidebar = page.getByTestId('sidebar');
  await expect(sidebar.getByText('Garden plan')).toBeVisible();
  await sidebar.getByText('Garden plan').click();

  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page).toHaveURL(new RegExp(boardPath(board, board.viewId)));
  await expect(page.getByTestId('board-title-text')).toHaveText('Garden plan');
  await expect(
    page.getByRole('navigation', { name: 'Views' }).locator('[data-tool="views"]'),
  ).toHaveText('By status');

  const column = (name: string) =>
    page.locator('[data-column]').filter({
      has: page.locator('h3').filter({ hasText: new RegExp(`^\\s*${name}\\s*$`) }),
    });
  await expect(column('To do').locator('[data-card]')).toHaveCount(2);
  await expect(column('Done').locator('[data-card]')).toHaveCount(1);
  await expect(column('Done').getByText('Build a fence')).toBeVisible();

  // Both members' names are asked for by their identifiers, though neither is on
  // the server's list of the workspace; the reader may only look. The header shows
  // who else is here now (nobody; presence.spec.ts has people come and go).
  expect(((await byId).postDataJSON() as string[]).length).toBe(2);
  // Nobody else here: the group says it, and there is no button of faces.
  await expect(page.getByTestId('board-people')).toHaveAccessibleName(
    /^People viewing this board: 0, members: \d+$/,
  );
  await expect(page.getByTestId('board-presence')).toBeHidden();
  await expect(page.getByTestId('read-only')).toBeVisible();

  // A reload opens the same board from its address.
  await page.reload();
  await expect(column('To do').locator('[data-card]')).toHaveCount(2);
  expect(errors).toEqual([]);
});
