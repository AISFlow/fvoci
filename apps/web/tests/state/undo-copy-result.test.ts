/** A menu must not open the identifier of a copy whose undo was already requested. */
import { expect, it, vi } from 'vitest';

import { openFirstBoard } from './support.ts';

it.each([false, true])(
  'does not open a copy undone during its reading (redo: %s)',
  async (redo) => {
    const { board, backend, workspace } = await openFirstBoard();
    const source = board.realCards[0];
    if (!source) throw new Error('missing card');
    await board.showCard(source.id);
    const duplicate = backend.duplicateCard.bind(backend);
    let release = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.spyOn(backend, 'duplicateCard').mockImplementationOnce(async (...args) => {
      await gate;
      return duplicate(...args);
    });
    const opened: string[] = [];
    try {
      const copying = board.duplicateCard(source.id).then((copy) => {
        // The card menu navigates to the result; table menus focus its title.
        if (copy) opened.push(copy.id);
      });
      const undoing = workspace.undoLast();
      const redoing = redo ? workspace.redoLast() : Promise.resolve();
      release();
      await Promise.all([copying, undoing, redoing]);
      expect(opened).toEqual([]);
      expect(board.openCard?.cardId).toBe(source.id);
      expect(board.realCards).toHaveLength(redo ? 4 : 3);
      expect(workspace.problems).toEqual([]);
    } finally {
      release();
      workspace.close();
    }
  },
);
