/**
 * Reactions in the in-memory backend (`Backend.reactions`), as the server
 * keeps them: a member from the commenter's role up answers a comment, a
 * card, a page or a unit with one emoji (`emojiRefusal`, asked before
 * anything else), at most 20 different emoji on a block; a diary is not
 * there. The container's readers are told the block's
 * reactions as they are now (`reactions`). Nothing of the block changes.
 */
import { BackendError, problemOf } from '../backend.ts';
import type { Reaction, Reactions, WithReactions } from '../reaction-types.ts';
import { REACTION_KINDS_PER_BLOCK } from '../reaction-types.ts';

import { memberRole, memoryPermissionsOf } from './container-access.ts';
import { emojiRefusal } from './emoji.ts';
import { fail } from './refuse.ts';
import type { World } from './world.ts';

const REACTED_TO: ReadonlySet<string> = new Set(['comment', 'card', 'page', 'unit']);

/** The reactions to the block, the emoji it was first answered with first. */
export function reactionsIn(world: World, blockId: string): Reaction[] {
  return world.reactions.get(blockId) ?? [];
}

/** `value` with the reactions to it, none left out. */
export function withMemoryReactions<T extends { id: string }>(
  world: World,
  value: T,
): WithReactions<T> {
  const reactions = reactionsIn(world, value.id);
  const carried: { reactions?: readonly Reaction[] } = reactions.length === 0 ? {} : { reactions };
  return { ...value, ...carried };
}

function react(
  world: World,
  userId: string,
  where: { workspaceId: string; containerId: string; blockId: string },
  emoji: string,
  present: boolean,
): void {
  // The emoji of the path first, as the server reads it.
  const refusal = emojiRefusal(emoji);
  if (refusal !== undefined) {
    throw new BackendError(400, {
      ...problemOf(400, refusal),
      errors: [{ parameter: 'emoji', detail: refusal, reason: 'invalid' }],
    });
  }
  // Then the container, as the server reaches it: not there (a diary is not), not
  // read (404), or not commented on (403); then the block of it.
  const container = world.containers.get(where.containerId);
  if (container?.workspaceId !== where.workspaceId || container.kind === 'diary') {
    fail(404, 'board not found');
  }
  memberRole(world, userId, container.id, 'any');
  if (!memoryPermissionsOf(world, userId, container.id).canComment) {
    fail(403, 'access denied to react in the container');
  }
  const block = world.blocks.get(where.blockId);
  if (block?.containerId !== container.id || !REACTED_TO.has(block.type)) {
    fail(404, 'block not found');
  }
  const reactions = reactionsIn(world, block.id);
  const found = reactions.find((reaction) => reaction.emoji === emoji);
  if (present === Boolean(found?.userIds.includes(userId))) return;
  let next: Reaction[];
  if (!present) {
    next = reactions
      .map((reaction) =>
        reaction.emoji === emoji
          ? { emoji, userIds: reaction.userIds.filter((id) => id !== userId) }
          : reaction,
      )
      .filter((reaction) => reaction.userIds.length > 0);
  } else if (found) {
    next = reactions.map((reaction) =>
      reaction === found ? { emoji, userIds: [...found.userIds, userId] } : reaction,
    );
  } else if (reactions.length >= REACTION_KINDS_PER_BLOCK) {
    fail(400, `a block is answered with at most ${REACTION_KINDS_PER_BLOCK} different emoji`, {
      reason: 'too_many_reactions',
      limit: REACTION_KINDS_PER_BLOCK,
    });
  } else {
    next = [...reactions, { emoji, userIds: [userId] }];
  }
  if (next.length === 0) world.reactions.delete(block.id);
  else world.reactions.set(block.id, next);
  world.tellReaders(container.id, {
    kind: 'reactions',
    containerId: container.id,
    blockId: block.id,
    reactions: next,
  });
}

/** `Backend.reactions` of the world: the reactions of the reader `of.userId`. */
export class MemoryReactions implements Reactions {
  readonly #of: { readonly world: World; readonly userId: string };
  readonly #enter: () => Promise<void>;

  constructor(of: { readonly world: World; readonly userId: string }, enter: () => Promise<void>) {
    this.#of = of;
    this.#enter = enter;
  }

  async add(workspaceId: string, containerId: string, blockId: string, emoji: string) {
    await this.#enter();
    react(this.#of.world, this.#of.userId, { workspaceId, containerId, blockId }, emoji, true);
  }

  async remove(workspaceId: string, containerId: string, blockId: string, emoji: string) {
    await this.#enter();
    react(this.#of.world, this.#of.userId, { workspaceId, containerId, blockId }, emoji, false);
  }
}
