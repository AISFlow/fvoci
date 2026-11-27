/** Identifier lookups can name departed writers only through work this reader can read. */
import { assert, expect, it } from 'vitest';

import { setup, text } from './support.ts';

it('names a departed card author but not an unrelated departed person or a foreign board author', async () => {
  const { backend, world, other } = setup();
  const board = [...world.containers.values()][0];
  assert(board);
  const card = world.blocksOf(board.id).find((block) => block.type === 'card');
  assert(card);
  const unrelated = [...world.users.values()].find(
    (user) => ![backend.userId, other.userId].includes(user.id),
  );
  assert(unrelated);
  for (const block of world.blocksOf(board.id)) {
    block.createdBy = backend.userId;
    block.modifiedBy = backend.userId;
  }
  board.createdBy = backend.userId;
  card.createdBy = other.userId;
  world.workspaceMembers.delete(other.userId);
  world.workspaceMembers.delete(unrelated.id);
  world.containers.set('foreign', {
    ...board,
    id: 'foreign',
    workspaceId: 'elsewhere',
    createdBy: unrelated.id,
  });
  world.members.set('foreign', world.members.get(board.id) ?? []);
  const ids = [other.userId, unrelated.id];
  expect((await backend.usersById(world.workspace.id, ids)).map((user) => user.id)).toEqual([
    other.userId,
  ]);
  expect((await backend.users(world.workspace.id)).some((user) => ids.includes(user.id))).toBe(
    false,
  );
  world.members.set(board.id, []);
  expect(await backend.usersById(world.workspace.id, ids)).toEqual([]);
  await expect(backend.usersById('elsewhere', ids)).rejects.toMatchObject({ status: 404 });
  expect(await backend.usersById(world.workspace.id, [])).toEqual([]);
  expect(
    (await backend.usersById(world.workspace.id, Array<string>(201).fill(backend.userId))).map(
      (user) => user.id,
    ),
  ).toEqual([backend.userId]);
});

it('names the writer of a kept comment revision, including its readable trash history', async () => {
  const { backend, world, other } = setup();
  const board = [...world.containers.values()][0];
  assert(board);
  const card = world.blocksOf(board.id).find((block) => block.type === 'card');
  assert(card);
  for (const block of world.blocksOf(board.id)) {
    block.createdBy = backend.userId;
    block.modifiedBy = backend.userId;
  }
  const comment = await other.createComment(board.id, card.id, text('Earlier'));
  await backend.patchComment(board.id, comment.id, {
    body: text('Current'),
    readAt: comment.updatedAt,
  });
  const stored = world.blocks.get(comment.id);
  assert(stored);
  stored.createdBy = backend.userId;
  world.workspaceMembers.delete(other.userId);
  world.members.set(
    board.id,
    (world.members.get(board.id) ?? []).filter((m) => m.userId !== other.userId),
  );
  const ids = [other.userId];
  expect((await backend.usersById(world.workspace.id, ids)).map((user) => user.id)).toEqual(ids);
  await backend.deleteBlock(board.id, comment.id);
  expect((await backend.usersById(world.workspace.id, ids)).map((user) => user.id)).toEqual(ids);
  world.commentVersions.delete(comment.id);
  expect(await backend.usersById(world.workspace.id, ids)).toEqual([]);
});

it('names and lists nobody whose account was closed, though their card stays', async () => {
  const { backend, world, other } = setup();
  const board = [...world.containers.values()][0];
  assert(board);
  const card = world.blocksOf(board.id).find((block) => block.type === 'card');
  assert(card);
  card.createdBy = other.userId;
  const ids = [other.userId];
  expect((await backend.usersById(world.workspace.id, ids)).map((user) => user.id)).toEqual(ids);
  const closed = world.users.get(other.userId);
  assert(closed);
  closed.deletedAt = 1;
  expect(await backend.usersById(world.workspace.id, ids)).toEqual([]);
  // Nor among the people of the workspace, nor found: as the server leaves closed accounts out.
  expect((await backend.users(world.workspace.id)).some((user) => user.id === other.userId)).toBe(
    false,
  );
  expect(await backend.searchUsers(world.workspace.id, closed.username)).toEqual([]);
});
