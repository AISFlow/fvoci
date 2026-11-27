/**
 * Undo and redo on a backend that answers 400ms late, through the board
 * store: Ctrl+Z right after a change takes back that change; a refused
 * change leaves the undo that waited for it nothing to do; a change made
 * while an undo waits drops it; a gesture of several writes is one step
 * from its start.
 */
import { describe, expect, it, vi } from 'vitest';

import { createBoardView } from '$lib/logic/wire.ts';
import { BackendError } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';

import { openFirstBoard, settle, STATUS } from './support.ts';

const LATE = 400;

async function slowBoard() {
  const made = await openFirstBoard();
  const { board, backend } = made;
  const alpha = board.realCards.find((card) => card.title === 'Alpha')?.id ?? '';
  const title = (): string | undefined => board.card(alpha)?.title;
  const stored = async (): Promise<string> => (await backend.card(board.boardId, alpha)).title;
  await board.setCardTitle(alpha, 'One');
  backend.latency = LATE;
  /** The writes of cards asked of the backend from here on. */
  const sent: unknown[] = [];
  const patchCard = backend.patchCard.bind(backend);
  backend.patchCard = (boardId, cardId, patch) => {
    sent.push(patch);
    return patchCard(boardId, cardId, patch);
  };
  return { ...made, alpha, title, stored, sent };
}

describe('undo asked for while a change is being written', () => {
  it('takes back that change, not the one before', async () => {
    const { board, workspace, alpha, title, stored } = await slowBoard();
    const changing = board.setCardTitle(alpha, 'Two');
    const undoing = workspace.undoLast();
    expect(workspace.undoBusy).toBe(true);
    await Promise.all([changing, undoing]);
    expect(title()).toBe('One');
    expect(await stored()).toBe('One');
    expect(workspace.undoBusy).toBe(false);
    // "One" is still in force and the next to undo; "Two" the next to redo.
    expect(workspace.canUndo).toBe(true);
    expect(workspace.canRedo).toBe(true);
  });

  it('is dropped when the change is refused, and the change before stays', async () => {
    const { board, backend, workspace, alpha, title, stored } = await slowBoard();
    backend.failNext('patchCard', new BackendError(403));
    const changing = board.setCardTitle(alpha, 'Two');
    const undoing = workspace.undoLast();
    await Promise.all([changing, undoing]);
    expect(title()).toBe('One');
    expect(await stored()).toBe('One');
    expect(workspace.problems.map((problem) => problem.action)).toEqual(['save']);
    expect(workspace.canUndo).toBe(true);
    expect(workspace.canRedo).toBe(false);
  });

  it('is dropped by a change made while it waits, which is made at once', async () => {
    const { board, workspace, alpha, title, stored } = await slowBoard();
    const renaming = board.setCardTitle(alpha, 'Two');
    const undoing = workspace.undoLast();
    const changing = board.setCardProperty(alpha, STATUS, 'done');
    expect(board.card(alpha)?.fields.properties[STATUS]).toBe('done');
    await Promise.all([renaming, undoing, changing]);
    expect(title()).toBe('Two');
    expect(await stored()).toBe('Two');
    expect(board.card(alpha)?.fields.properties[STATUS]).toBe('done');
    // The steps go on from what is in force: the status, then "Two", then "One".
    await workspace.undoLast();
    expect(board.card(alpha)?.fields.properties[STATUS]).toBe('todo');
    await workspace.undoLast();
    expect(title()).toBe('One');
  });
});

describe('redo asked for while an undo is being written', () => {
  it('waits for it, and does the change again', async () => {
    const { workspace, title, stored, sent } = await slowBoard();
    const undoing = workspace.undoLast();
    const redoing = workspace.redoLast();
    await Promise.all([undoing, redoing]);
    expect(sent).toEqual([{ title: 'Alpha' }, { title: 'One' }]);
    expect(title()).toBe('One');
    expect(await stored()).toBe('One');
  });

  it('is dropped when the undo is refused', async () => {
    const { backend, workspace, title, stored, sent } = await slowBoard();
    backend.failNext('patchCard', new BackendError(403));
    const undoing = workspace.undoLast();
    const redoing = workspace.redoLast();
    await Promise.all([undoing, redoing]);
    // The undo was sent, and the redo was not.
    expect(sent).toEqual([{ title: 'Alpha' }]);
    expect(title()).toBe('One');
    expect(await stored()).toBe('One');
    expect(workspace.problems.map((problem) => problem.action)).toEqual(['undo']);
    expect(workspace.canUndo).toBe(true);
    expect(workspace.canRedo).toBe(false);
  });
});

