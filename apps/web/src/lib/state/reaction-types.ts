/**
 * Reactions (`addReaction`, `removeReaction`): an emoji a member who may
 * comment answers a comment, a card, a page or a unit with. The members read
 * them with the comments and with the card, page or unit itself, and are told
 * of a change (`reactionsChanged`) as all of the block's reactions as they are
 * now. A reader by a public link has none: a reaction names who reacted.
 */

/** One emoji on a block and who answered it with it, the first first. */
export interface Reaction {
  emoji: string;
  userIds: readonly string[];
}

/** What is read with its reactions; none is left out. */
export type WithReactions<T> = T & { reactions?: readonly Reaction[] };

/** The emoji offered first, before the whole list. */
export const QUICK_REACTIONS: readonly string[] = ['👍', '❤️', '🎉', '👀', '🚀', '😄'];

/** How many different emoji a block takes; the server refuses one more (`too_many_reactions`). */
export const REACTION_KINDS_PER_BLOCK = 20;

export interface Reactions {
  /** Answers the block with the emoji as the reader; again is no error. */
  add(workspaceId: string, containerId: string, blockId: string, emoji: string): Promise<void>;
  /** Takes the reader's emoji off the block; one not there is no error. */
  remove(workspaceId: string, containerId: string, blockId: string, emoji: string): Promise<void>;
}

/** Where a backend keeps reactions; none through a public link. */
export interface ReactionsOfBackend {
  readonly reactions: Reactions | undefined;
}

/**
 * The reactions with the reader's `emoji` there (`present`) or not, as the
 * server makes them: an emoji new to the block goes last, the reader last
 * among those who reacted with it, and one whose last reader goes is left out.
 */
export function withMine(
  reactions: readonly Reaction[],
  emoji: string,
  userId: string,
  present: boolean,
): readonly Reaction[] {
  if (hasReacted(reactions, emoji, userId) === present) return reactions;
  if (present && !reactions.some((reaction) => reaction.emoji === emoji)) {
    return [...reactions, { emoji, userIds: [userId] }];
  }
  return reactions
    .map((reaction) => {
      if (reaction.emoji !== emoji) return reaction;
      const userIds = present
        ? [...reaction.userIds, userId]
        : reaction.userIds.filter((id) => id !== userId);
      return { emoji, userIds };
    })
    .filter((reaction) => reaction.userIds.length > 0);
}

/** Whether the reader has answered with `emoji`. */
export function hasReacted(
  reactions: readonly Reaction[],
  emoji: string,
  userId: string | undefined,
): boolean {
  return (
    userId !== undefined &&
    reactions.some((reaction) => reaction.emoji === emoji && reaction.userIds.includes(userId))
  );
}
