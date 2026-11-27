/**
 * A write whose answer was lost (status 0) but which the server stored is
 * a change like any other: it stays on the screen, and undo takes it back.
 * One that was not stored is taken back as a refused one.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, type Backend } from '$lib/state/index.ts';

import { openFirstBoard } from './support.ts';

type Lost = 'patchContainer' | 'patchCard' | 'patchView';

/** The next call of `method` reaches the backend (when `stored`), and its answer is lost. */
function loseAnswer(backend: Backend, method: Lost, stored: boolean): void {
  const real = backend[method].bind(backend) as (...args: unknown[]) => Promise<unknown>;
  vi.spyOn(backend, method).mockImplementationOnce(async (...args: unknown[]) => {
    if (stored) await real(...args);
    throw new BackendError(0, { detail: 'no answer' });
  });
}

async function cases() {
  const opened = await openFirstBoard();
  const { board, backend } = opened;
  const card = board.realCards[0];
  const view = board.views[0];
  if (!card || !view) throw new Error('the seed board has no card or view');
  const kinds = {
    patchContainer: {
      change: () => board.changeBoard((current) => ({ ...current, title: 'Changed' })),
      shown: () => board.board?.title,
      stored: async () => (await backend.container(board.boardId)).title,
      before: board.board?.title,
    },
    patchCard: {
      change: () => board.setCardTitle(card.id, 'Changed'),
      shown: () => board.card(card.id)?.title,
      stored: async () => (await backend.card(board.boardId, card.id)).title,
      before: card.title,
    },
    patchView: {
      change: () => board.changeView(view.id, (current) => ({ ...current, title: 'Changed' })),
      shown: () => board.view(view.id)?.title,
      stored: async () =>
        (await backend.views(board.boardId)).find((one) => one.id === view.id)?.title,
      before: view.title,
    },
  };
  return { ...opened, kinds };
}

describe.each(['patchContainer', 'patchCard', 'patchView'] as const)('%s', (method) => {
  it('stored, though its answer was lost, stays and is undone', async () => {
    const { backend, workspace, kinds } = await cases();
    const kind = kinds[method];
    const report = vi.spyOn(workspace, 'report');
    try {
      loseAnswer(backend, method, true);
      await kind.change();
      expect([kind.shown(), await kind.stored()]).toEqual(['Changed', 'Changed']);
      expect(report).not.toHaveBeenCalled();
      await workspace.undoLast();
      expect([kind.shown(), await kind.stored()]).toEqual([kind.before, kind.before]);
    } finally {
      workspace.close();
    }
  });

  it('not stored, its answer lost, is taken back as refused', async () => {
    const { backend, workspace, kinds } = await cases();
    const kind = kinds[method];
    try {
      loseAnswer(backend, method, false);
      await kind.change();
      expect([kind.shown(), await kind.stored()]).toEqual([kind.before, kind.before]);
      expect(workspace.undo.canUndo).toBe(false);
    } finally {
      workspace.close();
    }
  });
});

/** `value` with the keys of its objects in order, as the server's JSON has them. */
function sortedKeys<T>(value: T): T {
  if (Array.isArray(value)) return value.map(sortedKeys) as T;
  if (typeof value !== 'object' || value === null) return value;
  const entries = Object.keys(value)
    .sort()
    .map((key) => [key, sortedKeys((value as Record<string, unknown>)[key])]);
  return Object.fromEntries(entries) as T;
}

describe('a lost answer on the real server (found by a review of b0dec09e)', () => {
  it('finds a definition stored though the server orders its keys', async () => {
    const { board, backend, workspace } = await cases();
    const read = backend.container.bind(backend);
    vi.spyOn(backend, 'container').mockImplementation(async (boardId) =>
      sortedKeys(await read(boardId)),
    );
    const report = vi.spyOn(workspace, 'report');
    try {
      loseAnswer(backend, 'patchContainer', true);
      const added = { id: 'added', name: 'Added', type: 'select' as const, options: [] };
      await board.changeBoard((current) => ({
        ...current,
        cardProperties: [
          ...current.cardProperties,
          { ...added, options: [{ id: 'o', label: 'One', color: 'red' }] },
        ],
      }));
      expect(board.board?.cardProperties.map((property) => property.id)).toContain('added');
      expect(report).not.toHaveBeenCalled();
      expect(workspace.undo.canUndo).toBe(true);
    } finally {
      workspace.close();
    }
  });

  it('takes a write for stored when a later change of its field was stored before the reading', async () => {
    const { board, backend, workspace, kinds } = await cases();
    const card = board.realCards[0];
    if (!card) throw new Error('no card');
    const report = vi.spyOn(workspace, 'report');
    let lose = (): void => undefined;
    const real = backend.patchCard.bind(backend);
    vi.spyOn(backend, 'patchCard').mockImplementationOnce(async (...args) => {
      await real(...args);
      // The answer is lost only once the later write is stored.
      await new Promise<void>((resolve) => {
        lose = resolve;
      });
      throw new BackendError(0, { detail: 'no answer' });
    });
    try {
      const first = board.setCardTitle(card.id, 'X');
      await board.setCardTitle(card.id, 'Y');
      lose();
      await first;
      expect(report).not.toHaveBeenCalled();
      await workspace.undoLast();
      expect([kinds.patchCard.shown(), await kinds.patchCard.stored()]).toEqual(['X', 'X']);
      await workspace.undoLast();
      expect([kinds.patchCard.shown(), await kinds.patchCard.stored()]).toEqual([
        card.title,
        card.title,
      ]);
    } finally {
      lose();
      workspace.close();
    }
  });
});

it('takes a write for stored when its value was changed and changed back before the reading (found by a review of 0d6099ed)', async () => {
  const { board, backend, workspace, kinds } = await cases();
  const card = board.realCards[0];
  if (!card) throw new Error('no card');
  const report = vi.spyOn(workspace, 'report');
  let lose = (): void => undefined;
  let release = (): void => undefined;
  const real = backend.patchCard.bind(backend);
  vi.spyOn(backend, 'patchCard')
    .mockImplementationOnce(async (...args) => {
      await real(...args);
      await new Promise<void>((resolve) => {
        lose = resolve;
      });
      throw new BackendError(0, { detail: 'no answer' });
    })
    .mockImplementationOnce(real)
    // The change back reaches the server only after the reading.
    .mockImplementationOnce(async (...args) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return real(...args);
    });
  try {
    const first = board.setCardTitle(card.id, 'X');
    await board.setCardTitle(card.id, 'Y');
    const back = board.setCardTitle(card.id, 'X');
    lose();
    await first;
    release();
    await back;
    expect(report).not.toHaveBeenCalled();
    await workspace.undoLast();
    await workspace.undoLast();
    expect([kinds.patchCard.shown(), await kinds.patchCard.stored()]).toEqual(['X', 'X']);
  } finally {
    lose();
    release();
    workspace.close();
  }
});
