/**
 * An invited workspace guest must be able to open the same board in the app and API.
 *
 * The review's test (fvoci/review-org-o4), as it reads now that the role is changed
 * by the route of the workspace's members and a board's routes are under its
 * workspace.
 */
import { expect, test } from '@playwright/test';

import {
  ADMIN,
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  type Person,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

/** Makes a member of the workspace a guest of it, as its administrator does. */
async function makeGuest(baseURL: string, who: Person, workspaceId: string): Promise<void> {
  const theirs = await signedIn(baseURL, who);
  const me = (await (await theirs.get('/api/v1/users/me')).json()) as Schemas['User'];
  await theirs.dispose();
  const admin = await signedIn(baseURL, ADMIN);
  const changed = await admin.put(`/api/v1/w/${workspaceId}/members/${me.id}`, {
    data: { role: 'guest' },
  });
  expect(changed.status(), await changed.text()).toBe(200);
  expect(((await changed.json()) as Schemas['WorkspaceMember']).role).toBe('guest');
  await admin.dispose();
}

test('an invited workspace guest opens its board in the app', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  const owner = person('guest-review-owner');
  const guest = person('guest-review-reader');
  await register(base, owner);
  await register(base, guest);
  const ownerApi = await signedIn(base, owner);
  const board = await makeBoard(ownerApi, {
    title: 'Guest invitation',
    cards: [{ title: 'Visible to the invited guest', status: 'To do' }],
    members: [{ who: guest, role: 'viewer' }],
  });
  await ownerApi.dispose();
  await makeGuest(base, guest, board.workspaceId);

  const guestApi = await signedIn(base, guest);
  const cards = await guestApi.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`);
  expect(cards.status(), await cards.text()).toBe(200);
  expect(await cards.text()).toContain('Visible to the invited guest');
  await guestApi.dispose();

  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, guest);
  await expect(page.getByTestId('board-page')).toBeVisible();
  await expect(page.getByText('Visible to the invited guest')).toBeVisible();
});
