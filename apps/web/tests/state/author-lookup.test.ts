/** Author lookups share requests across screens, stay bounded and follow workspace visits. */
import { assert, expect, it, vi } from 'vitest';

import { BackendError, type User } from '$lib/state/backend.ts';
import { openFirstBoard, setup, text } from './support.ts';

it('names a departed comment author on opening, refreshing and receiving comments', async () => {
  const made = setup();
  const { world, other, workspace, backend } = made;
  const stored = [...world.containers.values()][0];
  assert(stored);
  const firstCard = world.blocksOf(stored.id).find((block) => block.type === 'card');
  assert(firstCard);
  const cardId = firstCard.id;
  const posted = await other.createComment(stored.id, cardId, text('Kept words'));
  world.workspaceMembers.delete(other.userId);
  world.members.set(
    stored.id,
    (world.members.get(stored.id) ?? []).filter((m) => m.userId !== other.userId),
  );
  const byId = vi.spyOn(backend, 'usersById');
  const { board } = await openFirstBoard(made);
  const card = await board.showCard(cardId);
  await vi.waitFor(() => {
    expect(workspace.userName(other.userId)).toBeTruthy();
  });
  expect(card.comments.find((comment) => comment.id === posted.id)).toBeDefined();
  expect(workspace.workspaceUsers.some((user) => user.id === other.userId)).toBe(false);
  const departed = world.users.get(other.userId);
  assert(departed);
  expect(await backend.searchUsers(world.workspace.id, departed.username)).toEqual([]);
  expect(board.people.some((user) => user.id === other.userId)).toBe(false);
  const calls = byId.mock.calls.length;
  await card.refresh();
  card.receive(posted);
  await board.showCard(cardId);
  expect(byId).toHaveBeenCalledTimes(calls);
});

it('splits 401 unique missing names into bounded requests and shares overlapping requests', async () => {
  const { workspace, backend } = await openFirstBoard();
  const prototype = workspace.users[0];
  assert(prototype);
  const users = Array.from({ length: 401 }, (_, index) => ({
    ...prototype,
    id: `author-${index}`,
  }));
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const byId = vi.spyOn(backend, 'usersById').mockImplementation(async (_id, ids) => {
    await gate;
    return users.filter((user) => ids.includes(user.id));
  });
  const ids = users.map((user) => user.id);
  const first = workspace.learnUsers([...ids, ...ids, '', prototype.id]);
  const second = workspace.learnUsers(ids);
  expect(byId.mock.calls.map(([, asked]) => asked.length)).toEqual([200, 200, 1]);
  release();
  await Promise.all([first, second]);
  expect(users.every((user) => workspace.user(user.id))).toBe(true);
  await workspace.learnUsers(ids);
  expect(byId).toHaveBeenCalledTimes(3);
});

it('retries a failed lookup, keeps nothing it did not ask for, and asks once a visit for a name left out', async () => {
  const { workspace, backend, other, world } = await openFirstBoard();
  const without = (): void => {
    workspace.users = workspace.users.filter((user) => user.id !== other.userId);
  };
  without();
  const byId = vi.spyOn(backend, 'usersById').mockRejectedValueOnce(new BackendError(503, {}));
  await workspace.learnUsers([other.userId]);
  expect(workspace.user(other.userId)).toBeUndefined();
  const known = workspace.users[0];
  assert(known);
  const unrelated = { ...known, id: 'unrequested' };
  byId.mockResolvedValueOnce([unrelated]);
  await workspace.learnUsers([other.userId]);
  expect(workspace.user('unrequested')).toBeUndefined();
  expect(byId).toHaveBeenCalledTimes(2);
  // Left out (an account that was closed, say): every screen that names them asks no more.
  await workspace.learnUsers([other.userId]);
  await workspace.learnUsers([other.userId]);
  expect(byId).toHaveBeenCalledTimes(2);
  expect(workspace.user(other.userId)).toBeUndefined();
  // The next visit asks again.
  await workspace.enter('another-workspace');
  await workspace.enter(world.workspace.id);
  without();
  await workspace.learnUsers([other.userId]);
  expect(byId).toHaveBeenCalledTimes(3);
  expect(workspace.user(other.userId)).toBeDefined();
});

it('signs out on a session that ended while a lookup was pending, though another workspace is shown', async () => {
  const { workspace, backend, other } = await openFirstBoard();
  workspace.users = workspace.users.filter((user) => user.id !== other.userId);
  let refuse!: (error: unknown) => void;
  vi.spyOn(backend, 'usersById').mockImplementation(
    () =>
      new Promise((_resolve, reject) => {
        refuse = reject;
      }),
  );
  const report = vi.spyOn(workspace, 'report');
  const reading = workspace.learnUsers([other.userId]);
  await workspace.enter('another-workspace');
  refuse(new BackendError(401, {}));
  await reading;
  expect(report).toHaveBeenCalledWith('load', expect.objectContaining({ status: 401 }));
});

it('does not bring a departed author response into a later workspace visit', async () => {
  const { workspace, backend, world, other } = await openFirstBoard();
  const user = world.users.get(other.userId);
  assert(user);
  workspace.users = workspace.users.filter((known) => known.id !== other.userId);
  let release!: (users: User[]) => void;
  vi.spyOn(backend, 'usersById').mockImplementation(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const reading = workspace.learnUsers([other.userId]);
  await workspace.enter('another-workspace');
  release([user]);
  await reading;
  expect(workspace.users).toEqual([]);
});

it('asks again for a name left out earlier in the visit once its person is made a member', async () => {
  const { workspace, backend, world, other, board } = await openFirstBoard();
  // They wrote nothing the reader reads: the server has no reason to name them.
  for (const stored of world.containers.values()) {
    if (stored.createdBy === other.userId) stored.createdBy = backend.userId;
    for (const block of world.blocksOf(stored.id)) {
      if (block.createdBy === other.userId) block.createdBy = backend.userId;
      if (block.modifiedBy === other.userId) block.modifiedBy = backend.userId;
    }
  }
  for (const versions of world.commentVersions.values())
    for (const version of versions)
      if (version.writtenBy === other.userId) version.writtenBy = backend.userId;
  world.workspaceMembers.delete(other.userId);
  world.members.set(
    board.boardId,
    (world.members.get(board.boardId) ?? []).filter((member) => member.userId !== other.userId),
  );
  workspace.users = workspace.users.filter((user) => user.id !== other.userId);
  const byId = vi.spyOn(backend, 'usersById');
  await workspace.learnUsers([other.userId]);
  expect(byId).toHaveBeenCalledTimes(1);
  expect(workspace.user(other.userId)).toBeUndefined();
  // Let in again, and made a member of the board without the workspace being read again.
  world.workspaceMembers.add(other.userId);
  await board.addMember(other.userId, 'editor');
  await vi.waitFor(() => {
    expect(workspace.user(other.userId)).toBeDefined();
  });
  expect(byId).toHaveBeenCalledTimes(2);
  expect(board.people.some((user) => user.id === other.userId)).toBe(true);
});
