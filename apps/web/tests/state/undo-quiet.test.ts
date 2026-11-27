/**
 * Undo and redo write what was said before, and tell nobody of it again: a
 * card made again by redo is made quietly (its writer watches it all the
 * same, as the server makes them), and a change of a card taken back or made
 * again is sent quietly. The first making and the first change are told.
 */
import { afterEach, expect, it, vi } from 'vitest';

import { openFirstBoard } from './support.ts';

afterEach(() => {
  vi.restoreAllMocks();
});

it('makes a card again quietly when redo makes it again', async () => {
  const { board, backend, workspace } = await openFirstBoard();
  const made = vi.spyOn(backend, 'createCard');
  try {
    await board.createCard({ title: 'Plan' });
    await workspace.undoLast();
    await workspace.redoLast();
    expect(made.mock.calls.map(([, draft]) => draft.quiet ?? false)).toEqual([false, true]);
    expect(board.realCards.filter((card) => card.title === 'Plan')).toHaveLength(1);
    expect(workspace.problems).toEqual([]);
  } finally {
    workspace.close();
  }
});

it('changes a card quietly when undo takes the change back and redo makes it again', async () => {
  const { board, backend, workspace } = await openFirstBoard();
  const card = board.realCards[0];
  if (!card) throw new Error('missing card');
  const changed = vi.spyOn(backend, 'patchCard');
  try {
    await board.setCardTitle(card.id, 'Renamed');
    await workspace.undoLast();
    await workspace.redoLast();
    expect(changed.mock.calls.map(([, , patch, telling]) => [patch.title, telling])).toEqual([
      ['Renamed', undefined],
      [card.title, { quiet: true }],
      ['Renamed', { quiet: true }],
    ]);
    expect(board.card(card.id)?.title).toBe('Renamed');
    expect(workspace.problems).toEqual([]);
  } finally {
    workspace.close();
  }
});

it('duplicates a card atomically on the backend, and again on redo', async () => {
  const { board, backend, workspace } = await openFirstBoard();
  const card = board.realCards[0];
  if (!card) throw new Error('missing card');
  const duplicated = vi.spyOn(backend, 'duplicateCard');
  const created = vi.spyOn(backend, 'createCard');
  try {
    await board.duplicateCard(card.id);
    await workspace.undoLast();
    await workspace.redoLast();
    expect(duplicated).toHaveBeenCalledTimes(2);
    expect(created).not.toHaveBeenCalled();
    expect(workspace.problems).toEqual([]);
  } finally {
    workspace.close();
  }
});
