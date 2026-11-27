/**
 * Review of chores-1: who left the workspace and wrote a card is named by the
 * server (`POST /users` by identifier), so the web must learn them too, or the
 * card's "Created by" and "Last updated by" say "Unknown user".
 */
import { expect, it, vi } from 'vitest';

import { openFirstBoard, setup } from './support.ts';

it('learns a departed card author whose name the server gives by identifier', async () => {
  const made = setup();
  const { world, backend, other, workspace } = made;
  const stored = [...world.containers.values()][0];
  if (!stored) throw new Error('no board');
  for (const block of world.blocksOf(stored.id)) {
    if (block.type !== 'card') continue;
    block.createdBy = other.userId;
    block.modifiedBy = other.userId;
  }
  world.members.set(
    stored.id,
    (world.members.get(stored.id) ?? []).filter((member) => member.userId !== other.userId),
  );
  vi.spyOn(backend, 'users').mockResolvedValue(
    [...world.users.values()].filter((user) => user.id !== other.userId),
  );
  vi.spyOn(backend, 'usersById').mockImplementation(async (_workspaceId, ids) =>
    [...world.users.values()].filter((user) => ids.includes(user.id)),
  );
  const { board } = await openFirstBoard(made);
  expect(board.cards.some((card) => card.createdBy === other.userId)).toBe(true);
  await vi.waitFor(() => {
    expect(workspace.userName(other.userId)).toBeTruthy();
  });
});
