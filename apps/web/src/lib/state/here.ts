/**
 * Where the reader is, for what an answer leads to. A screen that takes the
 * reader somewhere once the backend has answered (to the board it made, a
 * card it made or copied, a view it made or copied, the welcome board) does
 * so only while the reader is still where they were when they asked: in the
 * same entry of the workspace, and for something of a board, on that same
 * board. One who moved on meanwhile is not taken back, nor one who went
 * elsewhere and came back; what was made is there all the same.
 *
 * Every such place goes through `whileHere`: the one rule.
 */
import { BoardStore } from './board.svelte.ts';
import type { Workspace } from './workspace.svelte.ts';

/**
 * What `work` answers, if the reader is still in the workspace `place` is of
 * (and, for a board, still on it) when the answer arrives; else undefined.
 */
export async function whileHere<T>(
  place: Workspace | BoardStore,
  work: () => Promise<T>,
): Promise<T | undefined> {
  const workspace = place instanceof BoardStore ? place.workspace : place;
  const entered = workspace.info;
  const visit = place instanceof BoardStore ? place.visit : undefined;
  const answer = await work();
  const left =
    workspace.info !== entered ||
    (place instanceof BoardStore && (workspace.board !== place || place.visit !== visit));
  return left ? undefined : answer;
}
