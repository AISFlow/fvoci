/**
 * Reactions on the server (`addReaction`, `removeReaction`), and the
 * reactions a read carries (`reactions` of `CommentWithReactions`,
 * `CardWithReactions`, `PageWithReactions`, `UnitWithReactions`).
 */
import type { Reaction, Reactions, WithReactions } from '../state/reaction-types.ts';

import type { Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace `workspaceId`. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options: RequestOptions,
  workspaceId: string,
) => Promise<T>;

const PATH = '/containers/{containerId}/blocks/{blockId}/reactions/{emoji}';

export class ServerReactions implements Reactions {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  async add(workspaceId: string, containerId: string, blockId: string, emoji: string) {
    await this.#ask(PATH, { containerId, blockId, emoji }, { method: 'PUT' }, workspaceId);
  }

  async remove(workspaceId: string, containerId: string, blockId: string, emoji: string) {
    await this.#ask(PATH, { containerId, blockId, emoji }, { method: 'DELETE' }, workspaceId);
  }
}

/** The reactions as the screens keep them. */
export function reactionsOf(wire: readonly Schemas['Reaction'][] | undefined): Reaction[] {
  return (wire ?? []).map(({ emoji, userIds }) => ({ emoji, userIds }));
}

/** `value` with the reactions `read` carried, none left out. */
export function withReactions<T>(
  value: T,
  read: { reactions?: Schemas['Reaction'][] },
): WithReactions<T> {
  const reactions = reactionsOf(read.reactions);
  const carried: { reactions?: readonly Reaction[] } = reactions.length === 0 ? {} : { reactions };
  return { ...value, ...carried };
}
