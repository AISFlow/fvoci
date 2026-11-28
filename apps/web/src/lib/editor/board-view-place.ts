/**
 * Where a board view may be put in a body (board-view.ts): where a table may
 * stand, in the document and in what a toggle hides, and while the body shows
 * fewer than ten (`MAX_BOARD_VIEWS`, as the server counts them).
 */
import type { Node as ProseMirrorNode } from '@tiptap/pm/model';
import { NodeSelection, type Selection } from '@tiptap/pm/state';

import { MAX_BOARD_VIEWS } from './schema.ts';

export const BOARD_VIEW = 'boardView';

/** How many board views the document shows. */
export function boardViewCount(doc: ProseMirrorNode): number {
  let count = 0;
  doc.descendants((node) => {
    if (node.type.name === BOARD_VIEW) count += 1;
    // A board view stands among blocks: nothing inline holds one.
    return !node.isTextblock;
  });
  return count;
}

/** The nodes a board view stands in. */
const HOLDERS = new Set(['doc', 'detailsContent']);

/**
 * Where a board view of the selection goes: over an empty line of the
 * document or of a toggle, after a block there that is not empty (or that
 * is selected), and nowhere else (in a list, a quote, a callout, a cell).
 */
export function boardViewPlace(selection: Selection): { from: number; to: number } | undefined {
  if (selection instanceof NodeSelection) {
    const { $from } = selection;
    return HOLDERS.has($from.parent.type.name)
      ? { from: selection.to, to: selection.to }
      : undefined;
  }
  const { $from } = selection;
  if ($from.depth < 1 || !$from.parent.isTextblock) return undefined;
  if (!HOLDERS.has($from.node($from.depth - 1).type.name)) return undefined;
  const [before, after] = [$from.before(), $from.after()];
  const empty = $from.parent.type.name === 'paragraph' && $from.parent.content.size === 0;
  return empty ? { from: before, to: after } : { from: after, to: after };
}

/** Whether a board view can be put where the selection is now. */
export function canPutBoardView(doc: ProseMirrorNode, selection: Selection): boolean {
  return boardViewCount(doc) < MAX_BOARD_VIEWS && boardViewPlace(selection) !== undefined;
}