describe('a gesture of several writes', () => {
  it('is taken back whole by an undo asked for before its writes are done', async () => {
    const { board, backend, workspace, title } = await slowBoard();
    const ids = board.realCards.slice(0, 2).map((card) => card.id);
    const deleting = board.deleteCards(ids);
    const undoing = workspace.undoLast();
    await Promise.all([deleting, undoing]);
    expect(board.realCards.filter((card) => ids.includes(card.id))).toHaveLength(ids.length);
    const kept = await backend.cards(board.boardId);
    expect(kept.filter((card) => ids.includes(card.id))).toHaveLength(ids.length);
    // The title changed before the gesture is still in force.
    expect(title()).toBe('One');
  });
});

describe('a change made while a gesture waits for one of its writes (found by a review of 042ff43)', () => {
  it("is undone and redone in the order it was made among the gesture's changes", async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    const view = board.views[0];
    if (!card || !view) throw new Error('missing card or view');

    let release = (): void => undefined;
    let entered = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    const patchView = backend.patchView.bind(backend);
    backend.patchView = async (...args) => {
      backend.patchView = patchView;
      entered();
      await gate;
      return patchView(...args);
    };

    try {
      // A two-write gesture writes the view's order, then the card's status.
      const dropping = board.inOneStep(async (groupId) => {
        await board.setViewFields(view.id, { cardOrder: [card.id] }, 'moveCard', groupId);
        await board.setCardProperty(card.id, STATUS, 'done', groupId);
      });
      await started;
      await board.setCardProperty(card.id, STATUS, 'doing');
      release();
      await dropping;
      const final = (await backend.card(board.boardId, card.id)).fields.properties[STATUS];
      const stored = async (): Promise<unknown> =>
        (await backend.card(board.boardId, card.id)).fields.properties[STATUS];

      // In the order the changes were made: the drop's write of the status came last.
      await workspace.undoLast();
      expect(await stored()).toBe('doing');
      await workspace.undoLast();
      expect(await stored()).toBe('todo');
      await workspace.redoLast();
      await workspace.redoLast();

      expect(workspace.problems).toEqual([]);
      expect((await backend.card(board.boardId, card.id)).fields.properties[STATUS]).toBe(final);
      expect(board.card(card.id)?.fields.properties[STATUS]).toBe(final);
    } finally {
      release();
      workspace.close();
    }
  });
});

/** Holds the next write of a card until `release`; `started` settles when it is asked. */
function holdNextCardWrite(backend: MemoryBackend, refuse = false) {
  let release = (): void => undefined;
  let entered = (): void => undefined;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const started = new Promise<void>((resolve) => {
    entered = resolve;
  });
  const patchCard = backend.patchCard.bind(backend);
  backend.patchCard = async (...args) => {
    backend.patchCard = patchCard;
    entered();
    await gate;
    if (refuse) throw new BackendError(503);
    return patchCard(...args);
  };
  return { release, started };
}

describe('a change made while an undo or redo that fails is written (found by a review of 042ff43)', () => {
  it.each([
    { action: 'undo' as const, expected: 'One' },
    { action: 'redo' as const, expected: 'Alpha' },
  ])('is undone to the value stored, after a refused $action', async ({ action, expected }) => {
    const { board, backend, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('missing card');
    await board.setCardTitle(card.id, 'One');
    if (action === 'redo') await workspace.undoLast();
    const held = holdNextCardWrite(backend, true);
    try {
      const carrying = action === 'undo' ? workspace.undoLast() : workspace.redoLast();
      await held.started;
      const changing = board.setCardTitle(card.id, 'Two');
      held.release();
      await Promise.all([carrying, changing]);
      expect(workspace.problems.map((one) => one.action)).toEqual(['undo']);
      expect((await backend.card(board.boardId, card.id)).title).toBe('Two');

      await workspace.undoLast();

      expect((await backend.card(board.boardId, card.id)).title).toBe(expected);
      expect(board.card(card.id)?.title).toBe(expected);
    } finally {
      held.release();
      workspace.close();
    }
  });
});

describe('a change of the board made while its undo is refused', () => {
  it('stays on the screen, and is undone to the value stored', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const title = (): string | undefined => board.board?.title;
    const stored = async (): Promise<string> => (await backend.container(board.boardId)).title;
    const before = title();
    await board.changeBoard((current) => ({ ...current, title: 'One' }));
    let release = (): void => undefined;
    let entered = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    const patchBoard = backend.patchContainer.bind(backend);
    backend.patchContainer = async () => {
      backend.patchContainer = patchBoard;
      entered();
      await gate;
      throw new BackendError(503);
    };
    try {
      const undoing = workspace.undoLast();
      await started;
      // Answered before the refusal of the undo.
      await board.changeBoard((current) => ({ ...current, title: 'Two' }));
      release();
      await undoing;
      expect([await stored(), title()]).toEqual(['Two', 'Two']);

      await workspace.undoLast();
      expect([await stored(), title()]).toEqual(['One', 'One']);
      expect(before).not.toBe('One');
    } finally {
      release();
      workspace.close();
    }
  });
});

