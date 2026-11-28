/** The memory directory distinguishes current members from the authors of readable work. */
import { emptyName, nameContains } from '../../logic/person-names.ts';
import { isGone } from '../../logic/users.ts';
import type { User } from '../backend.ts';

import { isOpenWiki } from './container-access.ts';
import { roleOn } from './comments.ts';
import { fail } from './refuse.ts';
import { workersOf } from './spaces.ts';
import { copy, scramble } from './shapes.ts';
import type { World } from './world.ts';

export function seedUsers(world: World, users: readonly User[]): void {
  for (const user of users) {
    world.users.set(user.id, user);
    world.workspaceMembers.add(user.id);
  }
}

/**
 * Whether a search for `wanted`, in small letters, finds the user, as the
 * server finds them: in the login name or the name (`nameContains`).
 */
export function userFound(user: User, wanted: string): boolean {
  return (
    user.username.toLowerCase().includes(wanted) ||
    user.displayName.toLowerCase().includes(wanted) ||
    nameContains(user.name ?? emptyName(), wanted)
  );
}

/** The first ten of `users` a search for `query` finds (`userFound`), the oldest accounts first. */
export function usersMatching(users: readonly User[], query: string): User[] {
  const wanted = query.trim().toLowerCase();
  return users
    .filter((user) => userFound(user, wanted))
    .sort((first, second) => first.createdAt - second.createdAt)
    .slice(0, 10);
}

function listed(user: User): User {
  const { isAdmin: _isAdmin, ...rest } = copy(user);
  return rest;
}

/**
 * Deletes the account of a user, as the server erases one: it is in the
 * workspace and on its boards no more, nothing of the person is left (nor
 * when the account came or went: its times are zero), and what they wrote
 * stays under the identifier.
 */
export function eraseUser(world: World, userId: string): void {
  const user = world.users.get(userId);
  if (!user) return;
  const { deletedAt: _deletedAt, ...rest } = user;
  world.users.set(userId, {
    ...rest,
    username: '',
    displayName: '',
    name: emptyName(),
    email: '',
    isAdmin: false,
    createdAt: 0,
    updatedAt: 0,
  });
  world.workspaceMembers.delete(userId);
  world.settings.delete(userId);
  world.examScores.delete(userId);
  forgetReactionsOf(world, userId);
  for (const [boardId, members] of world.members) {
    const next = members.filter((member) => member.userId !== userId);
    world.members.set(boardId, next);
    if (next.length !== members.length) {
      world.tellContainer(boardId, { kind: 'members', containerId: boardId, members: copy(next) });
    }
  }
  world.tellUsers([...world.workspaceMembers], { kind: 'user', userId });
}

/**
 * Takes the reactions of an account erased off every block, as the server's
 * rows go with it: those of the blocks in a trash too, which are kept under
 * the block. Nobody is told, as the server tells nobody.
 */
function forgetReactionsOf(world: World, userId: string): void {
  for (const [blockId, reactions] of world.reactions) {
    if (!reactions.some((reaction) => reaction.userIds.includes(userId))) continue;
    const kept = reactions
      .map((reaction) => ({ ...reaction, userIds: reaction.userIds.filter((id) => id !== userId) }))
      .filter((reaction) => reaction.userIds.length > 0);
    if (kept.length === 0) world.reactions.delete(blockId);
    else world.reactions.set(blockId, kept);
  }
}

/** The people who work in a workspace (`workersOf`), but who are gone. */
export function usersAmong(world: World, workers: ReadonlySet<string>): User[] {
  return scramble([...world.users.values()])
    .filter((user) => workers.has(user.id) && !isGone(user) && user.deletedAt === undefined)
    .map(listed);
}

export function usersNamed(
  world: World,
  readerId: string,
  workspaceId: string,
  userIds: readonly string[],
): User[] {
  const workers = workersOf(world, workspaceId);
  if (!workers.has(readerId)) fail(404, 'no such workspace');
  const visible = new Set(workers);
  for (const board of world.containers.values()) {
    if (board.workspaceId !== workspaceId) continue;
    const role = roleOn(world.members.get(board.id) ?? [], readerId);
    if (!role && !(board.isTemplate && board.visibility === 'open') && !isOpenWiki(board)) continue;
    visible.add(board.createdBy);
    for (const block of world.blocksOf(board.id)) {
      visible.add(block.createdBy);
      visible.add(block.modifiedBy);
      if (block.type === 'comment') {
        for (const version of world.commentVersions.get(block.id) ?? [])
          visible.add(version.writtenBy);
      }
    }
    for (const deleted of world.deletedBlocks.values()) {
      if (deleted.containerId !== board.id) continue;
      for (const block of deleted.blocks) {
        if (block.type !== 'comment') continue;
        for (const version of world.commentVersions.get(block.id) ?? [])
          visible.add(version.writtenBy);
      }
    }
    for (const meeting of world.courseMeetings.get(board.id) ?? []) {
      if (meeting.presenterId) visible.add(meeting.presenterId);
      if (meeting.hostId) visible.add(meeting.hostId);
      for (const row of meeting.attendance) visible.add(row.userId);
    }
  }
  return (
    scramble([...world.users.values()])
      // As the server: a closed account is named by nobody, though its work stays; one
      // that was deleted is answered as what is left of it, to whoever asks.
      .filter(
        (user) =>
          userIds.includes(user.id) &&
          (isGone(user) || (visible.has(user.id) && user.deletedAt === undefined)),
      )
      .map(listed)
  );
}
