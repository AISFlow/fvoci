/** Finding boards by title in memory, as the server's `searchWorkspaceBoards` does. */
import type { Container } from '../../logic/wire.ts';

import { copy } from './shapes.ts';
import type { World } from './world.ts';

/**
 * As the server matches: every word of the term in the title, whatever the
 * case; oldest first. Among the boards of the workspace the backend in memory
 * lists (the reader's): no templates, and no wikis, which the server does not
 * search.
 */
export function searchBoardsOf(
  world: World,
  userId: string,
  workspaceId: string,
  query: string,
): Container[] {
  const words = query
    .toLowerCase()
    .split(' ')
    .filter((word) => word !== '');
  if (words.length === 0) return [];
  const isMember = (boardId: string): boolean =>
    (world.members.get(boardId) ?? []).some((member) => member.userId === userId);
  return [...world.containers.values()]
    .filter(
      (board) =>
        !board.isTemplate &&
        (board.kind ?? 'board') === 'board' &&
        board.workspaceId === workspaceId &&
        isMember(board.id) &&
        words.every((word) => board.title.toLowerCase().includes(word)),
    )
    .sort((first, second) => first.createdAt - second.createdAt)
    .map(copy);
}