describe('an undo that waits while the reader moves on (found by a review of 042ff43)', () => {
  it.each(['card', 'board'])('is carried out after its %s was closed', async (closed) => {
    const { board, backend, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('missing card');
    await board.setCardTitle(card.id, 'One');
    await board.showCard(card.id);
    const second = await backend.createBoard(
      workspace.info?.id ?? '',
      { title: 'Other' },
      { title: 'View', fields: createBoardView().fields },
    );
    const held = holdNextCardWrite(backend);
    try {
      const changing = board.setCardTitle(card.id, 'Two');
      const undoing = workspace.undoLast();
      await held.started;
      if (closed === 'card') board.closeCard();
      else await workspace.openBoard(second.board.id);
      held.release();
      await Promise.all([changing, undoing]);

      expect((await backend.card(board.boardId, card.id)).title).toBe('One');
      expect(workspace.undoBusy).toBe(false);
      expect(workspace.problems).toEqual([]);
      if (closed === 'card') expect(board.openCard).toBeUndefined();
      else expect(workspace.board?.boardId).toBe(second.board.id);
    } finally {
      held.release();
      workspace.close();
    }
  });

  it('keeps what another person changed in another property meanwhile', async () => {
    const { board, backend, other, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('missing card');
    await board.setCardTitle(card.id, 'One');
    const held = holdNextCardWrite(backend);
    try {
      const changing = board.setCardTitle(card.id, 'Two');
      const undoing = workspace.undoLast();
      await held.started;
      await other.patchCard(board.boardId, card.id, { updatedProperties: { [STATUS]: 'done' } });
      held.release();
      await Promise.all([changing, undoing]);

      const saved = await backend.card(board.boardId, card.id);
      expect(saved.title).toBe('One');
      expect(saved.fields.properties[STATUS]).toBe('done');
      expect(board.card(card.id)?.fields.properties[STATUS]).toBe('done');
      expect(workspace.problems).toEqual([]);
    } finally {
      held.release();
      workspace.close();
    }
  });
});

/** Holds the next duplicateCard call until `release`; `started` settles when it is asked. */
function holdNextCardRead(backend: MemoryBackend) {
  let release = (): void => undefined;
  let entered = (): void => undefined;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const started = new Promise<void>((resolve) => {
    entered = resolve;
  });
  const duplicate = backend.duplicateCard.bind(backend);
  backend.duplicateCard = async (...args) => {
    backend.duplicateCard = duplicate;
    entered();
    await gate;
    return duplicate(...args);
  };
  return { release, started };
}

describe('a duplicate, which reads its card before it writes (found by a review of 042ff43)', () => {
  it('is what an undo asked for during the reading takes back, not the change before it', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('missing card');
    await board.setCardTitle(card.id, 'One');
    const before = (await backend.cards(board.boardId)).length;
    const held = holdNextCardRead(backend);
    try {
      const copying = board.duplicateCard(card.id);
      await held.started;
      const undoing = workspace.undoLast();
      await settle();
      held.release();
      await Promise.all([copying, undoing]);

      expect((await backend.card(board.boardId, card.id)).title).toBe('One');
      expect((await backend.cards(board.boardId)).length).toBe(before);
    } finally {
      held.release();
      workspace.close();
    }
  });

  it('made late is not in the history of another workspace entered meanwhile', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const card = board.realCards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('missing card');
    const other = { id: 'other-workspace', title: 'Other' };
    workspace.workspaces = [...workspace.workspaces, other];
    const users = backend.users.bind(backend);
    const boards = backend.boards.bind(backend);
    const categories = backend.categories.bind(backend);
    vi.spyOn(backend, 'users').mockImplementation((id) =>
      id === other.id ? Promise.resolve([]) : users(id),
    );
    vi.spyOn(backend, 'boards').mockImplementation((id) =>
      id === other.id ? Promise.resolve([]) : boards(id),
    );
    vi.spyOn(backend, 'categories').mockImplementation((id) =>
      id === other.id ? Promise.resolve([]) : categories(id),
    );
    const wikis = backend.wiki.wikis.bind(backend.wiki);
    vi.spyOn(backend.wiki, 'wikis').mockImplementation((id) =>
      id === other.id ? Promise.resolve([]) : wikis(id),
    );
    const courses = backend.course.courses.bind(backend.course);
    vi.spyOn(backend.course, 'courses').mockImplementation((id) =>
      id === other.id ? Promise.resolve([]) : courses(id),
    );
    const held = holdNextCardRead(backend);
    try {
      const copying = board.duplicateCard(card.id);
      await held.started;
      await workspace.enter(other.id);
      expect(workspace.canUndo).toBe(false);
      held.release();
      const copy = await copying;

      expect(workspace.info?.id).toBe(other.id);
      expect(workspace.canUndo).toBe(false);
      await workspace.undoLast();
      // Made in the workspace left, the undo of the one entered does not delete it.
      expect(copy).toBeDefined();
      expect((await backend.cards(board.boardId)).some((one) => one.id === copy?.id)).toBe(true);
    } finally {
      held.release();
      workspace.close();
    }
  });
});
