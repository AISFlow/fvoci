/** An author's cached name does not make them a current member or a picker candidate. */
import { expect, it, vi } from 'vitest';

import { openFirstBoard, setup } from './support.ts';

it('learns the departed board creator by identifier without offering them among people', async () => {
  const made = setup();
  const { world, backend, other, workspace } = made;
  const stored = [...world.containers.values()][0];
  if (!stored) throw new Error('no board');
  stored.createdBy = other.userId;
  world.members.set(
    stored.id,
    (world.members.get(stored.id) ?? []).filter((member) => member.userId !== other.userId),
  );
  vi.spyOn(backend, 'users').mockResolvedValue(
    [...world.users.values()].filter((user) => user.id !== other.userId),
  );
  const byId = vi
    .spyOn(backend, 'usersById')
    .mockImplementation(async (_workspaceId, ids) =>
      [...world.users.values()].filter((user) => ids.includes(user.id)),
    );
  const { board } = await openFirstBoard(made);
  await vi.waitFor(() => {
    expect(workspace.user(other.userId)).toEqual(world.users.get(other.userId));
  });
  expect(byId).toHaveBeenCalledWith(workspace.info?.id, [other.userId]);
  expect(board.people.some((user) => user.id === other.userId)).toBe(false);
  expect(workspace.userName(other.userId)).toBeTruthy();
});

it('does not ask for an empty creator identifier on an imported board', async () => {
  const made = setup();
  for (const board of made.world.containers.values()) board.createdBy = '';
  const byId = vi.spyOn(made.backend, 'usersById');
  await openFirstBoard(made);
  expect(byId).not.toHaveBeenCalled();
});
