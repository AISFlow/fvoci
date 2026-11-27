/**
 * A refused write of a card or a view takes back only what is still its own
 * on the screen, and an undo that takes back to the value of a write still
 * under way waits for it: it never sends the value the server refused.
 * Found by the review of the whole branch.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError } from '$lib/state/index.ts';

import { openFirstBoard, settle } from './support.ts';

type Method = 'patchContainer' | 'patchCard' | 'patchView';

async function opened() {
  const made = await openFirstBoard();
  const { board, backend } = made;
  const card = board.realCards[0];
  const view = board.views[0];
  if (!card || !view) throw new Error('the seed board has no card or view');
  const kinds = {
    patchContainer: {
      title: (title: string) => board.changeBoard((current) => ({ ...current, title })),
      shown: () => board.board?.title,
      stored: async () => (await backend.container(board.boardId)).title,
    },
    patchCard: {
      title: (title: string) => board.setCardTitle(card.id, title),
      shown: () => board.card(card.id)?.title,
      stored: async () => (await backend.card(board.boardId, card.id)).title,
    },
    patchView: {
      title: (title: string) => board.changeView(view.id, (current) => ({ ...current, title })),
      shown: () => board.view(view.id)?.title,
      stored: async () =>
        (await backend.views(board.boardId)).find((one) => one.id === view.id)?.title,
    },
  };
  return { ...made, kinds };
}

/** The next write of `method` waits for `refuse`, then is refused. */
function refuseLater(backend: Record<Method, unknown>, method: Method) {
  let refuse = (): void => undefined;
  vi.spyOn(
    backend as unknown as Record<Method, () => Promise<unknown>>,
    method,
  ).mockImplementationOnce(
    () =>
      new Promise((_resolve, reject) => {
        refuse = () => {
          reject(new BackendError(403));
        };
      }),
  );
  return () => {
    refuse();
  };
}

describe.each(['patchCard', 'patchView'] as const)('%s refused', (method) => {
  it('keeps a later change on the screen while its write still waits', async () => {
    const { backend, workspace, kinds } = await opened();
    const kind = kinds[method];
    const before = kind.shown();
    const refuse = refuseLater(backend, method);
    let release = (): void => undefined;
    const real = backend[method].bind(backend) as (...args: unknown[]) => Promise<unknown>;
    vi.spyOn(
      backend as unknown as Record<Method, () => Promise<unknown>>,
      method,
    ).mockImplementationOnce(async (...args: unknown[]) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return real(...args);
    });
    try {
      const refused = kind.title('refused');
      const later = kind.title('later');
      refuse();
      await refused;
      expect(kind.shown()).toBe('later');
      release();
      await later;
      expect([kind.shown(), await kind.stored()]).toEqual(['later', 'later']);
      await workspace.undoLast();
      expect([kind.shown(), await kind.stored()]).toEqual([before, before]);
    } finally {
      refuse();
      release();
      workspace.close();
    }
  });
});

describe.each(['patchContainer', 'patchCard', 'patchView'] as const)('%s', (method) => {
  it('undoes a change made over a write refused later to the value stored, never sending the refused one', async () => {
    const { backend, workspace, kinds } = await opened();
    const kind = kinds[method];
    const before = kind.shown();
    const refuse = refuseLater(backend, method);
    try {
      const refused = kind.title('refused');
      await kind.title('later');
      const sent = vi.spyOn(backend as unknown as Record<Method, () => Promise<unknown>>, method);
      const from = sent.mock.calls.length;
      const undoing = workspace.undoLast();
      // The undo would be sent now; it waits for the write its value came from.
      await settle();
      refuse();
      await refused;
      await undoing;
      expect([kind.shown(), await kind.stored()]).toEqual([before, before]);
      expect(JSON.stringify(sent.mock.calls.slice(from))).not.toContain('refused');
    } finally {
      refuse();
      workspace.close();
    }
  });
});

describe('an undo waiting for the write its value came from (found by a review of 0a15e86a)', () => {
  it('is dropped by a change made meanwhile, which stays', async () => {
    const { backend, workspace, kinds } = await opened();
    const kind = kinds.patchContainer;
    const refuse = refuseLater(backend, 'patchContainer');
    try {
      const refused = kind.title('refused');
      await kind.title('later');
      const undoing = workspace.undoLast();
      await settle();
      await kind.title('meanwhile');
      refuse();
      await Promise.all([refused, undoing]);
      expect([kind.shown(), await kind.stored()]).toEqual(['meanwhile', 'meanwhile']);
    } finally {
      refuse();
      workspace.close();
    }
  });

  it('does not keep undo busy for good when that write never answers', async () => {
    const { board, backend, workspace, kinds } = await opened();
    const kind = kinds.patchContainer;
    const never = refuseLater(backend, 'patchContainer');
    const card = board.realCards[0];
    if (!card) throw new Error('no card');
    try {
      void kind.title('unanswered');
      await kind.title('later');
      void workspace.undoLast();
      await settle();
      await board.setCardTitle(card.id, 'changed');
      expect(workspace.undo.busy).toBe(false);
      await workspace.undoLast();
      expect(board.card(card.id)?.title).toBe(card.title);
    } finally {
      never();
      workspace.close();
    }
  });
});
