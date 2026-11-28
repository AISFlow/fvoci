/**
 * Cards deleted together where one is under another (`deleteBoardCards`),
 * as the bar of a selection deletes the rows of a table: one undo brings
 * them back as they were, the child under its parent, in whichever order
 * they were selected. A card goes with what it is under; only the cards
 * with nothing selected above them are sent, read again when it is redone.
 */
import { describe, expect, it, vi } from 'vitest';

import { openFirstBoard, settle } from './support.ts';

describe('a card and the card it is under, deleted together', () => {
  it.each([{ order: 'the child first' }, { order: 'the parent first' }])(
    'come back with one undo as they were, $order',
    async ({ order }) => {
      const { board, backend, workspace } = await openFirstBoard();
      const alpha = board.realCards.find((card) => card.title === 'Alpha');
      if (!alpha) throw new Error('no Alpha');
      const child = await backend.createCard(board.boardId, { title: 'Child', parentId: alpha.id });
      await settle();
      expect(board.card(child.id)?.parentId).toBe(alpha.id);
      const sent = vi.spyOn(backend, 'batchDeleteCards');

      const ids = order === 'the child first' ? [child.id, alpha.id] : [alpha.id, child.id];
      await board.deleteCards(ids);
      expect(sent).toHaveBeenCalledExactlyOnceWith(board.boardId, [alpha.id]);
      expect(board.realCards.map((card) => card.title)).toEqual(['Beta', 'Gamma']);

      await workspace.undoLast();
      expect(workspace.problems).toEqual([]);
      expect(board.card(child.id)?.parentId).toBe(alpha.id);
      expect((await backend.card(board.boardId, child.id)).parentId).toBe(alpha.id);
      expect(board.realCards.map((card) => card.title).sort()).toEqual(
        ['Alpha', 'Beta', 'Child', 'Gamma'].sort(),
      );
    },
  );
});

describe('a deletion of cards, redone', () => {
  it('reads again which are under which: a child moved from under its parent goes on its own', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no Alpha');
    const child = await backend.createCard(board.boardId, { title: 'Child', parentId: alpha.id });
    await settle();
    await board.deleteCards([child.id, alpha.id]);
    await workspace.undoLast();
    // Someone else moves the child to the top of the board meanwhile.
    await backend.patchCard(board.boardId, child.id, { parentId: null });
    await settle();
    expect(board.card(child.id)?.parentId).toBe('');
    const sent = vi.spyOn(backend, 'batchDeleteCards');

    await workspace.redoLast();
    expect(workspace.problems).toEqual([]);
    expect(sent).toHaveBeenCalledExactlyOnceWith(board.boardId, [child.id, alpha.id]);
    expect(board.realCards.map((card) => card.title)).toEqual(['Beta', 'Gamma']);
  });
});

describe('the memory backend, given a card and the card it is under to restore', () => {
  it('brings back the family once, and skips the child its parent brought back, as the server does', async () => {
    const { board, backend } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no Alpha');
    const child = await backend.createCard(board.boardId, { title: 'Child', parentId: alpha.id });
    await backend.batchDeleteCards(board.boardId, [alpha.id, child.id]);
    const restored = await backend.batchRestoreCards(board.boardId, [alpha.id, child.id]);
    expect(restored.map((card) => card.id).sort()).toEqual([alpha.id, child.id].sort());
    expect((await backend.card(board.boardId, child.id)).parentId).toBe(alpha.id);
  });

  it('changes nothing where one of the cards is in no trash', async () => {
    const { board, backend } = await openFirstBoard();
    const [alpha, beta] = board.realCards;
    if (!alpha || !beta) throw new Error('no cards');
    await backend.batchDeleteCards(board.boardId, [alpha.id]);
    await expect(
      backend.batchRestoreCards(board.boardId, [alpha.id, beta.id]),
    ).rejects.toMatchObject({ status: 404 });
    await expect(backend.card(board.boardId, alpha.id)).rejects.toMatchObject({ status: 404 });
  });
});
