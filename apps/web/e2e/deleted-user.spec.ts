/**
 * What is left of somebody whose account was deleted, on the in-memory
 * backend (`?gone=1` deletes the other person's): what they wrote stays, and
 * whoever reads it is shown a deleted user, wherever a name was.
 */
import { expect, test } from '@playwright/test';

import { boardPath } from './address.ts';

interface Dev {
  backend: {
    me(): Promise<{ id: string }>;
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string }[]>;
    views(boardId: string): Promise<{ id: string }[]>;
    cards(boardId: string): Promise<{ id: string; createdBy: string }[]>;
    comments(boardId: string, cardId: string): Promise<{ createdBy: string }[]>;
    users(workspaceId: string): Promise<{ id: string; username: string }[]>;
    usersById(
      workspaceId: string,
      ids: string[],
    ): Promise<{ username: string; createdAt: number; updatedAt: number; deletedAt?: number }[]>;
    searchUsers(workspaceId: string, query: string): Promise<unknown[]>;
  };
}

test('what a deleted user wrote stays, under no name', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/?gone=1');
  await expect(page.getByTestId('board-page')).toBeVisible();
  // A card somebody else commented on, who is none of the workspace's people any more.
  const place = await page.evaluate(async () => {
    const reader = (window as unknown as { fvociDev: Dev }).fvociDev.backend;
    const [workspace] = await reader.workspaces();
    const workspaceId = workspace?.id ?? '';
    const people = new Set((await reader.users(workspaceId)).map((user) => user.id));
    for (const board of await reader.boards(workspaceId)) {
      const [view] = await reader.views(board.id);
      for (const card of await reader.cards(board.id)) {
        const comments = await reader.comments(board.id, card.id);
        const gone = comments.find((comment) => !people.has(comment.createdBy));
        if (!gone || !view) continue;
        const [left] = await reader.usersById(workspaceId, [gone.createdBy]);
        const found = await reader.searchUsers(workspaceId, 'minjun');
        return { boardId: board.id, viewId: view.id, cardId: card.id, left, found };
      }
    }
    throw new Error('the seed has no comment of the deleted user');
  });
  // Asked by identifier they are what is left of the account; asked by name, nobody.
  expect(place.left?.username).toBe('');
  // Nor when the account came or went.
  expect([place.left?.createdAt, place.left?.updatedAt, place.left?.deletedAt]).toEqual([
    0,
    0,
    undefined,
  ]);
  expect(place.found).toEqual([]);

  await page.goto(`${boardPath(place.boardId, place.viewId, place.cardId)}?gone=1`);
  const comments = page.getByTestId('card-comments');
  await expect(comments.getByText('Deleted user', { exact: true }).first()).toBeVisible();
  await expect(comments).not.toContainText('minjun');
  // Nowhere on the page, behind the card or on it: not as the writer of a comment,
  // nor where a card is set to them.
  await expect(page.locator('body')).not.toContainText('Unknown user');
  await expect(page.locator('body')).not.toContainText('minjun');
  expect(errors).toEqual([]);
});
